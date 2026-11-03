//! Bounded, sparse-aware volume archives. Names are encoded bytes and parents
//! are earlier directory IDs; extraction never resolves a supplied path or link.
use crate::{
    error::{Error, Result},
    storage::xfs::mount_id,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
};
use uuid::Uuid;
const MAGIC: &[u8; 8] = b"XELABVA1";
const MAX_ENTRIES: usize = 65536;
const MAX_DEPTH: usize = 256;
const MAX_RECORD: usize = 262144;
const MAX_METADATA: u64 = 67108864;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Directory,
    File,
    Hardlink,
    Symlink,
    Fifo,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attribute {
    name: String,
    value: String,
}

#[path = "archive_verify.rs"]
mod verification;
pub use verification::verify;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: u32,
    parent: u32,
    name: String,
    kind: Kind,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    atime: i64,
    atime_nsec: i64,
    mtime: i64,
    mtime_nsec: i64,
    link_to: Option<u32>,
    target: Option<String>,
    attributes: Vec<Attribute>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
enum Record {
    Entry(Entry),
    End { entries: u32, data_bytes: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub entries: u32,
    pub data_bytes: u64,
}
fn invalid() -> Error {
    Error::unavailable("Volume archive is invalid or unsupported")
}
fn name(bytes: &[u8]) -> Result<CString> {
    if bytes.is_empty()
        || bytes.len() > 255
        || matches!(bytes, b"." | b"..")
        || bytes.contains(&b'/')
    {
        return Err(invalid());
    }
    CString::new(bytes).map_err(|_| invalid())
}
fn open_at(parent: &File, name: &CString, flags: i32, mode: u32) -> Result<File> {
    // SAFETY: the name is NUL terminated and a successful descriptor is owned once.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn root(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?)
}
fn same_fs(fd: &File, device: u64, mount: u64) -> Result<()> {
    if fd.metadata()?.dev() != device || mount_id(fd)? != mount {
        return Err(invalid());
    }
    Ok(())
}
fn attributes(fd: &File) -> Result<Vec<Attribute>> {
    let count = unsafe { libc::flistxattr(fd.as_raw_fd(), std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if count > 65536 {
        return Err(invalid());
    }
    let mut names = vec![0u8; count as usize];
    if count > 0
        && unsafe { libc::flistxattr(fd.as_raw_fd(), names.as_mut_ptr().cast(), names.len()) }
            != count
    {
        return Err(invalid());
    }
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for raw in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        // Extended application attributes are data. Unsupported privileged/ACL
        // metadata is rejected rather than silently discarded or reinterpreted.
        if !raw.starts_with(b"user.") {
            return Err(invalid());
        }
        let c = CString::new(raw).map_err(|_| invalid())?;
        let length =
            unsafe { libc::fgetxattr(fd.as_raw_fd(), c.as_ptr(), std::ptr::null_mut(), 0) };
        if !(0..=65536).contains(&length) {
            return Err(invalid());
        }
        bytes = bytes
            .checked_add(raw.len() + length as usize)
            .filter(|n| *n <= 65536)
            .ok_or_else(invalid)?;
        let mut value = vec![0u8; length as usize];
        if unsafe {
            libc::fgetxattr(
                fd.as_raw_fd(),
                c.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        } != length
        {
            return Err(invalid());
        }
        out.push(Attribute {
            name: STANDARD.encode(raw),
            value: STANDARD.encode(value),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
fn entry(
    id: u32,
    parent: u32,
    raw: &[u8],
    kind: Kind,
    meta: &fs::Metadata,
    fd: Option<&File>,
) -> Result<Entry> {
    if ![0, 65532, unsafe { libc::geteuid() }].contains(&meta.uid())
        || ![0, 65532, unsafe { libc::getegid() }].contains(&meta.gid())
    {
        return Err(invalid());
    }
    Ok(Entry {
        id,
        parent,
        name: STANDARD.encode(raw),
        kind,
        mode: meta.mode() & 0o7777,
        uid: meta.uid(),
        gid: meta.gid(),
        size: if kind == Kind::File { meta.len() } else { 0 },
        atime: meta.atime(),
        atime_nsec: meta.atime_nsec(),
        mtime: meta.mtime(),
        mtime_nsec: meta.mtime_nsec(),
        link_to: None,
        target: None,
        attributes: fd.map(attributes).transpose()?.unwrap_or_default(),
    })
}
fn write_record(output: &mut impl Write, record: &Record, metadata: &mut u64) -> Result<()> {
    let bytes = serde_json::to_vec(record)?;
    if bytes.len() > MAX_RECORD {
        return Err(invalid());
    }
    *metadata = metadata
        .checked_add(bytes.len() as u64)
        .filter(|n| *n <= MAX_METADATA)
        .ok_or_else(invalid)?;
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(&bytes)?;
    Ok(())
}
struct Packing {
    next: u32,
    queued: usize,
    metadata: u64,
    data: u64,
    maximum: u64,
    device: u64,
    mount: u64,
    links: BTreeMap<u64, u32>,
}
fn pack_dir(
    parent: &File,
    parent_id: u32,
    depth: usize,
    output: &mut impl Write,
    state: &mut Packing,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(invalid());
    }
    let original = parent.metadata()?;
    let mut children = Vec::new();
    for child in fs::read_dir(format!("/proc/self/fd/{}", parent.as_raw_fd()))? {
        // Bound directory enumeration before allocation, across all recursive
        // frames. A visited-entry limit alone leaves pending sibling lists
        // unbounded when each level contains a wide subtree.
        if state.next as usize + state.queued >= MAX_ENTRIES {
            return Err(invalid());
        }
        children.push(child?);
        state.queued += 1;
    }
    children.sort_by(|a, b| a.file_name().as_bytes().cmp(b.file_name().as_bytes()));
    for child in children {
        state.queued -= 1;
        if state.next as usize >= MAX_ENTRIES {
            return Err(invalid());
        }
        let id = state.next;
        state.next += 1;
        let raw = child.file_name();
        let c = name(raw.as_bytes())?;
        let meta = fs::symlink_metadata(child.path())?;
        if meta.dev() != state.device {
            return Err(invalid());
        }
        let kind = match meta.mode() & libc::S_IFMT {
            libc::S_IFDIR => Kind::Directory,
            libc::S_IFREG => Kind::File,
            libc::S_IFLNK => Kind::Symlink,
            libc::S_IFIFO => Kind::Fifo,
            _ => return Err(invalid()),
        };
        if kind == Kind::Symlink {
            let target = fs::read_link(child.path())?;
            let bytes = target.as_os_str().as_bytes();
            if bytes.is_empty() || bytes.len() > 4096 {
                return Err(invalid());
            }
            let mut record = entry(id, parent_id, raw.as_bytes(), kind, &meta, None)?;
            record.target = Some(STANDARD.encode(bytes));
            write_record(output, &Record::Entry(record), &mut state.metadata)?;
            continue;
        }
        let flags = if kind == Kind::Directory {
            libc::O_RDONLY | libc::O_DIRECTORY
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK
        };
        let mut fd = open_at(parent, &c, flags, 0)?;
        same_fs(&fd, state.device, state.mount)?;
        let before = fd.metadata()?;
        if before.ino() != meta.ino() || before.mode() & libc::S_IFMT != meta.mode() & libc::S_IFMT
        {
            return Err(invalid());
        }
        let mut record = entry(id, parent_id, raw.as_bytes(), kind, &before, Some(&fd))?;
        if kind == Kind::File {
            if let Some(first) = state.links.get(&before.ino()) {
                record.kind = Kind::Hardlink;
                record.size = 0;
                record.link_to = Some(*first);
                record.attributes.clear();
                write_record(output, &Record::Entry(record), &mut state.metadata)?;
                continue;
            }
            state.links.insert(before.ino(), id);
        }
        write_record(output, &Record::Entry(record), &mut state.metadata)?;
        if kind == Kind::Directory {
            pack_dir(&fd, id, depth + 1, output, state)?;
        } else if kind == Kind::File {
            let mut offset = 0u64;
            let mut segments = 0u64;
            let mut buffer = zeroize::Zeroizing::new(vec![0u8; 65536]);
            while offset < before.len() {
                let data =
                    unsafe { libc::lseek(fd.as_raw_fd(), offset as libc::off_t, libc::SEEK_DATA) };
                if data < 0 {
                    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENXIO) {
                        break;
                    }
                    return Err(std::io::Error::last_os_error().into());
                }
                let hole = unsafe { libc::lseek(fd.as_raw_fd(), data, libc::SEEK_HOLE) };
                if hole <= data {
                    return Err(invalid());
                }
                let mut at = data as u64;
                let end = (hole as u64).min(before.len());
                if at < offset || at >= end {
                    return Err(invalid());
                }
                fd.seek(SeekFrom::Start(at))?;
                while at < end {
                    segments += 1;
                    if segments > u32::MAX as u64 {
                        return Err(invalid());
                    }
                    let n = ((end - at).min(buffer.len() as u64)) as usize;
                    fd.read_exact(&mut buffer[..n])?;
                    state.data = state
                        .data
                        .checked_add(n as u64)
                        .filter(|v| *v <= state.maximum)
                        .ok_or_else(Error::exhausted)?;
                    output.write_all(&at.to_be_bytes())?;
                    output.write_all(&(n as u32).to_be_bytes())?;
                    output.write_all(&buffer[..n])?;
                    at += n as u64;
                }
                offset = end;
            }
            output.write_all(&u64::MAX.to_be_bytes())?;
            output.write_all(&0u32.to_be_bytes())?;
            let after = fd.metadata()?;
            if after.len() != before.len()
                || after.mtime() != before.mtime()
                || after.mtime_nsec() != before.mtime_nsec()
                || after.ctime() != before.ctime()
                || after.ctime_nsec() != before.ctime_nsec()
            {
                return Err(Error::conflict("Volume changed during backup"));
            }
        }
    }
    let observed = parent.metadata()?;
    if observed.mtime() != original.mtime()
        || observed.mtime_nsec() != original.mtime_nsec()
        || observed.ctime() != original.ctime()
        || observed.ctime_nsec() != original.ctime_nsec()
    {
        return Err(Error::conflict("Volume directory changed during backup"));
    }
    Ok(())
}
pub fn pack(path: &Path, output: &mut impl Write, volume: Uuid, maximum: u64) -> Result<Summary> {
    let fd = root(path)?;
    let meta = fd.metadata()?;
    let mut state = Packing {
        next: 1,
        queued: 0,
        metadata: 0,
        data: 0,
        maximum,
        device: meta.dev(),
        mount: mount_id(&fd)?,
        links: BTreeMap::new(),
    };
    output.write_all(MAGIC)?;
    output.write_all(volume.as_bytes())?;
    write_record(
        output,
        &Record::Entry(entry(0, 0, b"", Kind::Directory, &meta, Some(&fd))?),
        &mut state.metadata,
    )?;
    pack_dir(&fd, 0, 0, output, &mut state)?;
    write_record(
        output,
        &Record::End {
            entries: state.next,
            data_bytes: state.data,
        },
        &mut state.metadata,
    )?;
    output.flush()?;
    Ok(Summary {
        entries: state.next,
        data_bytes: state.data,
    })
}
fn read_record(input: &mut impl Read, metadata: &mut u64) -> Result<Record> {
    let mut raw = [0u8; 4];
    input.read_exact(&mut raw).map_err(|_| invalid())?;
    let length = u32::from_be_bytes(raw) as usize;
    if length == 0 || length > MAX_RECORD {
        return Err(invalid());
    }
    *metadata = metadata
        .checked_add(length as u64)
        .filter(|n| *n <= MAX_METADATA)
        .ok_or_else(invalid)?;
    let mut body = vec![0u8; length];
    input.read_exact(&mut body).map_err(|_| invalid())?;
    serde_json::from_slice(&body).map_err(|_| invalid())
}
fn decode_name(value: &str) -> Result<CString> {
    name(&STANDARD.decode(value).map_err(|_| invalid())?)
}
fn directory(root: &File, nodes: &[Entry], id: u32) -> Result<File> {
    let mut at = id;
    let mut components = Vec::new();
    while at != 0 {
        let node = nodes.get(at as usize).ok_or_else(invalid)?;
        if node.kind != Kind::Directory || components.len() >= MAX_DEPTH {
            return Err(invalid());
        }
        components.push(decode_name(&node.name)?);
        at = node.parent;
    }
    let mut fd = root.try_clone()?;
    for name in components.iter().rev() {
        fd = open_at(&fd, name, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    }
    Ok(fd)
}
fn metadata(fd: &File, entry: &Entry) -> Result<()> {
    if unsafe { libc::fchown(fd.as_raw_fd(), entry.uid, entry.gid) } < 0
        || unsafe { libc::fchmod(fd.as_raw_fd(), entry.mode) } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    for attr in &entry.attributes {
        let raw = STANDARD.decode(&attr.name).map_err(|_| invalid())?;
        let value = STANDARD.decode(&attr.value).map_err(|_| invalid())?;
        total = total
            .checked_add(raw.len() + value.len())
            .filter(|v| *v <= 65536)
            .ok_or_else(invalid)?;
        if !raw.starts_with(b"user.") || !names.insert(raw.clone()) {
            return Err(invalid());
        }
        let name = CString::new(raw).map_err(|_| invalid())?;
        if unsafe {
            libc::fsetxattr(
                fd.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
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
    if fd.metadata()?.mode() & libc::S_IFMT != libc::S_IFIFO {
        fd.sync_all()?;
    }
    Ok(())
}
pub fn unpack(path: &Path, input: &mut impl Read, volume: Uuid, maximum: u64) -> Result<Summary> {
    let root = root(path)?;
    if fs::read_dir(format!("/proc/self/fd/{}", root.as_raw_fd()))?
        .next()
        .is_some()
    {
        return Err(Error::conflict("Archive destination must be new and empty"));
    }
    let mut header = [0u8; 24];
    input.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..8] != MAGIC || header[8..] != *volume.as_bytes() {
        return Err(invalid());
    }
    let device = root.metadata()?.dev();
    let mount = mount_id(&root)?;
    let mut nodes: Vec<Entry> = Vec::new();
    let mut children = BTreeSet::new();
    let mut meta_bytes = 0;
    let mut data_bytes = 0u64;
    loop {
        match read_record(input, &mut meta_bytes)? {
            Record::End {
                entries,
                data_bytes: expected,
            } => {
                if entries as usize != nodes.len() || expected != data_bytes || nodes.is_empty() {
                    return Err(invalid());
                }
                let mut trailing = [0u8; 1];
                if input.read(&mut trailing)? != 0 {
                    return Err(invalid());
                }
                for node in nodes.iter().rev().filter(|n| n.kind == Kind::Directory) {
                    let fd = directory(&root, &nodes, node.id)?;
                    same_fs(&fd, device, mount)?;
                    metadata(&fd, node)?;
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
                    || ![0, 65532, unsafe { libc::geteuid() }].contains(&entry.uid)
                    || ![0, 65532, unsafe { libc::getegid() }].contains(&entry.gid)
                    || !(0..1000000000).contains(&entry.atime_nsec)
                    || !(0..1000000000).contains(&entry.mtime_nsec)
                    || entry.size > i64::MAX as u64
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
                    nodes.push(entry);
                    continue;
                }
                if entry.parent >= entry.id
                    || nodes[entry.parent as usize].kind != Kind::Directory
                    || !children.insert((entry.parent, entry.name.clone()))
                {
                    return Err(invalid());
                }
                let name = decode_name(&entry.name)?;
                let parent = directory(&root, &nodes, entry.parent)?;
                same_fs(&parent, device, mount)?;
                match entry.kind {
                    Kind::Directory => {
                        if entry.size != 0 || entry.link_to.is_some() || entry.target.is_some() {
                            return Err(invalid());
                        }
                        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
                            return Err(std::io::Error::last_os_error().into());
                        }
                    }
                    Kind::File => {
                        if entry.link_to.is_some() || entry.target.is_some() {
                            return Err(invalid());
                        }
                        let mut fd = open_at(
                            &parent,
                            &name,
                            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
                            0o600,
                        )?;
                        same_fs(&fd, device, mount)?;
                        fd.set_len(entry.size)?;
                        let mut previous = 0;
                        let mut segments = 0u64;
                        loop {
                            let mut offset = [0u8; 8];
                            let mut length = [0u8; 4];
                            input.read_exact(&mut offset).map_err(|_| invalid())?;
                            input.read_exact(&mut length).map_err(|_| invalid())?;
                            let offset = u64::from_be_bytes(offset);
                            let length = u32::from_be_bytes(length);
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
                                    .is_none_or(|v| v > entry.size)
                            {
                                return Err(invalid());
                            }
                            data_bytes = data_bytes
                                .checked_add(length as u64)
                                .filter(|v| *v <= maximum)
                                .ok_or_else(Error::exhausted)?;
                            let mut data = vec![0u8; length as usize];
                            input.read_exact(&mut data).map_err(|_| invalid())?;
                            fd.seek(SeekFrom::Start(offset))?;
                            fd.write_all(&data)?;
                            previous = offset + length as u64;
                        }
                        metadata(&fd, &entry)?;
                    }
                    Kind::Hardlink => {
                        let first = entry
                            .link_to
                            .and_then(|id| nodes.get(id as usize))
                            .ok_or_else(invalid)?;
                        if first.kind != Kind::File
                            || entry.size != 0
                            || entry.target.is_some()
                            || !entry.attributes.is_empty()
                            || entry.uid != first.uid
                            || entry.gid != first.gid
                            || entry.mode != first.mode
                            || entry.mtime != first.mtime
                            || entry.mtime_nsec != first.mtime_nsec
                        {
                            return Err(invalid());
                        }
                        let source_parent = directory(&root, &nodes, first.parent)?;
                        let source = decode_name(&first.name)?;
                        if unsafe {
                            libc::linkat(
                                source_parent.as_raw_fd(),
                                source.as_ptr(),
                                parent.as_raw_fd(),
                                name.as_ptr(),
                                0,
                            )
                        } < 0
                        {
                            return Err(std::io::Error::last_os_error().into());
                        }
                    }
                    Kind::Symlink => {
                        if entry.size != 0
                            || entry.link_to.is_some()
                            || !entry.attributes.is_empty()
                            || entry.mode != 0o777
                        {
                            return Err(invalid());
                        }
                        let target = STANDARD
                            .decode(entry.target.as_ref().ok_or_else(invalid)?)
                            .map_err(|_| invalid())?;
                        if target.is_empty() || target.len() > 4096 {
                            return Err(invalid());
                        }
                        let target = CString::new(target).map_err(|_| invalid())?;
                        if unsafe {
                            libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr())
                        } < 0
                            || unsafe {
                                libc::fchownat(
                                    parent.as_raw_fd(),
                                    name.as_ptr(),
                                    entry.uid,
                                    entry.gid,
                                    libc::AT_SYMLINK_NOFOLLOW,
                                )
                            } < 0
                        {
                            return Err(std::io::Error::last_os_error().into());
                        }
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
                        if unsafe {
                            libc::utimensat(
                                parent.as_raw_fd(),
                                name.as_ptr(),
                                times.as_ptr(),
                                libc::AT_SYMLINK_NOFOLLOW,
                            )
                        } < 0
                        {
                            return Err(std::io::Error::last_os_error().into());
                        }
                    }
                    Kind::Fifo => {
                        if entry.size != 0 || entry.link_to.is_some() || entry.target.is_some() {
                            return Err(invalid());
                        }
                        if unsafe { libc::mkfifoat(parent.as_raw_fd(), name.as_ptr(), 0o600) } < 0 {
                            return Err(std::io::Error::last_os_error().into());
                        }
                        let fd = open_at(&parent, &name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
                        metadata(&fd, &entry)?;
                    }
                }
                parent.sync_all()?;
                nodes.push(entry);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{symlink, FileExt, FileTypeExt, PermissionsExt},
    };
    #[test]
    fn sparse_data_links_fifo_attributes_and_permissions_roundtrip_without_following_links() {
        let source = tempfile::tempdir_in("/tmp").unwrap();
        let target = tempfile::tempdir_in("/tmp").unwrap();
        let outside = tempfile::tempdir_in("/tmp").unwrap();
        let victim = outside.path().join("retained");
        fs::write(&victim, b"outside").unwrap();
        fs::create_dir(source.path().join("nested")).unwrap();
        let payload = source.path().join("nested/payload");
        fs::write(&payload, b"payload").unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o640)).unwrap();
        fs::hard_link(&payload, source.path().join("alias")).unwrap();
        symlink(&victim, source.path().join("outside-link")).unwrap();
        let fd = File::open(&payload).unwrap();
        let attr = CString::new("user.xelab-test").unwrap();
        assert_eq!(
            unsafe {
                libc::fsetxattr(
                    fd.as_raw_fd(),
                    attr.as_ptr(),
                    b"metadata".as_ptr().cast(),
                    8,
                    0,
                )
            },
            0
        );
        let fifo = CString::new(source.path().join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let sparse = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(source.path().join("sparse"))
            .unwrap();
        let length = 8u64 * 1024 * 1024 * 1024;
        sparse.set_len(length).unwrap();
        sparse.write_all_at(b"head", 0).unwrap();
        sparse.write_all_at(b"tail", length - 4).unwrap();
        sparse.sync_all().unwrap();
        let before = fd.metadata().unwrap();
        let volume = Uuid::new_v4();
        let mut encoded = Vec::new();
        let summary = pack(source.path(), &mut encoded, volume, 1048576).unwrap();
        assert!(summary.data_bytes < 131072);
        assert!(
            encoded.len() < 262144,
            "sparse holes must not become plaintext zero bytes"
        );
        assert_eq!(
            unpack(target.path(), &mut encoded.as_slice(), volume, 1048576).unwrap(),
            summary
        );
        assert_eq!(
            verify(target.path(), &mut encoded.as_slice(), volume, 1048576).unwrap(),
            summary
        );
        assert_eq!(
            fs::read(target.path().join("nested/payload")).unwrap(),
            b"payload"
        );
        assert_eq!(fs::read(&victim).unwrap(), b"outside");
        assert_eq!(
            fs::read_link(target.path().join("outside-link")).unwrap(),
            victim
        );
        assert!(fs::symlink_metadata(target.path().join("fifo"))
            .unwrap()
            .file_type()
            .is_fifo());
        let restored = File::open(target.path().join("nested/payload")).unwrap();
        let after = restored.metadata().unwrap();
        assert_eq!(after.mode() & 0o7777, 0o640);
        assert_eq!(after.mtime(), before.mtime());
        assert_eq!(after.mtime_nsec(), before.mtime_nsec());
        assert_eq!(
            after.ino(),
            fs::metadata(target.path().join("alias")).unwrap().ino()
        );
        let mut attr_bytes = [0u8; 8];
        assert_eq!(
            unsafe {
                libc::fgetxattr(
                    restored.as_raw_fd(),
                    attr.as_ptr(),
                    attr_bytes.as_mut_ptr().cast(),
                    8,
                )
            },
            8
        );
        assert_eq!(&attr_bytes, b"metadata");
        let restored = File::open(target.path().join("sparse")).unwrap();
        assert_eq!(restored.metadata().unwrap().len(), length);
        assert!(restored.metadata().unwrap().blocks() * 512 < 131072);
        let mut bytes = [0u8; 4];
        restored.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"head");
        restored.read_exact_at(&mut bytes, length - 4).unwrap();
        assert_eq!(&bytes, b"tail");
        restored
            .read_exact_at(&mut bytes, 1024 * 1024 * 1024)
            .unwrap();
        assert_eq!(bytes, [0; 4]);
        // Nonzero bytes placed into an expected sparse hole must be detected
        // even if the visible file size/mtime were restored after corruption.
        let sparse_meta = restored.metadata().unwrap();
        drop(restored);
        let altered = OpenOptions::new()
            .read(true)
            .write(true)
            .open(target.path().join("sparse"))
            .unwrap();
        altered.write_all_at(b"bad", 1024 * 1024 * 1024).unwrap();
        let stamps = [
            libc::timespec {
                tv_sec: sparse_meta.atime(),
                tv_nsec: sparse_meta.atime_nsec(),
            },
            libc::timespec {
                tv_sec: sparse_meta.mtime(),
                tv_nsec: sparse_meta.mtime_nsec(),
            },
        ];
        assert_eq!(
            unsafe { libc::futimens(altered.as_raw_fd(), stamps.as_ptr()) },
            0
        );
        assert!(verify(target.path(), &mut encoded.as_slice(), volume, 1048576).is_err());
    }
    #[test]
    fn malformed_or_incomplete_archives_cannot_escape_or_replace_existing_data() {
        let source = tempfile::tempdir_in("/tmp").unwrap();
        fs::write(source.path().join("safe"), b"bytes").unwrap();
        let volume = Uuid::new_v4();
        let mut encoded = Vec::new();
        pack(source.path(), &mut encoded, volume, 1024).unwrap();
        for length in [0, 23, 24, encoded.len() - 1] {
            let dest = tempfile::tempdir_in("/tmp").unwrap();
            assert!(unpack(dest.path(), &mut &encoded[..length], volume, 1024).is_err());
        }
        let dest = tempfile::tempdir_in("/tmp").unwrap();
        fs::write(dest.path().join("keep"), b"kept").unwrap();
        assert!(unpack(dest.path(), &mut encoded.as_slice(), volume, 1024).is_err());
        assert_eq!(fs::read(dest.path().join("keep")).unwrap(), b"kept");
        for raw in [
            b"..".as_slice(),
            b"../outside",
            b"/absolute",
            b"a/b",
            b"a\0b",
        ] {
            let mut input = &encoded[24..];
            let mut budget = 0;
            let root = read_record(&mut input, &mut budget).unwrap();
            let Record::Entry(mut child) = read_record(&mut input, &mut budget).unwrap() else {
                panic!()
            };
            child.name = STANDARD.encode(raw);
            let mut bad = encoded[..24].to_vec();
            let mut budget = 0;
            write_record(&mut bad, &root, &mut budget).unwrap();
            write_record(&mut bad, &Record::Entry(child), &mut budget).unwrap();
            bad.extend_from_slice(input);
            let dest = tempfile::tempdir_in("/tmp").unwrap();
            assert!(unpack(dest.path(), &mut bad.as_slice(), volume, 1024).is_err());
            assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
        }
        let dest = tempfile::tempdir_in("/tmp").unwrap();
        assert!(unpack(dest.path(), &mut encoded.as_slice(), Uuid::new_v4(), 1024).is_err());
        assert_eq!(fs::read_dir(dest.path()).unwrap().count(), 0);
        let dest = tempfile::tempdir_in("/tmp").unwrap();
        assert!(unpack(dest.path(), &mut encoded.as_slice(), volume, 4).is_err());
        assert_eq!(fs::read(source.path().join("safe")).unwrap(), b"bytes");
    }
}
