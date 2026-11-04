//! Raw immutable file objects for Blob, metadata and OCI archives. The expected
//! plaintext digest comes from trusted metadata, and is checked before complete.
//! All producers settle before journal/complete; targets stay unpublished.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, Metadata},
    io::{Seek, SeekFrom},
    os::unix::fs::MetadataExt,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub backup: Uuid,
    pub object: Uuid,
    pub maximum_bytes: u64,
}
impl Object {
    pub fn key(&self) -> String {
        format!("{}/{}.sealed", self.backup, self.object)
    }
    fn validate(&self) -> Result<()> {
        if self.backup.is_nil() || self.object.is_nil() || self.maximum_bytes > MAX_ARCHIVE {
            return Err(Error::invalid("Invalid raw backup object"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: backup_crypto::Summary,
    pub off_host: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedUpload {
    pub id: String,
    pub etags: Vec<String>,
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: backup_crypto::Summary,
}
pub struct Source {
    file: File,
    expected: backup_crypto::Summary,
    metadata: Metadata,
}
impl Source {
    pub fn new(file: File, expected: backup_crypto::Summary) -> Result<Self> {
        check_summary(&expected, MAX_ARCHIVE)?;
        let metadata = file.metadata()?;
        private(&metadata)?;
        if metadata.len() != expected.bytes {
            return Err(Error::conflict(
                "Raw backup file length differs from trusted metadata",
            ));
        }
        Ok(Self {
            file,
            expected,
            metadata,
        })
    }
}
pub struct Target {
    file: File,
}
impl Target {
    /// File must be privately staged and unpublished. Failure may leave partial
    /// plaintext, so the coordinator must quarantine it before any publication.
    pub fn new(file: File) -> Result<Self> {
        let metadata = file.metadata()?;
        private(&metadata)?;
        if metadata.len() != 0 {
            return Err(Error::conflict("Raw restore target must be empty"));
        }
        Ok(Self { file })
    }
}
pub struct Restored {
    pub file: File,
    pub verified: Verified,
}
#[derive(Clone)]
pub struct Expected {
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: Option<backup_crypto::Summary>,
}
#[tonic::async_trait]
pub trait Journal: Send + Sync {
    /// Persist and validate the saved preparation against its source metadata
    /// before S3 completion. Failure prevents publication of the object.
    async fn prepared(&self, object: Object, saved: &SavedUpload) -> Result<()>;
}
#[derive(Clone)]
pub struct RawPool {
    pool: Pool,
    journal: Option<Arc<dyn Journal>>,
}
impl Pool {
    /// Raw objects and archive objects share this pool's two transfer slots.
    pub fn raw(&self, journal: Option<Arc<dyn Journal>>) -> RawPool {
        RawPool {
            pool: self.clone(),
            journal,
        }
    }
}
fn private(meta: &Metadata) -> Result<()> {
    let owner = unsafe { libc::geteuid() };
    if !meta.is_file()
        || (meta.uid() != 0 && meta.uid() != owner)
        || meta.mode() & 0o077 != 0
        || meta.nlink() != 1
    {
        return Err(Error::invalid(
            "Raw transfer requires an owned private single-link regular file",
        ));
    }
    Ok(())
}
fn unchanged(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.uid() == b.uid()
        && a.gid() == b.gid()
        && a.mode() == b.mode()
        && a.nlink() == b.nlink()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
fn check_summary(value: &backup_crypto::Summary, maximum: u64) -> Result<()> {
    let s = &value.digest;
    if value.bytes > maximum
        || s.len() != 71
        || !s.starts_with("sha256:")
        || !s[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid("Invalid raw transfer digest or length"));
    }
    Ok(())
}
impl RawPool {
    pub fn upload_file(
        &self,
        client: Arc<s3::Client>,
        source: Source,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        upload: String,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        check_summary(&source.expected, object.maximum_bytes)?;
        if upload.is_empty() || upload.len() > 4096 {
            return Err(Error::invalid("Invalid saved multipart identity"));
        }
        let end = deadline(timeout)?;
        let permit = self
            .pool
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        let journal = self.journal.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let value = upload_inner(client, source, object, master, &upload, end, journal).await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
    /// Saved preparation must also match its original source metadata in the
    /// coordinator. Recovery never reopens a mutable source file.
    pub fn recover(
        &self,
        client: Arc<s3::Client>,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        saved: SavedUpload,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        check_summary(&saved.plaintext, object.maximum_bytes)?;
        check_summary(&saved.ciphertext, MAX_CIPHER)?;
        if saved.id.is_empty()
            || saved.id.len() > 4096
            || saved.etags.is_empty()
            || saved.etags.len() > 10000
            || saved
                .etags
                .iter()
                .any(|s| s.is_empty() || s.len() > 256 || !s.is_ascii())
        {
            return Err(Error::invalid("Invalid saved raw preparation"));
        }
        let end = deadline(timeout)?;
        let permit = self
            .pool
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let value = finalize(&client, object, &master, saved, end).await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
    pub fn download_file(
        &self,
        client: Arc<s3::Client>,
        target: Target,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        expected: Expected,
        timeout: Duration,
    ) -> Result<Transfer<Restored>> {
        object.validate()?;
        check_summary(&expected.plaintext, object.maximum_bytes)?;
        if let Some(cipher) = &expected.ciphertext {
            check_summary(cipher, MAX_CIPHER)?;
        }
        let end = deadline(timeout)?;
        let permit = self
            .pool
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let value = restore_inner(&client, target, object, master, expected, end).await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
}
async fn upload_inner(
    client: Arc<s3::Client>,
    mut source: Source,
    object: Object,
    master: Zeroizing<[u8; 32]>,
    upload: &str,
    end: Instant,
    journal: Option<Arc<dyn Journal>>,
) -> Result<Verified> {
    let canceled = Arc::new(AtomicBool::new(false));
    let (writer, reader) = UnixStream::pair()?;
    let mut writer = Pipe::new(writer, end, canceled.clone())?;
    let reader = Pipe::new(reader, end, canceled.clone())?;
    let expected = source.expected.clone();
    let produce = tokio::task::spawn_blocking(move || -> Result<()> {
        let before = source.file.metadata()?;
        private(&before)?;
        if !unchanged(&source.metadata, &before) {
            return Err(Error::conflict("Raw source changed before reading"));
        }
        source.file.seek(SeekFrom::Start(0))?;
        let bytes = std::io::copy(
            &mut (&mut source.file).take(object.maximum_bytes.saturating_add(1)),
            &mut writer,
        )?;
        if bytes != source.expected.bytes || !unchanged(&before, &source.file.metadata()?) {
            return Err(Error::conflict("Raw source changed while reading"));
        }
        Ok(())
    });
    let (sender, mut receiver) = mpsc::channel(1);
    let key_copy = Zeroizing::new(*master);
    let cancel = canceled.clone();
    let encrypt = tokio::task::spawn_blocking(move || {
        let mut parts = Parts {
            buffer: Vec::with_capacity(PART),
            sender,
            hash: Sha256::new(),
            bytes: 0,
            end,
            canceled: cancel,
        };
        let plaintext = backup_crypto::seal(
            reader,
            &mut parts,
            &key_copy,
            object.backup,
            object.object,
            object.maximum_bytes,
        )?;
        Ok((plaintext, parts.finish()?))
    });
    let key = object.key();
    let network = within(end, async {
        let mut etags = Vec::new();
        while let Some(part) = receiver.recv().await {
            if etags.len() >= 10000 {
                return Err(Error::exhausted());
            }
            etags.push(
                client
                    .part(&key, upload, etags.len() as u32 + 1, part)
                    .await?,
            );
        }
        Ok(etags)
    })
    .await;
    drop(receiver);
    if network.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let encrypted = joined(encrypt).await;
    if encrypted.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let produced = joined(produce).await;
    let prepared = network.and_then(|etags| {
        produced?;
        let (plaintext, ciphertext) = encrypted?;
        if plaintext != expected {
            return Err(Error::conflict(
                "Raw content does not match trusted source digest",
            ));
        }
        Ok(SavedUpload {
            id: upload.to_owned(),
            etags,
            plaintext,
            ciphertext,
        })
    });
    let result = async {
        let saved = prepared?;
        if let Some(journal) = journal {
            within(end, journal.prepared(object, &saved)).await?;
        }
        finalize(&client, object, &master, saved, end).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(5), client.abort(&key, upload)).await;
    }
    result
}
async fn finalize(
    client: &s3::Client,
    object: Object,
    master: &[u8; 32],
    saved: SavedUpload,
    end: Instant,
) -> Result<Verified> {
    let completion = within(end, client.complete(&object.key(), &saved.id, &saved.etags)).await;
    let actual = super::verify_bytes(
        client,
        object.backup,
        object.object,
        object.maximum_bytes,
        master,
        &saved.ciphertext,
        end,
    )
    .await;
    if actual.as_ref().is_ok_and(|plain| *plain == saved.plaintext) {
        return Ok(Verified {
            plaintext: saved.plaintext,
            ciphertext: saved.ciphertext,
            off_host: client.off_host(),
        });
    }
    Err(completion.err().unwrap_or_else(stopped))
}
async fn restore_inner(
    client: &s3::Client,
    mut target: Target,
    object: Object,
    master: Zeroizing<[u8; 32]>,
    expected: Expected,
    end: Instant,
) -> Result<Restored> {
    let canceled = Arc::new(AtomicBool::new(false));
    let (writer, reader) = download_pipe(end, canceled.clone())?;
    let decrypt = tokio::task::spawn_blocking(move || -> Result<_> {
        let before = target.file.metadata()?;
        private(&before)?;
        if before.len() != 0 {
            return Err(Error::conflict("Raw target changed before restore"));
        }
        target.file.seek(SeekFrom::Start(0))?;
        let plain = backup_crypto::open(
            reader,
            &mut target.file,
            &master,
            object.backup,
            object.object,
            object.maximum_bytes,
        )?;
        target.file.sync_all()?;
        let written = target.file.metadata()?;
        private(&written)?;
        if written.len() != plain.bytes {
            return Err(Error::conflict("Raw target length changed"));
        }
        target.file.seek(SeekFrom::Start(0))?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 65536];
        loop {
            let n = target.file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            bytes = bytes
                .checked_add(n as u64)
                .filter(|n| *n <= plain.bytes)
                .ok_or_else(Error::exhausted)?;
            hash.update(&buffer[..n]);
        }
        if summary(bytes, hash) != plain || !unchanged(&written, &target.file.metadata()?) {
            return Err(Error::conflict(
                "Raw restored file does not match authenticated content",
            ));
        }
        target.file.seek(SeekFrom::Start(0))?;
        Ok((target.file, plain))
    });
    let downloaded = super::download(client, &object.key(), writer, end).await;
    if downloaded.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let decrypted = joined(decrypt).await;
    let ciphertext = downloaded?;
    let (file, plaintext) = decrypted?;
    if plaintext != expected.plaintext
        || expected
            .ciphertext
            .as_ref()
            .is_some_and(|c| *c != ciphertext)
    {
        return Err(stopped());
    }
    Ok(Restored {
        file,
        verified: Verified {
            plaintext,
            ciphertext,
            off_host: client.off_host(),
        },
    })
}

#[cfg(test)]
#[path = "backup_raw_tests.rs"]
mod tests;
