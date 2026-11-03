//! Compare an unpublished restore with authenticated archive records. Hole
//! verification reads only actual extents, accepting zero allocation differences
//! while detecting nonzero data written into an expected hole.
use super::*;
use std::os::unix::fs::FileExt;

fn times(fd: &File, entry: &Entry) -> Result<()> {
    let times = [
        libc::timespec {
            tv_sec: entry.atime,
            tv_nsec: entry.atime_nsec,
        },
        libc::timespec {
            tv_sec: entry.mtime,
            tv_nsec: entry.mtime_nsec,
        },
    ];
    if unsafe { libc::futimens(fd.as_raw_fd(), times.as_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    fd.sync_all()?;
    Ok(())
}
fn check_meta(meta: &fs::Metadata, entry: &Entry) -> Result<()> {
    let kind = match entry.kind {
        Kind::Directory => libc::S_IFDIR,
        Kind::File | Kind::Hardlink => libc::S_IFREG,
        Kind::Symlink => libc::S_IFLNK,
        Kind::Fifo => libc::S_IFIFO,
    };
    if meta.mode() & libc::S_IFMT != kind
        || meta.mode() & 0o7777 != entry.mode
        || meta.uid() != entry.uid
        || meta.gid() != entry.gid
        || meta.mtime() != entry.mtime
        || meta.mtime_nsec() != entry.mtime_nsec
        || (entry.kind == Kind::File && meta.len() != entry.size)
    {
        return Err(invalid());
    }
    Ok(())
}
fn charge(total: &mut u64, bytes: u64, maximum: u64) -> Result<()> {
    *total = total
        .checked_add(bytes)
        .filter(|n| *n <= maximum)
        .ok_or_else(Error::exhausted)?;
    Ok(())
}
fn zero_gap(fd: &File, mut start: u64, end: u64, reads: &mut u64, maximum: u64) -> Result<()> {
    let mut buffer = zeroize::Zeroizing::new(vec![0u8; 65536]);
    while start < end {
        let at = unsafe { libc::lseek(fd.as_raw_fd(), start as libc::off_t, libc::SEEK_DATA) };
        if at < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENXIO) {
                return Ok(());
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if at as u64 >= end {
            return Ok(());
        }
        if (at as u64) < start {
            return Err(invalid());
        }
        let hole = unsafe { libc::lseek(fd.as_raw_fd(), at, libc::SEEK_HOLE) };
        if hole <= at {
            return Err(invalid());
        }
        start = at as u64;
        let until = (hole as u64).min(end);
        while start < until {
            let n = (until - start).min(buffer.len() as u64) as usize;
            charge(reads, n as u64, maximum)?;
            fd.read_exact_at(&mut buffer[..n], start)?;
            if buffer[..n].iter().any(|b| *b != 0) {
                return Err(invalid());
            }
            start += n as u64;
        }
    }
    Ok(())
}

pub fn verify(path: &Path, input: &mut impl Read, volume: Uuid, maximum: u64) -> Result<Summary> {
    let root = root(path)?;
    let device = root.metadata()?.dev();
    let mount = mount_id(&root)?;
    let mut header = [0u8; 24];
    input.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..8] != MAGIC || header[8..] != *volume.as_bytes() {
        return Err(invalid());
    }
    let mut nodes: Vec<Entry> = Vec::new();
    let mut children: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    let mut links: BTreeMap<u32, u64> = BTreeMap::new();
    let mut metadata_bytes = 0;
    let mut data_bytes = 0;
    let mut reads = 0;
    loop {
        match read_record(input, &mut metadata_bytes)? {
            Record::End {
                entries,
                data_bytes: expected,
            } => {
                if nodes.is_empty() || entries as usize != nodes.len() || expected != data_bytes {
                    return Err(invalid());
                }
                let mut extra = [0];
                if input.read(&mut extra)? != 0 {
                    return Err(invalid());
                }
                let mut counted = 0usize;
                for node in nodes.iter().rev() {
                    if node.kind == Kind::Directory {
                        let fd = directory(&root, &nodes, node.id)?;
                        same_fs(&fd, device, mount)?;
                        let before = fd.metadata()?;
                        check_meta(&before, node)?;
                        let mut actual = BTreeSet::new();
                        for child in fs::read_dir(format!("/proc/self/fd/{}", fd.as_raw_fd()))? {
                            counted += 1;
                            if counted >= MAX_ENTRIES {
                                return Err(invalid());
                            }
                            actual.insert(STANDARD.encode(child?.file_name().as_bytes()));
                        }
                        if actual != children.remove(&node.id).unwrap_or_default() {
                            return Err(invalid());
                        }
                        let after = fd.metadata()?;
                        if after.ctime() != before.ctime()
                            || after.ctime_nsec() != before.ctime_nsec()
                        {
                            return Err(invalid());
                        }
                        times(&fd, node)?;
                    } else if node.kind == Kind::File {
                        let parent = directory(&root, &nodes, node.parent)?;
                        let fd = open_at(
                            &parent,
                            &decode_name(&node.name)?,
                            libc::O_RDONLY | libc::O_NONBLOCK,
                            0,
                        )?;
                        same_fs(&fd, device, mount)?;
                        if fd.metadata()?.nlink() != *links.get(&node.id).ok_or_else(invalid)? {
                            return Err(invalid());
                        }
                    }
                }
                root.sync_all()?;
                if unsafe { libc::syncfs(root.as_raw_fd()) } < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                return Ok(Summary {
                    entries,
                    data_bytes,
                });
            }
            Record::Entry(entry) => {
                if entry.id as usize != nodes.len()
                    || nodes.len() >= MAX_ENTRIES
                    || entry.mode & !0o7777 != 0
                    || entry.size > i64::MAX as u64
                    || ![0, 65532, unsafe { libc::geteuid() }].contains(&entry.uid)
                    || ![0, 65532, unsafe { libc::getegid() }].contains(&entry.gid)
                    || !(0..1000000000).contains(&entry.atime_nsec)
                    || !(0..1000000000).contains(&entry.mtime_nsec)
                {
                    return Err(invalid());
                }
                if nodes.is_empty() {
                    if entry.kind != Kind::Directory
                        || entry.parent != 0
                        || !entry.name.is_empty()
                        || entry.size != 0
                        || entry.link_to.is_some()
                        || entry.target.is_some()
                    {
                        return Err(invalid());
                    }
                    check_meta(&root.metadata()?, &entry)?;
                    if attributes(&root)? != entry.attributes {
                        return Err(invalid());
                    }
                    nodes.push(entry);
                    continue;
                }
                if entry.parent >= entry.id
                    || nodes[entry.parent as usize].kind != Kind::Directory
                    || !children
                        .entry(entry.parent)
                        .or_default()
                        .insert(entry.name.clone())
                {
                    return Err(invalid());
                }
                let parent = directory(&root, &nodes, entry.parent)?;
                same_fs(&parent, device, mount)?;
                let name = decode_name(&entry.name)?;
                if entry.kind == Kind::Symlink {
                    if entry.size != 0 || entry.link_to.is_some() || !entry.attributes.is_empty() {
                        return Err(invalid());
                    }
                    let path = Path::new("/proc/self/fd")
                        .join(parent.as_raw_fd().to_string())
                        .join(std::ffi::OsStr::from_bytes(name.as_bytes()));
                    let meta = fs::symlink_metadata(&path)?;
                    if meta.dev() != device {
                        return Err(invalid());
                    }
                    check_meta(&meta, &entry)?;
                    let target = STANDARD
                        .decode(entry.target.as_ref().ok_or_else(invalid)?)
                        .map_err(|_| invalid())?;
                    if fs::read_link(path)?.as_os_str().as_bytes() != target {
                        return Err(invalid());
                    }
                    let t = [
                        libc::timespec {
                            tv_sec: entry.atime,
                            tv_nsec: entry.atime_nsec,
                        },
                        libc::timespec {
                            tv_sec: entry.mtime,
                            tv_nsec: entry.mtime_nsec,
                        },
                    ];
                    if unsafe {
                        libc::utimensat(
                            parent.as_raw_fd(),
                            name.as_ptr(),
                            t.as_ptr(),
                            libc::AT_SYMLINK_NOFOLLOW,
                        )
                    } < 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                } else {
                    let flags = libc::O_RDONLY
                        | libc::O_NONBLOCK
                        | if entry.kind == Kind::Directory {
                            libc::O_DIRECTORY
                        } else {
                            0
                        };
                    let fd = open_at(&parent, &name, flags, 0)?;
                    same_fs(&fd, device, mount)?;
                    let before = fd.metadata()?;
                    check_meta(&before, &entry)?;
                    if entry.kind != Kind::Hardlink && attributes(&fd)? != entry.attributes {
                        return Err(invalid());
                    }
                    match entry.kind {
                        Kind::File => {
                            if entry.link_to.is_some() || entry.target.is_some() {
                                return Err(invalid());
                            }
                            links.insert(entry.id, 1);
                            let mut previous = 0;
                            let mut segments = 0u64;
                            loop {
                                let mut raw = [0u8; 12];
                                input.read_exact(&mut raw).map_err(|_| invalid())?;
                                let offset = u64::from_be_bytes(raw[..8].try_into().unwrap());
                                let length =
                                    u32::from_be_bytes(raw[8..].try_into().unwrap()) as usize;
                                if offset == u64::MAX && length == 0 {
                                    break;
                                }
                                segments += 1;
                                if segments > u32::MAX as u64
                                    || length == 0
                                    || length > 65536
                                    || offset < previous
                                    || offset
                                        .checked_add(length as u64)
                                        .is_none_or(|n| n > entry.size)
                                {
                                    return Err(invalid());
                                }
                                zero_gap(&fd, previous, offset, &mut reads, maximum)?;
                                charge(&mut data_bytes, length as u64, maximum)?;
                                charge(&mut reads, length as u64, maximum)?;
                                let mut expected = zeroize::Zeroizing::new(vec![0u8; length]);
                                let mut actual = zeroize::Zeroizing::new(vec![0u8; length]);
                                input.read_exact(&mut expected).map_err(|_| invalid())?;
                                fd.read_exact_at(&mut actual, offset)?;
                                if expected != actual {
                                    return Err(invalid());
                                }
                                previous = offset + length as u64;
                            }
                            zero_gap(&fd, previous, entry.size, &mut reads, maximum)?;
                            let after = fd.metadata()?;
                            check_meta(&after, &entry)?;
                            if after.ctime() != before.ctime()
                                || after.ctime_nsec() != before.ctime_nsec()
                            {
                                return Err(invalid());
                            }
                            times(&fd, &entry)?;
                        }
                        Kind::Hardlink => {
                            let first = entry
                                .link_to
                                .and_then(|n| nodes.get(n as usize))
                                .filter(|n| n.kind == Kind::File)
                                .ok_or_else(invalid)?;
                            if entry.size != 0
                                || entry.target.is_some()
                                || !entry.attributes.is_empty()
                            {
                                return Err(invalid());
                            }
                            let source_parent = directory(&root, &nodes, first.parent)?;
                            let source = open_at(
                                &source_parent,
                                &decode_name(&first.name)?,
                                libc::O_RDONLY | libc::O_NONBLOCK,
                                0,
                            )?;
                            same_fs(&source, device, mount)?;
                            if source.metadata()?.ino() != before.ino() {
                                return Err(invalid());
                            }
                            *links.get_mut(&first.id).ok_or_else(invalid)? += 1;
                        }
                        Kind::Directory | Kind::Fifo => {
                            if entry.size != 0 || entry.link_to.is_some() || entry.target.is_some()
                            {
                                return Err(invalid());
                            }
                        }
                        Kind::Symlink => unreachable!(),
                    }
                }
                nodes.push(entry);
            }
        }
    }
}
