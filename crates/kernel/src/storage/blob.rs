//! Private immutable objects. Every filename is generated from a UUID and every
//! path component is opened without following links. Only this backend unlinks.
use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path},
    sync::Arc,
};
use uuid::Uuid;
const RESERVE: u64 = 128 * 1024 * 1024;
#[derive(Clone)]
pub struct Store {
    root: Arc<File>,
}
fn component(parent: &File, name: &CString, flags: i32, mode: libc::mode_t) -> Result<File> {
    // SAFETY: the name is terminated and the returned descriptor is owned once.
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
fn protected(file: &File, private: bool) -> Result<()> {
    let m = file.metadata()?;
    let owner = unsafe { libc::geteuid() };
    let sticky = m.uid() == 0 && m.mode() & libc::S_ISVTX != 0;
    if !m.is_dir()
        || (m.uid() != 0 && m.uid() != owner)
        || (!sticky && m.mode() & 0o022 != 0)
        || (private && (m.uid() != owner || m.mode() & 0o077 != 0))
    {
        return Err(Error::unavailable(
            "Blob storage directory is not protected",
        ));
    }
    Ok(())
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        {
            return Err(Error::invalid("Invalid Blob root"));
        }
        let mut root = File::open("/")?;
        for part in path.components() {
            if let Component::Normal(name) = part {
                protected(&root, false)?;
                root = component(
                    &root,
                    &CString::new(name.as_bytes())
                        .map_err(|_| Error::invalid("Invalid Blob root"))?,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                    0,
                )?;
            }
        }
        protected(&root, true)?;
        let child = CString::new("blobs").unwrap();
        // A fresh child is private from its first observable instant.
        if unsafe { libc::mkdirat(root.as_raw_fd(), child.as_ptr(), 0o700) } < 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let root = component(&root, &child, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        protected(&root, true)?;
        Ok(Self {
            root: Arc::new(root),
        })
    }
    pub fn identity(&self, expected: Uuid, creating: bool) -> Result<()> {
        let marker = CString::new(".identity").unwrap();
        let fd = unsafe {
            libc::openat(
                self.root.as_raw_fd(),
                marker.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd >= 0 {
            let mut file = unsafe { File::from_raw_fd(fd) };
            self.valid_file(&file)?;
            let mut bytes = Vec::new();
            (&mut file).take(38).read_to_end(&mut bytes)?;
            if bytes != format!("{expected}\n").as_bytes() {
                return Err(Error::unavailable("Blob storage identity differs"));
            }
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound || !creating {
            return Err(Error::unavailable("Registered Blob storage is unavailable"));
        }
        let temporary = CString::new(format!(".identity-{}", Uuid::new_v4())).unwrap();
        let result = (|| {
            use std::io::Write;
            let mut file = component(
                &self.root,
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            file.write_all(format!("{expected}\n").as_bytes())?;
            file.sync_all()?;
            if unsafe {
                libc::renameat2(
                    self.root.as_raw_fd(),
                    temporary.as_ptr(),
                    self.root.as_raw_fd(),
                    marker.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            self.root.sync_all()?;
            Ok(())
        })();
        let _ = self.unlink(&temporary);
        result
    }
    fn valid_file(&self, file: &File) -> Result<()> {
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o777 != 0o600
            || m.nlink() != 1
            || m.dev() != self.root.metadata()?.dev()
        {
            return Err(Error::unavailable("Blob object identity is invalid"));
        }
        Ok(())
    }
    pub fn stage(&self, size: u64) -> Result<Staged> {
        let name = format!("upload-{}", Uuid::new_v4());
        let c = CString::new(name.clone()).unwrap();
        let file = component(
            &self.root,
            &c,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?;
        let staged = Staged {
            store: self.clone(),
            file: Some(tokio::fs::File::from_std(file)),
            name,
            published: false,
        };
        // A descriptor clone shares the open file description and flock lifetime.
        let fd = staged.file.as_ref().unwrap().as_raw_fd();
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Err(Error::conflict("Blob upload is locked"));
        }
        // A fresh open file description makes this lock coordinate threads and
        // separate processes, even when they share the same Store handle.
        let capacity = component(
            &self.root,
            &CString::new(".").unwrap(),
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?;
        if unsafe { libc::flock(capacity.as_raw_fd(), libc::LOCK_EX) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut space: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatvfs(self.root.as_raw_fd(), &mut space) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let available = (space.f_bavail as u128) * (space.f_frsize as u128);
        if available < (size as u128) + (RESERVE as u128) {
            return Err(Error::exhausted());
        }
        if size > 0 {
            let code = unsafe { libc::posix_fallocate(fd, 0, size as i64) };
            if code != 0 {
                return Err(Error::exhausted());
            }
        }
        if unsafe { libc::fstatvfs(self.root.as_raw_fd(), &mut space) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if (space.f_bavail as u128) * (space.f_frsize as u128) < RESERVE as u128 {
            return Err(Error::exhausted());
        }
        Ok(staged)
    }
    pub fn open_object(&self, id: Uuid) -> Result<File> {
        let file = component(
            &self.root,
            &CString::new(id.to_string()).unwrap(),
            libc::O_RDONLY,
            0,
        )?;
        self.valid_file(&file)?;
        Ok(file)
    }
    pub fn verify(&self, id: Uuid, size: u64, expected: &str) -> Result<File> {
        let mut file = self.open_object(id)?;
        if file.metadata()?.len() != size {
            return Err(Error::unavailable("Blob size differs"));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut length = 0u64;
        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            length += n as u64;
            if length > size {
                return Err(Error::unavailable("Blob size differs"));
            }
            hash.update(&buffer[..n]);
        }
        if length != size || format!("sha256:{:x}", hash.finalize()) != expected {
            return Err(Error::unavailable("Blob digest differs"));
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
    pub fn remove(&self, id: Uuid) -> Result<bool> {
        let name = CString::new(id.to_string()).unwrap();
        let fd = unsafe {
            libc::openat(
                self.root.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(true);
            }
            return Err(e.into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        self.valid_file(&file)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Ok(false);
        }
        self.unlink(&name)?;
        Ok(true)
    }
    fn unlink(&self, name: &CString) -> Result<()> {
        if unsafe { libc::unlinkat(self.root.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        self.root.sync_all()?;
        Ok(())
    }
}
pub struct Staged {
    store: Store,
    pub file: Option<tokio::fs::File>,
    name: String,
    published: bool,
}
impl Staged {
    pub async fn publish(&mut self, id: Uuid) -> Result<()> {
        self.file
            .as_ref()
            .ok_or_else(|| Error::unavailable("Blob upload is closed"))?
            .sync_all()
            .await?;
        let old = CString::new(self.name.clone()).unwrap();
        let new = CString::new(id.to_string()).unwrap();
        if unsafe {
            libc::renameat2(
                self.store.root.as_raw_fd(),
                old.as_ptr(),
                self.store.root.as_raw_fd(),
                new.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // A commit transport failure can mean PostgreSQL committed. Never remove
        // a published file in Drop; reconciliation consults durable metadata.
        self.published = true;
        self.name = id.to_string();
        self.store.root.sync_all()?;
        Ok(())
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        if !self.published {
            let _ = self.store.unlink(&CString::new(self.name.clone()).unwrap());
        }
    }
}

pub struct Orphan {
    pub id: Option<Uuid>,
    store: Store,
    name: CString,
    file: File,
}
impl Store {
    pub fn entries(&self) -> Result<std::fs::ReadDir> {
        Ok(std::fs::read_dir(format!(
            "/proc/self/fd/{}",
            self.root.as_raw_fd()
        ))?)
    }
    pub fn orphan(&self, entry: std::fs::DirEntry) -> Result<Option<Orphan>> {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Ok(None);
        };
        let (id, raw) = if let Some(raw) = name.strip_prefix("upload-") {
            (None, raw)
        } else {
            let Ok(id) = Uuid::parse_str(name) else {
                return Ok(None);
            };
            (Some(id), name)
        };
        let Ok(parsed) = Uuid::parse_str(raw) else {
            return Ok(None);
        };
        if parsed.to_string() != raw {
            return Ok(None);
        }
        let name = CString::new(name).unwrap();
        let file = component(&self.root, &name, libc::O_RDONLY, 0)?;
        self.valid_file(&file)?;
        if file.metadata()?.modified()?.elapsed().unwrap_or_default()
            < std::time::Duration::from_secs(600)
        {
            return Ok(None);
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
            return Ok(None);
        }
        Ok(Some(Orphan {
            id,
            store: self.clone(),
            name,
            file,
        }))
    }
}
impl Orphan {
    pub fn remove(self) -> Result<()> {
        let mut current: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                self.store.root.as_raw_fd(),
                self.name.as_ptr(),
                &mut current,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let held = self.file.metadata()?;
        if current.st_ino != held.ino() || current.st_dev != held.dev() {
            return Err(Error::conflict("Blob cleanup identity changed"));
        }
        self.store.unlink(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn writable_ancestor_cannot_be_hidden_by_a_private_leaf() {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let unsafe_parent = root.path().join("writable");
        let leaf = unsafe_parent.join("private");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::set_permissions(&leaf, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&unsafe_parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Store::open(&leaf).is_err());
    }
    #[tokio::test]
    async fn private_objects_publish_once_and_reject_links_or_corruption() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = Store::open(dir.path()).unwrap();
        let id = Uuid::new_v4();
        let mut staged = store.stage(3).unwrap();
        use tokio::io::AsyncWriteExt;
        staged
            .file
            .as_mut()
            .unwrap()
            .write_all(b"abc")
            .await
            .unwrap();
        staged.publish(id).await.unwrap();
        assert!(store
            .verify(id, 3, &crate::contracts::digest(b"abc"))
            .is_ok());
        assert!(!store.remove(id).unwrap());
        drop(staged);
        assert!(store.remove(id).unwrap());
        assert!(store.remove(id).unwrap());
        let id = Uuid::new_v4();
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("blobs").join(id.to_string()))
            .unwrap();
        assert!(store.open_object(id).is_err());
        std::fs::set_permissions(
            dir.path().join("blobs"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(Store::open(dir.path()).is_err());
    }
    #[tokio::test]
    async fn unpublished_upload_drop_removes_only_its_private_temporary_file() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let store = Store::open(dir.path()).unwrap();
        {
            let _upload = store.stage(0).unwrap();
            assert_eq!(
                std::fs::read_dir(dir.path().join("blobs")).unwrap().count(),
                1
            );
        }
        assert_eq!(
            std::fs::read_dir(dir.path().join("blobs")).unwrap().count(),
            0
        );
    }
}
