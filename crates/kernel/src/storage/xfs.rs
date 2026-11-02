//! XFS project quotas on a trusted, operator-provisioned filesystem.
//!
//! Linux UAPI: linux/dqblk_xfs.h and linux/fs.h. All quota block counts are
//! 512-byte basic blocks, NOT filesystem blocks. quotactl_fd pins the filesystem
//! to an open descriptor instead of re-resolving a device or shell command.
use crate::{
    config::require_host_admin,
    error::{Error, Result},
};
use serde::Serialize;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io, mem,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
};
use uuid::Uuid;

const PROJINHERIT: u32 = 0x200;
const RTINHERIT: u32 = 0x100;
const QUOTA_FLAGS: u16 = (1 << 4) | (1 << 5);
const LIMIT_FIELDS: u16 = 0x3f;
const MAX_BYTES: u64 = 9_007_199_254_740_991;

#[repr(C)]
#[derive(Default)]
struct Xattr {
    flags: u32,
    extent_size: u32,
    extents: u32,
    project: u32,
    cow_extent_size: u32,
    padding: [u8; 8],
}
#[repr(C)]
#[derive(Default)]
struct DiskQuota {
    version: i8,
    flags: i8,
    fields: u16,
    id: u32,
    block_hard: u64,
    block_soft: u64,
    inode_hard: u64,
    inode_soft: u64,
    blocks: u64,
    inodes: u64,
    inode_timer: i32,
    block_timer: i32,
    inode_warns: u16,
    block_warns: u16,
    timer_high: [i8; 4],
    rt_hard: u64,
    rt_soft: u64,
    rt_blocks: u64,
    rt_timer: i32,
    rt_warns: u16,
    padding: i16,
    reserved: [u8; 8],
}
#[repr(C)]
#[derive(Default)]
struct QuotaFile {
    inode: u64,
    blocks: u64,
    extents: u32,
    padding: u32,
}
#[repr(C)]
#[derive(Default)]
struct QuotaState {
    version: i8,
    padding: u8,
    flags: u16,
    cached: u32,
    user: QuotaFile,
    group: QuotaFile,
    project: QuotaFile,
    block_time: i32,
    inode_time: i32,
    rt_time: i32,
    block_warns: u16,
    inode_warns: u16,
    rt_warns: u16,
    padding2: u16,
    padding3: u32,
    reserved: [u64; 7],
}

// Stable V1 geometry is sufficient to reject a separate realtime allocation
// pool, which is accounted independently and must not bypass the byte limit.
#[repr(C)]
#[derive(Default)]
struct Geometry {
    block_size: u32,
    rt_extent_size: u32,
    ag_blocks: u32,
    ag_count: u32,
    log_blocks: u32,
    sector_size: u32,
    inode_size: u32,
    max_inode_percent: u32,
    data_blocks: u64,
    rt_blocks: u64,
    rt_extents: u64,
    log_start: u64,
    uuid: [u8; 16],
    stripe_unit: u32,
    stripe_width: u32,
    version: i32,
    flags: u32,
    log_sector_size: u32,
    rt_sector_size: u32,
    directory_block_size: u32,
}

