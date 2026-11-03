//! Bounded archive -> encryption -> S3 transfers. A caller must durably record
//! the multipart identity before starting. Dropping a waiter never cancels the
//! owned supervisor: it joins all filesystem workers before releasing capacity.
//! This module neither stops writers nor publishes restored volumes.
use super::{archive, backup_crypto, s3};
use crate::error::{Error, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, oneshot, Semaphore},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const PART: usize = 8 * 1024 * 1024;
const MAX_ARCHIVE: u64 = 64 * 1024 * 1024 * 1024;
const MAX_CIPHER: u64 = MAX_ARCHIVE + MAX_ARCHIVE / 65536 * 20 + 1024;

#[path = "backup_raw.rs"]
pub mod raw;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Object {
    pub backup: Uuid,
    pub object: Uuid,
    pub volume: Uuid,
    pub maximum_data: u64,
    pub maximum_archive: u64,
}
impl Object {
    pub fn key(&self) -> String {
        format!("{}/{}.sealed", self.backup, self.object)
    }
    fn validate(&self) -> Result<()> {
        if self.maximum_archive == 0
            || self.maximum_archive > MAX_ARCHIVE
            || self.maximum_data > self.maximum_archive
        {
            return Err(Error::invalid(
                "Backup transfer exceeds its supported bound",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: backup_crypto::Summary,
    pub archive: archive::Summary,
    pub off_host: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedUpload {
    pub id: String,
    pub etags: Vec<String>,
    pub plaintext: backup_crypto::Summary,
    pub ciphertext: backup_crypto::Summary,
    pub archive: archive::Summary,
}
pub struct Transfer<T> {
    result: oneshot::Receiver<Result<T>>,
}
impl<T> Transfer<T> {
    pub async fn wait(self) -> Result<T> {
        self.result
            .await
            .map_err(|_| Error::unavailable("Backup worker stopped"))?
    }
}
#[derive(Clone)]
pub struct Pool {
    permits: Arc<Semaphore>,
    journal: Option<Arc<dyn Journal>>,
}
#[tonic::async_trait]
pub trait Journal: Send + Sync {
    /// Persist exact hashes and multipart identities before completion. Returning
    /// an error prevents completion, including when authorization was withdrawn.
    async fn prepared(
        &self,
        object: Object,
        upload: &str,
        etags: &[String],
        plaintext: &backup_crypto::Summary,
        ciphertext: &backup_crypto::Summary,
        archive: &archive::Summary,
    ) -> Result<()>;
}
impl Default for Pool {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
            journal: None,
        }
    }
}
impl Pool {
    pub fn with_journal(journal: Arc<dyn Journal>) -> Self {
        Self {
            journal: Some(journal),
            ..Self::default()
        }
    }
    /// Resume only a fully persisted preparation. Completion may already have
    /// succeeded; exact GET+AEAD comparison remains mandatory in either case.
    pub fn recover(
        &self,
        client: Arc<s3::Client>,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        saved: SavedUpload,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        let end = deadline(timeout)?;
        if saved.id.is_empty()
            || saved.id.len() > 4096
            || saved.etags.is_empty()
            || saved.etags.len() > 10000
            || saved
                .etags
                .iter()
                .any(|v| v.is_empty() || v.len() > 256 || !v.is_ascii())
            || saved.plaintext.bytes > object.maximum_archive
            || saved.ciphertext.bytes > MAX_CIPHER
            || saved.archive.data_bytes > object.maximum_data
            || saved.archive.entries > 65536
        {
            return Err(Error::invalid("Invalid saved backup preparation"));
        }
        let permit = self
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
    /// `upload` must already be saved in the durable job. Sources must remain
    /// stopped and fenced until wait completes, including after HTTP cancellation.
    pub fn upload(
        &self,
        client: Arc<s3::Client>,
        source: PathBuf,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        upload: String,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        let deadline = deadline(timeout)?;
        if upload.is_empty() || upload.len() > 4096 {
            return Err(Error::invalid("Invalid saved multipart identity"));
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        let journal = self.journal.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let value =
                upload_inner(client, source, object, master, &upload, deadline, journal).await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
    /// `target` must be a private, new, unpublished volume. Failure can leave
    /// partial authenticated files; its owner must quarantine or remove them.
    pub fn restore(
        &self,
        client: Arc<s3::Client>,
        target: PathBuf,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        expected: Verified,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        let deadline = deadline(timeout)?;
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let value = restore_inner(client, target, object, master, expected, deadline).await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
    /// Recheck a previously verified private allocation after daemon restart.
    /// It is never unpacked over an existing tree or accepted from metadata alone.
    pub fn verify_restored(
        &self,
        client: Arc<s3::Client>,
        target: PathBuf,
        object: Object,
        master: Zeroizing<[u8; 32]>,
        expected: Verified,
        timeout: Duration,
    ) -> Result<Transfer<Verified>> {
        object.validate()?;
        let end = deadline(timeout)?;
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let value = async {
                let actual = target_pass(&client, target, object, &master, end, true).await?;
                if !same_data(&actual, &expected) {
                    return Err(stopped());
                }
                Ok(actual)
            }
            .await;
            let _ = sender.send(value);
        });
        Ok(Transfer { result })
    }
}
fn deadline(timeout: Duration) -> Result<Instant> {
    if timeout.is_zero() || timeout > Duration::from_secs(3600) {
        return Err(Error::invalid("Invalid backup transfer deadline"));
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(Error::exhausted)
}
fn stopped() -> Error {
    Error::unavailable("Backup transfer did not complete")
}
async fn within<T>(
    end: Instant,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout_at(end.into(), future)
        .await
        .map_err(|_| stopped())?
}

struct Pipe {
    stream: UnixStream,
    end: Instant,
    canceled: Arc<AtomicBool>,
}
impl Pipe {
    fn new(stream: UnixStream, end: Instant, canceled: Arc<AtomicBool>) -> Result<Self> {
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        stream.set_write_timeout(Some(Duration::from_millis(250)))?;
        Ok(Self {
            stream,
            end,
            canceled,
        })
    }
    fn check(&self) -> std::io::Result<()> {
        if self.canceled.load(Ordering::Acquire) || Instant::now() >= self.end {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Backup transfer stopped",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for Pipe {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.read(output) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
}
impl Write for Pipe {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.stream.write(input) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()
    }
}

struct Parts {
    buffer: Vec<u8>,
    sender: mpsc::Sender<Vec<u8>>,
    hash: Sha256,
    bytes: u64,
    end: Instant,
    canceled: Arc<AtomicBool>,
}
impl Parts {
    fn emit(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            let buffer = std::mem::replace(&mut self.buffer, Vec::with_capacity(PART));
            self.sender.blocking_send(buffer).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Backup upload stopped")
            })?;
        }
        Ok(())
    }
    fn finish(mut self) -> Result<backup_crypto::Summary> {
        self.emit()?;
        Ok(summary(self.bytes, self.hash))
    }
}
impl Write for Parts {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if self.canceled.load(Ordering::Acquire)
            || Instant::now() >= self.end
            || self
                .bytes
                .checked_add(input.len() as u64)
                .is_none_or(|n| n > MAX_CIPHER)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Backup upload stopped",
            ));
        }
        self.hash.update(input);
        self.bytes += input.len() as u64;
        let mut remaining = input;
        while !remaining.is_empty() {
            let n = remaining.len().min(PART - self.buffer.len());
            self.buffer.extend_from_slice(&remaining[..n]);
            remaining = &remaining[n..];
            if self.buffer.len() == PART {
                self.emit()?;
            }
        }
        Ok(input.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn summary(bytes: u64, hash: Sha256) -> backup_crypto::Summary {
    backup_crypto::Summary {
        bytes,
        digest: format!("sha256:{:x}", hash.finalize()),
    }
}
async fn joined<T>(worker: tokio::task::JoinHandle<Result<T>>) -> Result<T> {
    worker.await.map_err(|_| stopped())?
}
async fn upload_inner(
    client: Arc<s3::Client>,
    source: PathBuf,
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
    let pack = tokio::task::spawn_blocking(move || {
        archive::pack(&source, &mut writer, object.volume, object.maximum_data)
    });
    let (sender, mut receiver) = mpsc::channel(1);
    let cancel = canceled.clone();
    let key_copy = Zeroizing::new(*master);
    let encrypt = tokio::task::spawn_blocking(move || {
        let mut parts = Parts {
            buffer: Vec::with_capacity(PART),
            sender,
            hash: Sha256::new(),
            bytes: 0,
            end,
            canceled: cancel,
        };
        let plain = backup_crypto::seal(
            reader,
            &mut parts,
            &key_copy,
            object.backup,
            object.object,
            object.maximum_archive,
        )?;
        Ok((plain, parts.finish()?))
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
    // Both results are mandatory. A failed archiver may have closed its pipe,
    // allowing encryption to authenticate an incomplete archive successfully.
    let encryption = joined(encrypt).await;
    if encryption.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let archive = joined(pack).await;
    let prepared = network.and_then(|parts| Ok((parts, encryption?, archive?)));
    let (parts, (plaintext, ciphertext), archive) = match prepared {
        Ok(value) => value,
        Err(error) => {
            // Abort is best effort, not proof of cleanup; the durable job keeps
            // the saved upload ID for reconciliation after this result.
            let _ = tokio::time::timeout(Duration::from_secs(5), client.abort(&key, upload)).await;
            return Err(error);
        }
    };
    if let Some(journal) = journal {
        if let Err(error) = within(
            end,
            journal.prepared(object, upload, &parts, &plaintext, &ciphertext, &archive),
        )
        .await
        {
            let _ = tokio::time::timeout(Duration::from_secs(5), client.abort(&key, upload)).await;
            return Err(error);
        }
    }
    finalize(
        &client,
        object,
        &master,
        SavedUpload {
            id: upload.to_owned(),
            etags: parts,
            plaintext,
            ciphertext,
            archive,
        },
        end,
    )
    .await
}
async fn finalize(
    client: &s3::Client,
    object: Object,
    master: &[u8; 32],
    saved: SavedUpload,
    end: Instant,
) -> Result<Verified> {
    let key = object.key();
    let completion = within(end, client.complete(&key, &saved.id, &saved.etags)).await;
    // Completion errors are ambiguous. Only exact GET+AEAD verification can
    // recover one; no overwrite or deletion of a possibly published object.
    let verify = verify_remote(client, object, master, &saved.ciphertext, end).await;
    let verified = match verify {
        Ok(value) if value == saved.plaintext => value,
        _ => return Err(completion.err().unwrap_or_else(stopped)),
    };
    Ok(Verified {
        plaintext: verified,
        ciphertext: saved.ciphertext,
        archive: saved.archive,
        off_host: client.off_host(),
    })
}

fn download_pipe(
    end: Instant,
    canceled: Arc<AtomicBool>,
) -> Result<(tokio::net::UnixStream, Pipe)> {
    let (writer, reader) = UnixStream::pair()?;
    let reader = Pipe::new(reader, end, canceled)?;
    writer.set_nonblocking(true)?;
    Ok((tokio::net::UnixStream::from_std(writer)?, reader))
}
async fn download(
    client: &s3::Client,
    key: &str,
    mut writer: tokio::net::UnixStream,
    end: Instant,
) -> Result<backup_crypto::Summary> {
    within(end, async {
        let response = client.get(key).await?;
        if response.content_length().is_some_and(|n| n > MAX_CIPHER) {
            return Err(Error::exhausted());
        }
        let mut input = response.bytes_stream();
        let mut bytes = 0u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = input.next().await {
            let chunk = chunk.map_err(|_| stopped())?;
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .filter(|n| *n <= MAX_CIPHER)
                .ok_or_else(Error::exhausted)?;
            hash.update(&chunk);
            writer.write_all(&chunk).await?;
        }
        writer.shutdown().await?;
        Ok(summary(bytes, hash))
    })
    .await
}
async fn verify_remote(
    client: &s3::Client,
    object: Object,
    master: &[u8; 32],
    expected: &backup_crypto::Summary,
    end: Instant,
) -> Result<backup_crypto::Summary> {
    verify_bytes(
        client,
        object.backup,
        object.object,
        object.maximum_archive,
        master,
        expected,
        end,
    )
    .await
}
async fn verify_bytes(
    client: &s3::Client,
    backup: Uuid,
    object: Uuid,
    maximum: u64,
    master: &[u8; 32],
    expected: &backup_crypto::Summary,
    end: Instant,
) -> Result<backup_crypto::Summary> {
    let canceled = Arc::new(AtomicBool::new(false));
    let (writer, reader) = download_pipe(end, canceled.clone())?;
    let master = Zeroizing::new(*master);
    let decrypt = tokio::task::spawn_blocking(move || {
        backup_crypto::open(reader, std::io::sink(), &master, backup, object, maximum)
    });
    let downloaded = download(client, &format!("{backup}/{object}.sealed"), writer, end).await;
    if downloaded.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let plaintext = joined(decrypt).await;
    if downloaded? != *expected {
        return Err(stopped());
    }
    plaintext
}
async fn restore_inner(
    client: Arc<s3::Client>,
    target: PathBuf,
    object: Object,
    master: Zeroizing<[u8; 32]>,
    expected: Verified,
    end: Instant,
) -> Result<Verified> {
    let written = target_pass(&client, target.clone(), object, &master, end, false).await?;
    if !same_data(&written, &expected) {
        return Err(stopped());
    }
    // Independently read the actual restored bytes, holes and metadata before
    // returning a publishable result. fsync alone is not a content comparison.
    let verified = target_pass(&client, target, object, &master, end, true).await?;
    if !same_data(&verified, &expected) {
        return Err(stopped());
    }
    Ok(verified)
}
fn same_data(a: &Verified, b: &Verified) -> bool {
    a.plaintext == b.plaintext && a.ciphertext == b.ciphertext && a.archive == b.archive
}
async fn target_pass(
    client: &s3::Client,
    target: PathBuf,
    object: Object,
    master: &[u8; 32],
    end: Instant,
    verify: bool,
) -> Result<Verified> {
    let canceled = Arc::new(AtomicBool::new(false));
    let (network_writer, encrypted_reader) = download_pipe(end, canceled.clone())?;
    let (plain_writer, plain_reader) = UnixStream::pair()?;
    let plain_writer = Pipe::new(plain_writer, end, canceled.clone())?;
    let mut plain_reader = Pipe::new(plain_reader, end, canceled.clone())?;
    let master = Zeroizing::new(*master);
    let decrypt = tokio::task::spawn_blocking(move || {
        backup_crypto::open(
            encrypted_reader,
            plain_writer,
            &master,
            object.backup,
            object.object,
            object.maximum_archive,
        )
    });
    let extract = tokio::task::spawn_blocking(move || {
        if verify {
            archive::verify(
                &target,
                &mut plain_reader,
                object.volume,
                object.maximum_data,
            )
        } else {
            archive::unpack(
                &target,
                &mut plain_reader,
                object.volume,
                object.maximum_data,
            )
        }
    });
    let ciphertext = download(client, &object.key(), network_writer, end).await;
    if ciphertext.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let plaintext = joined(decrypt).await;
    if plaintext.is_err() {
        canceled.store(true, Ordering::Release);
    }
    let archive = joined(extract).await;
    let actual = Verified {
        ciphertext: ciphertext?,
        plaintext: plaintext?,
        archive: archive?,
        off_host: client.off_host(),
    };
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Seek, SeekFrom},
        os::unix::fs::{symlink, MetadataExt},
    };

    fn object() -> Object {
        Object {
            backup: Uuid::new_v4(),
            object: Uuid::new_v4(),
            volume: Uuid::new_v4(),
            maximum_data: 32 * 1024 * 1024,
            maximum_archive: 33 * 1024 * 1024,
        }
    }
    #[derive(Default)]
    struct TestJournal {
        saved: std::sync::Mutex<Option<SavedUpload>>,
        reject: AtomicBool,
    }
    #[tonic::async_trait]
    impl Journal for TestJournal {
        async fn prepared(
            &self,
            _object: Object,
            upload: &str,
            etags: &[String],
            plaintext: &backup_crypto::Summary,
            ciphertext: &backup_crypto::Summary,
            archive: &archive::Summary,
        ) -> Result<()> {
            *self.saved.lock().unwrap() = Some(SavedUpload {
                id: upload.into(),
                etags: etags.to_vec(),
                plaintext: plaintext.clone(),
                ciphertext: ciphertext.clone(),
                archive: archive.clone(),
            });
            if self.reject.load(Ordering::SeqCst) {
                return Err(Error::denied());
            }
            Ok(())
        }
    }
    #[tokio::test]
    async fn actual_s3_prepared_journal_blocks_completion_and_recovers_lost_success() {
        let Some((client, root)) = s3::tests::fixture().await else {
            return;
        };
        let client = Arc::new(client);
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload"), b"durable backup point").unwrap();
        let journal = Arc::new(TestJournal::default());
        let pool = Pool::with_journal(journal.clone());
        let good = object();
        let upload = client.initiate(&good.key()).await.unwrap();
        let verified = pool
            .upload(
                client.clone(),
                source.clone(),
                good,
                Zeroizing::new([16; 32]),
                upload,
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        let saved = journal.saved.lock().unwrap().clone().unwrap();
        // Complete now reports NoSuchUpload, as after a lost response or daemon
        // restart. Exact readback recovers the already published object.
        assert_eq!(
            pool.recover(
                client.clone(),
                good,
                Zeroizing::new([16; 32]),
                saved.clone(),
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .unwrap(),
            verified
        );
        let mut wrong = saved;
        wrong.ciphertext.digest = format!("sha256:{}", "0".repeat(64));
        assert!(pool
            .recover(
                client.clone(),
                good,
                Zeroizing::new([16; 32]),
                wrong,
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        journal.reject.store(true, Ordering::SeqCst);
        let denied = Object {
            object: Uuid::new_v4(),
            ..good
        };
        let upload = client.initiate(&denied.key()).await.unwrap();
        assert!(pool
            .upload(
                client.clone(),
                source,
                denied,
                Zeroizing::new([16; 32]),
                upload,
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        assert!(client.get(&denied.key()).await.is_err());
        assert!(client
            .pending_uploads(&denied.key())
            .await
            .unwrap()
            .0
            .is_empty());
        s3::tests::cleanup(&client, &[good.key()]).await;
    }
    #[tokio::test]
    async fn actual_s3_archive_encryption_readback_restore_and_failed_producer() {
        let Some((client, root)) = s3::tests::fixture().await else {
            return;
        };
        let client = Arc::new(client);
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(source.join("payload"), vec![79; 10 * 1024 * 1024 + 17]).unwrap();
        fs::hard_link(source.join("payload"), source.join("payload-link")).unwrap();
        symlink("/must-never-follow", source.join("symlink")).unwrap();
        let mut sparse = fs::File::create(source.join("sparse")).unwrap();
        sparse.set_len(8 * 1024 * 1024 * 1024).unwrap();
        sparse.seek(SeekFrom::End(-4096)).unwrap();
        sparse.write_all(&[43; 4096]).unwrap();
        drop(sparse);
        let pool = Pool::default();
        let good = object();
        let upload = client.initiate(&good.key()).await.unwrap();
        let verified = pool
            .upload(
                client.clone(),
                source.clone(),
                good,
                Zeroizing::new([39; 32]),
                upload,
                Duration::from_secs(60),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(!verified.off_host);
        assert!(verified.ciphertext.bytes > PART as u64);
        assert!(verified.ciphertext.bytes < 12 * 1024 * 1024);
        assert_eq!(verified.archive.entries, 5);
        let restored = pool
            .restore(
                client.clone(),
                target.clone(),
                good,
                Zeroizing::new([39; 32]),
                verified.clone(),
                Duration::from_secs(60),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(restored, verified);
        assert_eq!(
            fs::read(target.join("payload")).unwrap(),
            fs::read(source.join("payload")).unwrap()
        );
        assert_eq!(
            fs::metadata(target.join("payload")).unwrap().ino(),
            fs::metadata(target.join("payload-link")).unwrap().ino()
        );
        let meta = fs::metadata(target.join("sparse")).unwrap();
        assert_eq!(meta.len(), 8 * 1024 * 1024 * 1024);
        assert!(meta.blocks() < 128);
        assert_eq!(
            fs::read_link(target.join("symlink")).unwrap(),
            PathBuf::from("/must-never-follow")
        );
        // A destination is never replaced, even when all remote checks succeed.
        assert!(pool
            .restore(
                client.clone(),
                target.clone(),
                good,
                Zeroizing::new([39; 32]),
                verified.clone(),
                Duration::from_secs(60)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        assert_eq!(fs::read(target.join("payload")).unwrap()[0], 79);

        // The archive producer fails after streaming a full multipart part.
        // Encryption can still authenticate EOF, but that cannot publish ready.
        let socket = std::os::unix::net::UnixListener::bind(source.join("z-socket")).unwrap();
        let failed = Object {
            object: Uuid::new_v4(),
            ..good
        };
        let upload = client.initiate(&failed.key()).await.unwrap();
        assert!(pool
            .upload(
                client.clone(),
                source.clone(),
                failed,
                Zeroizing::new([39; 32]),
                upload,
                Duration::from_secs(60)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        assert!(client.get(&failed.key()).await.is_err());
        drop(socket);
        fs::remove_file(source.join("z-socket")).unwrap();

        // The archive itself is complete, but the mandatory AEAD footer fails.
        // Even successful extraction is insufficient to verify the new volume.
        let sealed = s3::bounded(client.get(&good.key()).await.unwrap(), 12 * 1024 * 1024)
            .await
            .unwrap();
        let mut plain = Vec::new();
        backup_crypto::open(
            sealed.as_slice(),
            &mut plain,
            &[39; 32],
            good.backup,
            good.object,
            good.maximum_archive,
        )
        .unwrap();
        let bad = Object {
            object: Uuid::new_v4(),
            ..good
        };
        let mut sealed = Vec::new();
        backup_crypto::seal(
            plain.as_slice(),
            &mut sealed,
            &[39; 32],
            bad.backup,
            bad.object,
            bad.maximum_archive,
        )
        .unwrap();
        *sealed.last_mut().unwrap() ^= 1;
        let mut expected = verified.clone();
        expected.ciphertext = backup_crypto::Summary {
            bytes: sealed.len() as u64,
            digest: format!("sha256:{:x}", Sha256::digest(&sealed)),
        };
        client.put(&bad.key(), sealed).await.unwrap();
        let quarantine = root.path().join("unpublished");
        fs::create_dir(&quarantine).unwrap();
        assert!(pool
            .restore(
                client.clone(),
                quarantine,
                bad,
                Zeroizing::new([39; 32]),
                expected,
                Duration::from_secs(60)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        assert_eq!(fs::read(target.join("payload")).unwrap()[0], 79);
        s3::tests::cleanup(&client, &[good.key(), bad.key()]).await;
    }

    #[tokio::test]
    async fn dropped_waiters_keep_capacity_until_owned_workers_finish() {
        crate::init_crypto();
        let root = tempfile::tempdir_in("/tmp").unwrap();
        let credentials = root.path().join("s3.json");
        fs::write(&credentials, b"{\"access_key_id\":\"xelab-test\",\"secret_access_key\":\"test-only-backup-password\"}").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).unwrap();
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload"), b"private data").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, mut connections) = mpsc::channel(2);
        let server = tokio::spawn(async move {
            let mut sockets = Vec::new();
            loop {
                sockets.push(listener.accept().await.unwrap().0);
                let _ = accepted.send(()).await;
            }
        });
        let location = s3::Location::aws("s3://xelab-test-bucket/node/", "us-east-1")
            .unwrap()
            .compatible(&endpoint)
            .unwrap();
        let client = Arc::new(
            s3::Client::new(location, s3::Credentials::load(&credentials).unwrap()).unwrap(),
        );
        let pool = Pool::default();
        let mut waiters = Vec::new();
        for _ in 0..2 {
            waiters.push(
                pool.upload(
                    client.clone(),
                    source.clone(),
                    object(),
                    Zeroizing::new([12; 32]),
                    "saved-upload-id".into(),
                    Duration::from_secs(2),
                )
                .unwrap(),
            );
        }
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(2), connections.recv())
                .await
                .unwrap()
                .unwrap();
        }
        drop(waiters);
        assert!(pool
            .upload(
                client.clone(),
                source.clone(),
                object(),
                Zeroizing::new([12; 32]),
                "saved-upload-id".into(),
                Duration::from_secs(2)
            )
            .is_err());
        assert_eq!(pool.permits.available_permits(), 0);
        // Failed transfers also attempt bounded abort reconciliation; keep the
        // server responsive to connection closure only after the deadline.
        server.abort();
        tokio::time::timeout(Duration::from_secs(10), async {
            while pool.permits.available_permits() != 2 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}
