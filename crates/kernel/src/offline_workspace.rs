//! Private local object intents. Only fixed names and generated UUIDs are used
//! beneath an already protected root descriptor; writes are fsynced replacements.
use super::*;
use crate::storage::{
    backup_crypto,
    backup_transfer::{self, raw},
};
use std::{
    ffi::CString,
    io::{Read, Write},
    sync::{Arc, Mutex as StdMutex, Weak},
};

const RECORD_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObjectPlan {
    Raw {
        expected: backup_crypto::Summary,
    },
    Volume {
        volume: Uuid,
        maximum_data: u64,
        maximum_archive: u64,
    },
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Preparation {
    Raw(raw::SavedUpload),
    Volume(backup_transfer::SavedUpload),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub snapshot: Uuid,
    pub object: Uuid,
    pub plan: ObjectPlan,
    pub phase: String,
    pub upload: Option<String>,
    pub preparation: Option<Preparation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub format: u32,
    pub snapshot: Uuid,
    pub repository: Uuid,
    pub master_key_reference: String,
    pub phase: String,
    pub context: Value,
}
pub struct Workspace {
    directory: File,
    objects: File,
    gate: StdMutex<()>,
}
fn name(value: &str) -> Result<CString> {
    if value.is_empty() || value.contains('/') || matches!(value, "." | "..") {
        return Err(Error::invalid("Invalid maintenance file name"));
    }
    CString::new(value).map_err(|_| Error::invalid("Invalid maintenance file name"))
}
fn owned(file: &File, directory: bool) -> Result<()> {
    let meta = file.metadata()?;
    if meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
        || meta.is_dir() != directory
        || (!directory && (!meta.is_file() || meta.nlink() != 1))
    {
        return Err(Error::invalid("Maintenance path is not private and owned"));
    }
    Ok(())
}
fn lease(directory: &File) -> Result<()> {
    if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        return Err(Error::conflict("Maintenance workspace is already in use"));
    }
    Ok(())
}
fn phase(value: &str) -> bool {
    matches!(
        value,
        "preparing" | "frozen" | "transferring" | "committing" | "succeeded" | "failed"
    )
}
fn open(parent: &File, child: &str, flags: i32) -> Result<File> {
    let child = name(child)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            child.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn subdirectory(parent: &File, child: &str, new: bool) -> Result<File> {
    let child_c = name(child)?;
    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), child_c.as_ptr(), 0o700) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if new || error.raw_os_error() != Some(libc::EEXIST) {
            return Err(error.into());
        }
    }
    let file = open(parent, child, libc::O_RDONLY | libc::O_DIRECTORY)?;
    owned(&file, true)?;
    parent.sync_all()?;
    Ok(file)
}
fn read<T: serde::de::DeserializeOwned>(parent: &File, child: &str) -> Result<T> {
    let mut file = open(parent, child, libc::O_RDONLY)?;
    owned(&file, false)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(RECORD_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)?;
    let value = strict_json(&bytes, RECORD_LIMIT)?;
    Ok(serde_json::from_value(value)?)
}
fn write<T: Serialize>(parent: &File, child: &str, value: &T, exclusive: bool) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > RECORD_LIMIT {
        return Err(Error::exhausted());
    }
    let temporary = format!(".write-{}", Uuid::new_v4());
    let result = (|| -> Result<()> {
        let mut file = open(
            parent,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        owned(&file, false)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let from = name(&temporary)?;
        let to = name(child)?;
        if unsafe {
            libc::renameat2(
                parent.as_raw_fd(),
                from.as_ptr(),
                parent.as_raw_fd(),
                to.as_ptr(),
                if exclusive { libc::RENAME_NOREPLACE } else { 0 },
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        parent.sync_all()?;
        Ok(())
    })();
    let _ = unsafe { libc::unlinkat(parent.as_raw_fd(), name(&temporary)?.as_ptr(), 0) };
    result
}
impl Workspace {
    pub fn create(root: &File, header: Header) -> Result<Self> {
        owned(root, true)?;
        if header.format != 1
            || header.snapshot.is_nil()
            || header.repository.is_nil()
            || header.phase != "preparing"
            || !valid_digest(&header.master_key_reference)
        {
            return Err(Error::invalid("Invalid maintenance header"));
        }
        let snapshots = subdirectory(root, ".platform-backups", false)?;
        let directory = subdirectory(&snapshots, &header.snapshot.to_string(), true)?;
        lease(&directory)?;
        let objects = subdirectory(&directory, "objects", true)?;
        write(&directory, "snapshot.json", &header, true)?;
        Ok(Self {
            directory,
            objects,
            gate: StdMutex::new(()),
        })
    }
    pub fn resume(root: &File, snapshot: Uuid, repository: Uuid, key: &str) -> Result<Self> {
        owned(root, true)?;
        let snapshots = open(
            root,
            ".platform-backups",
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        owned(&snapshots, true)?;
        let directory = open(
            &snapshots,
            &snapshot.to_string(),
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        owned(&directory, true)?;
        lease(&directory)?;
        let objects = open(&directory, "objects", libc::O_RDONLY | libc::O_DIRECTORY)?;
        owned(&objects, true)?;
        let value = Self {
            directory,
            objects,
            gate: StdMutex::new(()),
        };
        let header = value.header()?;
        if header.format != 1
            || header.snapshot != snapshot
            || header.repository != repository
            || header.master_key_reference != key
        {
            return Err(Error::conflict(
                "Maintenance workspace belongs to a different snapshot or key",
            ));
        }
        Ok(value)
    }
    pub fn header(&self) -> Result<Header> {
        let header: Header = read(&self.directory, "snapshot.json")?;
        if header.format != 1
            || header.snapshot.is_nil()
            || header.repository.is_nil()
            || !valid_digest(&header.master_key_reference)
            || !phase(&header.phase)
        {
            return Err(Error::invalid("Invalid maintenance header"));
        }
        Ok(header)
    }
    pub fn update_header(&self, phase: &str, context: Value) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        if !matches!(
            phase,
            "preparing" | "frozen" | "transferring" | "committing" | "succeeded" | "failed"
        ) {
            return Err(Error::invalid("Invalid maintenance phase"));
        }
        let mut header = self.header()?;
        if matches!(header.phase.as_str(), "succeeded" | "failed") {
            if phase == header.phase && context == header.context {
                return Ok(());
            }
            return Err(Error::conflict("Maintenance session is already terminal"));
        }
        let allowed = phase == header.phase
            || phase == "failed"
            || matches!(
                (header.phase.as_str(), phase),
                ("preparing", "frozen")
                    | ("frozen", "transferring")
                    | ("transferring", "committing")
                    | ("committing", "succeeded")
            );
        if !allowed {
            return Err(Error::conflict(
                "Maintenance phase cannot skip or rewind a checkpoint",
            ));
        }
        header.phase = phase.into();
        header.context = context;
        write(&self.directory, "snapshot.json", &header, false)
    }
    pub fn register(&self, object: Uuid, plan: ObjectPlan) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        if object.is_nil() {
            return Err(Error::invalid("Invalid maintenance object"));
        }
        let header = self.header()?;
        if !matches!(
            header.phase.as_str(),
            "preparing" | "frozen" | "transferring"
        ) {
            return Err(Error::conflict("Maintenance object set is closed"));
        }
        write(
            &self.objects,
            &format!("{object}.json"),
            &Record {
                snapshot: header.snapshot,
                object,
                plan,
                phase: "initializing".into(),
                upload: None,
                preparation: None,
            },
            true,
        )
    }
    pub fn record(&self, object: Uuid) -> Result<Record> {
        let record: Record = read(&self.objects, &format!("{object}.json"))?;
        if record.object != object || record.snapshot != self.header()?.snapshot {
            return Err(Error::conflict("Maintenance object identity differs"));
        }
        Ok(record)
    }
    pub fn upload_started(&self, object: Uuid, upload: &str) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        if upload.is_empty() || upload.len() > 4096 {
            return Err(Error::invalid("Invalid multipart identity"));
        }
        let mut record = self.record(object)?;
        if record.phase != "starting" || record.upload.is_some() {
            return Err(Error::conflict("Maintenance upload was already started"));
        }
        if self.header()?.phase != "transferring" {
            return Err(Error::conflict("Maintenance snapshot is not transferring"));
        }
        record.upload = Some(upload.into());
        record.phase = "uploading".into();
        write(&self.objects, &format!("{object}.json"), &record, false)
    }
    pub fn upload_starting(&self, object: Uuid) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        let mut record = self.record(object)?;
        if self.header()?.phase != "transferring" || record.phase != "initializing" {
            return Err(Error::conflict("Maintenance upload cannot be initiated"));
        }
        // Persist before S3: an initiation with a lost response must be found
        // later by its exact owned key, even though no upload ID was returned.
        record.phase = "starting".into();
        write(&self.objects, &format!("{object}.json"), &record, false)
    }
    fn prepared(
        &self,
        object: Uuid,
        upload: &str,
        plan: &ObjectPlan,
        preparation: Preparation,
    ) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        if self.header()?.phase != "transferring" {
            return Err(Error::conflict(
                "Maintenance snapshot no longer accepts preparations",
            ));
        }
        let mut record = self.record(object)?;
        if record.phase != "uploading"
            || record.upload.as_deref() != Some(upload)
            || record.plan != *plan
        {
            return Err(Error::conflict(
                "Maintenance preparation differs from its durable intent",
            ));
        }
        record.preparation = Some(preparation);
        record.phase = "prepared".into();
        write(&self.objects, &format!("{object}.json"), &record, false)
    }
    pub fn verified(
        &self,
        object: Uuid,
        plain: &backup_crypto::Summary,
        cipher: &backup_crypto::Summary,
    ) -> Result<()> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| Error::unavailable("Maintenance journal is unavailable"))?;
        let mut record = self.record(object)?;
        let matches = match &record.preparation {
            Some(Preparation::Raw(saved)) => {
                &saved.plaintext == plain && &saved.ciphertext == cipher
            }
            Some(Preparation::Volume(saved)) => {
                &saved.plaintext == plain && &saved.ciphertext == cipher
            }
            None => false,
        };
        if !matches || !matches!(record.phase.as_str(), "prepared" | "verified") {
            return Err(Error::conflict(
                "Verified maintenance bytes differ from their preparation",
            ));
        }
        record.phase = "verified".into();
        write(&self.objects, &format!("{object}.json"), &record, false)
    }
    pub fn stage_file(&self, images: bool) -> Result<File> {
        open(
            &self.directory,
            if images { "images.oci" } else { "metadata.db" },
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
    }
    pub fn staged_file(&self, images: bool) -> Result<File> {
        let file = open(
            &self.directory,
            if images { "images.oci" } else { "metadata.db" },
            libc::O_RDONLY,
        )?;
        owned(&file, false)?;
        Ok(file)
    }
    pub fn reserve_file(&self, images: bool, bytes: u64, headroom: u64) -> Result<File> {
        if bytes > 64 * 1024 * 1024 * 1024 {
            return Err(Error::exhausted());
        }
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatvfs(self.directory.as_raw_fd(), &mut stat) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let free = (stat.f_bavail as u64)
            .checked_mul(stat.f_frsize as u64)
            .ok_or_else(Error::exhausted)?;
        if bytes.checked_add(headroom).is_none_or(|n| n > free) {
            return Err(Error::exhausted());
        }
        let file = self.stage_file(images)?;
        if bytes > 0
            && unsafe {
                libc::fallocate(file.as_raw_fd(), libc::FALLOC_FL_KEEP_SIZE, 0, bytes as i64)
            } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        file.sync_all()?;
        Ok(file)
    }
    pub fn records(&self) -> Result<Vec<Record>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(format!("/proc/self/fd/{}", self.objects.as_raw_fd()))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| Error::invalid("Invalid maintenance object name"))?;
            if name.starts_with(".write-") {
                continue;
            }
            if result.len() >= 65556 {
                return Err(Error::exhausted());
            }
            let id: Uuid = name
                .strip_suffix(".json")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| Error::invalid("Unexpected maintenance object name"))?;
            result.push(self.record(id)?);
        }
        result.sort_by_key(|r| r.object);
        Ok(result)
    }
    pub fn remove_staged_data(&self) -> Result<()> {
        if self.header()?.phase != "succeeded" {
            return Err(Error::conflict(
                "Unfinished maintenance data must be retained",
            ));
        }
        for file in ["metadata.db", "images.oci"] {
            let file = name(file)?;
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), file.as_ptr(), 0) } < 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT)
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        self.directory.sync_all()?;
        Ok(())
    }
}
pub struct ObjectJournal {
    pub workspace: Arc<Workspace>,
    pub session: Weak<Session>,
}
impl ObjectJournal {
    fn lease(&self) -> Result<Arc<Session>> {
        let session = self
            .session
            .upgrade()
            .ok_or_else(|| Error::conflict("Maintenance owner has ended"))?;
        session.check_leader()?;
        Ok(session)
    }
}
#[tonic::async_trait]
impl raw::Journal for ObjectJournal {
    async fn prepared(&self, object: raw::Object, saved: &raw::SavedUpload) -> Result<()> {
        let session = self.lease()?;
        let header = self.workspace.header()?;
        if object.backup != header.snapshot || saved.plaintext.bytes > object.maximum_bytes {
            return Err(Error::conflict("Raw preparation identity differs"));
        }
        let record = self.workspace.record(object.object)?;
        let ObjectPlan::Raw { expected } = &record.plan else {
            return Err(Error::conflict("Expected a raw maintenance object"));
        };
        if expected != &saved.plaintext {
            return Err(Error::conflict(
                "Raw maintenance digest differs from source metadata",
            ));
        }
        self.workspace.prepared(
            object.object,
            &saved.id,
            &record.plan,
            Preparation::Raw(saved.clone()),
        )?;
        session.check_leader()
    }
}
#[tonic::async_trait]
impl backup_transfer::Journal for ObjectJournal {
    async fn prepared(
        &self,
        object: backup_transfer::Object,
        upload: &str,
        etags: &[String],
        plaintext: &backup_crypto::Summary,
        ciphertext: &backup_crypto::Summary,
        archive: &crate::storage::archive::Summary,
    ) -> Result<()> {
        let session = self.lease()?;
        let header = self.workspace.header()?;
        if object.backup != header.snapshot
            || plaintext.bytes > object.maximum_archive
            || archive.data_bytes > object.maximum_data
        {
            return Err(Error::conflict(
                "Volume preparation identity or size differs",
            ));
        }
        let plan = ObjectPlan::Volume {
            volume: object.volume,
            maximum_data: object.maximum_data,
            maximum_archive: object.maximum_archive,
        };
        self.workspace.prepared(
            object.object,
            upload,
            &plan,
            Preparation::Volume(backup_transfer::SavedUpload {
                id: upload.into(),
                etags: etags.to_vec(),
                plaintext: plaintext.clone(),
                ciphertext: ciphertext.clone(),
                archive: archive.clone(),
            }),
        )?;
        session.check_leader()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    fn private_root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap()
    }
    fn header() -> Header {
        Header {
            format: 1,
            snapshot: Uuid::new_v4(),
            repository: Uuid::new_v4(),
            master_key_reference: digest(b"test-only master"),
            phase: "preparing".into(),
            context: json!({"objects":1}),
        }
    }
    fn sum(data: &[u8]) -> backup_crypto::Summary {
        backup_crypto::Summary {
            bytes: data.len() as u64,
            digest: digest(data),
        }
    }
    #[test]
    fn actual_private_journal_recovers_exact_preparations_without_overwriting_other_sessions() {
        let root = private_root();
        let fd = File::open(root.path()).unwrap();
        let header = header();
        let workspace = Workspace::create(&fd, header.clone()).unwrap();
        assert!(Workspace::create(&fd, header.clone()).is_err());
        assert!(Workspace::resume(
            &fd,
            header.snapshot,
            header.repository,
            &header.master_key_reference
        )
        .is_err());
        let object = Uuid::new_v4();
        let original = sum(b"original bytes");
        let cipher = sum(b"encrypted fake fixture");
        let plan = ObjectPlan::Raw {
            expected: original.clone(),
        };
        workspace.register(object, plan.clone()).unwrap();
        assert!(workspace.register(object, plan.clone()).is_err());
        assert!(workspace.upload_started(object, "saved-upload").is_err());
        assert!(workspace
            .update_header("committing", header.context.clone())
            .is_err());
        workspace
            .update_header("frozen", header.context.clone())
            .unwrap();
        workspace
            .update_header("transferring", header.context.clone())
            .unwrap();
        workspace.upload_starting(object).unwrap();
        workspace.upload_started(object, "saved-upload").unwrap();
        let saved = Preparation::Raw(raw::SavedUpload {
            id: "saved-upload".into(),
            etags: vec!["fake-test-etag".into()],
            plaintext: original.clone(),
            ciphertext: cipher.clone(),
        });
        assert!(workspace
            .prepared(object, "foreign-upload", &plan, saved.clone())
            .is_err());
        assert!(workspace
            .prepared(
                object,
                "saved-upload",
                &ObjectPlan::Raw {
                    expected: sum(b"other")
                },
                saved.clone()
            )
            .is_err());
        workspace
            .prepared(object, "saved-upload", &plan, saved)
            .unwrap();
        assert!(workspace.verified(object, &sum(b"other"), &cipher).is_err());
        workspace.verified(object, &original, &cipher).unwrap();
        workspace
            .update_header("committing", json!({"manifest":"test-proof"}))
            .unwrap();
        workspace
            .update_header("succeeded", json!({"manifest":"test-proof"}))
            .unwrap();
        assert!(workspace.update_header("transferring", json!({})).is_err());
        assert!(workspace
            .update_header("succeeded", json!({"manifest":"different"}))
            .is_err());
        drop(workspace);
        let reopened = Workspace::resume(
            &fd,
            header.snapshot,
            header.repository,
            &header.master_key_reference,
        )
        .unwrap();
        assert_eq!(reopened.record(object).unwrap().phase, "verified");
        assert_eq!(reopened.header().unwrap().phase, "succeeded");
        drop(reopened);
        assert!(Workspace::resume(
            &fd,
            header.snapshot,
            Uuid::new_v4(),
            &header.master_key_reference
        )
        .is_err());
        assert!(Workspace::resume(
            &fd,
            header.snapshot,
            header.repository,
            &digest(b"wrong key")
        )
        .is_err());
    }
    #[test]
    fn private_journal_refuses_symlinks_and_torn_json() {
        let root = private_root();
        let other = private_root();
        let fd = File::open(root.path()).unwrap();
        std::os::unix::fs::symlink(other.path(), root.path().join(".platform-backups")).unwrap();
        assert!(Workspace::create(&fd, header()).is_err());
        assert_eq!(std::fs::read_dir(other.path()).unwrap().count(), 0);
        std::fs::remove_file(root.path().join(".platform-backups")).unwrap();
        let header = header();
        let workspace = Workspace::create(&fd, header.clone()).unwrap();
        let object = Uuid::new_v4();
        workspace
            .register(
                object,
                ObjectPlan::Raw {
                    expected: sum(b"raw"),
                },
            )
            .unwrap();
        let path = root
            .path()
            .join(".platform-backups")
            .join(header.snapshot.to_string())
            .join("objects")
            .join(format!("{object}.json"));
        std::fs::write(path, b"{\"snapshot\":").unwrap();
        assert!(workspace.record(object).is_err());
    }
}