fn quota<T>(fd: &File, subop: u32, id: u32, data: &mut T) -> io::Result<()> {
    let command = (((b'X' as u32) << 8 | subop) << 8) | 2;
    // SAFETY: each caller supplies the repr(C) UAPI object matching subop;
    // the descriptor and writable object live through this synchronous syscall.
    if unsafe {
        libc::syscall(
            libc::SYS_quotactl_fd,
            fd.as_raw_fd(),
            command,
            id,
            data as *mut T,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn attributes(fd: &File) -> Result<Xattr> {
    let mut attr = Xattr::default();
    // SAFETY: FSGETXATTR writes exactly one initialized, correctly sized Xattr.
    if unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            libc::_IOR::<Xattr>(b'X' as u32, 31),
            &mut attr,
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(attr)
}
fn open_at(parent: &File, name: &CStr) -> io::Result<File> {
    // SAFETY: name is NUL terminated; ownership of a successful new fd is transferred once.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn trusted(fd: &File, private: bool) -> Result<()> {
    let m = fd.metadata()?;
    if !m.is_dir() || m.uid() != 0 || m.mode() & (if private { 0o077 } else { 0o022 }) != 0 {
        return Err(Error::unavailable(
            "Storage requires root-owned protected directories",
        ));
    }
    Ok(())
}
fn trusted_root(path: &Path) -> Result<File> {
    require_host_admin()?;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || path == Path::new("/")
    {
        return Err(Error::invalid(
            "Storage root must be an absolute directory below root",
        ));
    }
    let mut fd = File::open("/")?;
    trusted(&fd, false)?;
    for c in path.components() {
        if let Component::Normal(name) = c {
            let name = CString::new(name.as_bytes())
                .map_err(|_| Error::invalid("Invalid storage directory"))?;
            fd = open_at(&fd, &name)?;
            trusted(&fd, false)?;
        }
    }
    trusted(&fd, true)?;
    Ok(fd)
}

#[derive(Debug, Serialize)]
pub struct BackendStatus {
    pub backend: &'static str,
    pub block_size: u64,
    pub project_accounting: bool,
    pub hard_limits_enforced: bool,
}
fn check_fd(fd: &File) -> Result<BackendStatus> {
    trusted(fd, true)?;
    // SAFETY: fstatfs initializes the correctly sized libc statfs on success.
    let mut stat: libc::statfs = unsafe { mem::zeroed() };
    if unsafe { libc::fstatfs(fd.as_raw_fd(), &mut stat) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if stat.f_type != libc::XFS_SUPER_MAGIC || stat.f_bsize < 512 {
        return Err(Error::unavailable("Storage requires XFS project quota"));
    }
    let mut geometry = Geometry::default();
    // SAFETY: FSGEOMETRY_V1 writes the fixed repr(C) V1 layout.
    if unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            libc::_IOR::<Geometry>(b'X' as u32, 100),
            &mut geometry,
        )
    } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if geometry.rt_blocks != 0 || geometry.block_size as u64 != stat.f_bsize as u64 {
        return Err(Error::unavailable("Realtime XFS storage is not supported"));
    }
    let mut state = QuotaState {
        version: 1,
        ..Default::default()
    };
    quota(fd, 8, 0, &mut state)?;
    if state.version != 1 || state.flags & QUOTA_FLAGS != QUOTA_FLAGS {
        return Err(Error::unavailable(
            "XFS project accounting and hard limit enforcement are required",
        ));
    }
    let attr = attributes(fd)?;
    if attr.project != 0 || attr.flags & (PROJINHERIT | RTINHERIT) != 0 {
        return Err(Error::unavailable(
            "Storage root must not inherit a project or realtime allocation",
        ));
    }
    Ok(BackendStatus {
        backend: "xfs_project",
        block_size: stat.f_bsize as u64,
        project_accounting: true,
        hard_limits_enforced: true,
    })
}
/// Read-only host preflight; never enables quotas, formats or mounts a filesystem.
pub fn check(path: &Path) -> Result<BackendStatus> {
    check_fd(&trusted_root(path)?)
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Limits {
    pub capacity_bytes: u64,
    pub inodes: u64,
}
impl Limits {
    fn validate(self, block_size: u64) -> Result<()> {
        if !(1_048_576..=MAX_BYTES).contains(&self.capacity_bytes)
            || !self.capacity_bytes.is_multiple_of(block_size)
            || self.inodes == 0
            || self.inodes > MAX_BYTES
        {
            return Err(Error::invalid(
                "Backend limits require aligned capacity and a nonzero inode budget",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Serialize)]
pub struct ProjectStatus {
    pub project_id: u32,
    pub limits: Limits,
    pub used_bytes: u64,
    pub used_inodes: u64,
}

/// Exclusive host-side backend handle. IDs must come from a future durable
/// allocator; this adapter never recycles an ID or exposes a directory to OCI.
/// Partial creation is quarantined (root-only directory / reserved quota).
/// The caller must persist intent BEFORE create and recover it before publishing.
pub struct ProjectStore {
    root: File,
    block_size: u64,
}
impl ProjectStore {
    pub fn open(path: &Path) -> Result<Self> {
        let root = trusted_root(path)?;
        // SAFETY: flock holds an advisory exclusive lock until root is dropped.
        if unsafe { libc::flock(root.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(Error::conflict("Storage backend is already in use"));
        }
        let status = check_fd(&root)?;
        Ok(Self {
            root,
            block_size: status.block_size,
        })
    }
    fn validate_id(id: u32) -> Result<()> {
        if id == 0 || id > i32::MAX as u32 {
            return Err(Error::invalid("Invalid project ID"));
        }
        Ok(())
    }
    fn read_quota(&self, id: u32) -> io::Result<DiskQuota> {
        let mut q = DiskQuota::default();
        quota(&self.root, 3, id, &mut q)?;
        Ok(q)
    }
    fn set_limits(&self, id: u32, limits: Limits) -> Result<()> {
        // Soft == hard means no grace-period overcommit. Realtime allocation
        // is not supported; directory inheritance is explicitly disabled.
        let mut q = DiskQuota {
            version: 1,
            flags: 2,
            fields: LIMIT_FIELDS,
            id,
            block_hard: limits.capacity_bytes / 512,
            block_soft: limits.capacity_bytes / 512,
            inode_hard: limits.inodes,
            inode_soft: limits.inodes,
            ..Default::default()
        };
        quota(&self.root, 4, id, &mut q)?;
        Ok(())
    }
    fn dir(&self, volume: Uuid) -> Result<File> {
        let name = CString::new(volume.to_string()).unwrap();
        let dir = open_at(&self.root, &name)?;
        trusted(&dir, true)?;
        if dir.metadata()?.dev() != self.root.metadata()?.dev() {
            return Err(Error::unavailable(
                "Volume directory crosses a filesystem boundary",
            ));
        }
        Ok(dir)
    }
    fn persist(&self) -> Result<()> {
        // SAFETY: syncfs flushes this open filesystem including quota metadata.
        if unsafe { libc::syncfs(self.root.as_raw_fd()) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    /// Create ONLY a new private directory. Existing data is never reassigned.
    pub fn create(&mut self, volume: Uuid, id: u32, limits: Limits) -> Result<ProjectStatus> {
        Self::validate_id(id)?;
        limits.validate(self.block_size)?;
        check_fd(&self.root)?;
        match self.read_quota(id) {
            Ok(q)
                if q.block_hard != 0
                    || q.block_soft != 0
                    || q.inode_hard != 0
                    || q.inode_soft != 0
                    || q.blocks != 0
                    || q.inodes != 0
                    || q.rt_hard != 0
                    || q.rt_soft != 0
                    || q.rt_blocks != 0 =>
            {
                return Err(Error::conflict("Project ID is already reserved"))
            }
            Ok(_) => (),
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => (),
            Err(e) => return Err(e.into()),
        }
        let name = CString::new(volume.to_string()).unwrap();
        // SAFETY: root is trusted and name is generated from a UUID, never user path text.
        if unsafe { libc::mkdirat(self.root.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            return Err(Error::conflict(
                "Volume directory cannot be exclusively created",
            ));
        }
        let dir = self.dir(volume)?;
        self.set_limits(id, limits)?;
        let mut attr = attributes(&dir)?;
        attr.project = id;
        attr.flags = (attr.flags | PROJINHERIT) & !RTINHERIT;
        attr.padding = [0; 8];
        // SAFETY: FSSETXATTR consumes the matching, initialized UAPI object.
        if unsafe { libc::ioctl(dir.as_raw_fd(), libc::_IOW::<Xattr>(b'X' as u32, 32), &attr) } < 0
        {
            return Err(io::Error::last_os_error().into());
        }
        self.persist()?;
        let status = self.inspect(volume, id)?;
        if status.limits != limits {
            return Err(Error::unavailable("Project limits failed verification"));
        }
        Ok(status)
    }
    pub fn inspect(&self, volume: Uuid, id: u32) -> Result<ProjectStatus> {
        Self::validate_id(id)?;
        check_fd(&self.root)?;
        let attr = attributes(&self.dir(volume)?)?;
        if attr.project != id || attr.flags & PROJINHERIT == 0 || attr.flags & RTINHERIT != 0 {
            return Err(Error::unavailable(
                "Volume project identity or inheritance does not match",
            ));
        }
        let q = self.read_quota(id)?;
        if q.version != 1
            || q.flags & 2 == 0
            || q.id != id
            || q.block_hard == 0
            || q.inode_hard == 0
            || q.block_soft != q.block_hard
            || q.inode_soft != q.inode_hard
            || q.rt_hard != 0
            || q.rt_soft != 0
            || q.rt_blocks != 0
        {
            return Err(Error::unavailable(
                "Volume quota is not a supported enforced limit",
            ));
        }
        Ok(ProjectStatus {
            project_id: id,
            limits: Limits {
                capacity_bytes: q.block_hard.checked_mul(512).ok_or_else(Error::exhausted)?,
                inodes: q.inode_hard,
            },
            used_bytes: q.blocks.checked_mul(512).ok_or_else(Error::exhausted)?,
            used_inodes: q.inodes,
        })
    }
    /// Grow an existing project. Host capacity admission and SQL serialization
    /// belong to the future Volume service; callers must reserve budget first.
    pub fn expand(&mut self, volume: Uuid, id: u32, limits: Limits) -> Result<ProjectStatus> {
        limits.validate(self.block_size)?;
        let old = self.inspect(volume, id)?;
        if limits.capacity_bytes <= old.limits.capacity_bytes || limits.inodes < old.limits.inodes {
            return Err(Error::conflict("Volume limits can only grow"));
        }
        self.set_limits(id, limits)?;
        self.persist()?;
        let status = self.inspect(volume, id)?;
        if status.limits != limits {
            return Err(Error::unavailable("Expanded limits failed verification"));
        }
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uapi_layout_and_capacity_boundaries() {
        assert_eq!(mem::size_of::<DiskQuota>(), 112);
        assert_eq!(mem::offset_of!(DiskQuota, rt_hard), 72);
        assert_eq!(mem::size_of::<QuotaState>(), 160);
        assert_eq!(mem::offset_of!(QuotaState, reserved), 104);
        assert_eq!(mem::size_of::<Xattr>(), 28);
        assert_eq!(mem::size_of::<Geometry>(), 112);
        assert_eq!(mem::offset_of!(Geometry, rt_blocks), 40);
        assert!(Limits {
            capacity_bytes: 1_048_576,
            inodes: 256
        }
        .validate(4096)
        .is_ok());
        for n in [0, 1_048_575, 1_048_577, MAX_BYTES, u64::MAX] {
            assert!(Limits {
                capacity_bytes: n,
                inodes: 256
            }
            .validate(4096)
            .is_err());
        }
        assert!(Limits {
            capacity_bytes: 1_048_576,
            inodes: 0
        }
        .validate(4096)
        .is_err());
        assert!(ProjectStore::validate_id(0).is_err());
        assert!(ProjectStore::validate_id(u32::MAX).is_err());
    }
    #[test]
    fn rejects_unsafe_or_non_xfs_roots_without_mutation() {
        for p in ["relative", "/", "/etc/../etc", "/tmp", "/proc"] {
            assert!(check(Path::new(p)).is_err());
        }
        // Default tests need neither mount nor CAP_SYS_ADMIN.
        let d = tempfile::tempdir().unwrap();
        assert!(ProjectStore::open(d.path()).is_err());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }
}
