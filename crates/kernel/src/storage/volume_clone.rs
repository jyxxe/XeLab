//! Frozen source -> unpublished clone -> independent on-disk verification.
//! The caller owns source writer fencing, durable fresh quota allocations and
//! leases until this owned transfer has joined every filesystem worker.
use super::{archive, backup_crypto::Summary};
use crate::error::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{oneshot, Semaphore},
    task::JoinSet,
};
use uuid::Uuid;

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub volume: Uuid,
    pub maximum_data: u64,
    pub maximum_archive: u64,
}
impl Plan {
    fn validate(self) -> Result<()> {
        if self.volume.is_nil()
            || self.maximum_archive == 0
            || self.maximum_archive > 64 * 1024 * 1024 * 1024
            || self.maximum_data > self.maximum_archive
        {
            Err(Error::invalid("Invalid private clone bound"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub copied: Summary,
    pub verified: Summary,
    pub archive: archive::Summary,
}
pub struct Transfer {
    result: oneshot::Receiver<Result<Verified>>,
}
impl Transfer {
    pub async fn wait(self) -> Result<Verified> {
        self.result
            .await
            .map_err(|_| Error::unavailable("Private clone coordinator stopped"))?
    }
}
#[derive(Clone)]
pub struct Pool {
    permits: Arc<Semaphore>,
}
impl Default for Pool {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
        }
    }
}

struct Stream {
    socket: UnixStream,
    hash: Sha256,
    bytes: u64,
    maximum: u64,
}
impl Stream {
    fn new(socket: UnixStream, maximum: u64) -> Self {
        Self {
            socket,
            hash: Sha256::new(),
            bytes: 0,
            maximum,
        }
    }
    fn summary(&self) -> Summary {
        Summary {
            bytes: self.bytes,
            digest: format!("sha256:{:x}", self.hash.clone().finalize()),
        }
    }
    fn charge(&mut self, bytes: &[u8]) {
        self.hash.update(bytes);
        self.bytes += bytes.len() as u64;
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let allowed = (self.maximum - self.bytes).min(bytes.len() as u64) as usize;
        if allowed == 0 {
            return Err(std::io::Error::other("clone stream bound exceeded"));
        }
        let n = self.socket.write(&bytes[..allowed])?;
        self.charge(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.socket.flush()
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let allowed = (self.maximum - self.bytes).min(bytes.len() as u64) as usize;
        if allowed == 0 {
            let mut extra = [0];
            return if self.socket.read(&mut extra)? == 0 {
                Ok(0)
            } else {
                Err(std::io::Error::other("clone stream bound exceeded"))
            };
        }
        let n = self.socket.read(&mut bytes[..allowed])?;
        self.charge(&bytes[..n]);
        Ok(n)
    }
}
struct Stop {
    writer: UnixStream,
    reader: UnixStream,
}
impl Stop {
    fn shutdown(&self) {
        let _ = self.writer.shutdown(Shutdown::Both);
        let _ = self.reader.shutdown(Shutdown::Both);
    }
}
impl Drop for Stop {
    fn drop(&mut self) {
        self.shutdown();
    }
}
struct Proof {
    bytes: Summary,
    archive: archive::Summary,
}

async fn pass(
    source: PathBuf,
    target: PathBuf,
    plan: Plan,
    verify: bool,
    end: tokio::time::Instant,
) -> Result<Proof> {
    if tokio::time::Instant::now() >= end {
        return Err(Error::unavailable("Private clone deadline expired"));
    }
    let (writer, reader) = UnixStream::pair()?;
    let stop = Stop {
        writer: writer.try_clone()?,
        reader: reader.try_clone()?,
    };
    let mut workers = JoinSet::new();
    workers.spawn_blocking(move || -> Result<(bool, Proof)> {
        let mut stream = Stream::new(writer, plan.maximum_archive);
        let result = archive::pack(&source, &mut stream, plan.volume, plan.maximum_data);
        // Control descriptors stay alive to interrupt errors; explicit write
        // shutdown is necessary for the consumer to observe a successful EOF.
        let shutdown = stream.socket.shutdown(Shutdown::Write);
        let archive = result?;
        shutdown?;
        Ok((
            true,
            Proof {
                bytes: stream.summary(),
                archive,
            },
        ))
    });
    workers.spawn_blocking(move || -> Result<(bool, Proof)> {
        let mut stream = Stream::new(reader, plan.maximum_archive);
        let archive = if verify {
            archive::verify(&target, &mut stream, plan.volume, plan.maximum_data)?
        } else {
            archive::unpack(&target, &mut stream, plan.volume, plan.maximum_data)?
        };
        Ok((
            false,
            Proof {
                bytes: stream.summary(),
                archive,
            },
        ))
    });
    let mut producer = None;
    let mut consumer = None;
    let mut failure = None;
    let mut expired = false;
    while !workers.is_empty() {
        let joined = if expired {
            workers.join_next().await
        } else {
            tokio::select! {value=workers.join_next()=>value,_=tokio::time::sleep_until(end)=>{expired=true;failure=Some(Error::unavailable("Private clone deadline expired"));stop.shutdown();continue;}}
        };
        match joined {
            Some(Ok(Ok((is_producer, proof)))) => {
                if is_producer {
                    producer = Some(proof);
                } else {
                    consumer = Some(proof);
                }
            }
            Some(Ok(Err(error))) => {
                failure.get_or_insert(error);
                stop.shutdown();
            }
            Some(Err(_)) => {
                failure.get_or_insert_with(|| Error::unavailable("Private clone worker stopped"));
                stop.shutdown();
            }
            None => break,
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let producer = producer.ok_or_else(|| Error::unavailable("Missing source clone proof"))?;
    let consumer = consumer.ok_or_else(|| Error::unavailable("Missing target clone proof"))?;
    if producer.bytes != consumer.bytes || producer.archive != consumer.archive {
        return Err(Error::conflict("Private clone stream proof differs"));
    }
    Ok(producer)
}
impl Pool {
    /// Persists no public volume or schema state. Dropping the waiter does not
    /// cancel this supervisor. Errors retain the unpublished target for its
    /// coordinator's fenced cleanup; an existing target is never overwritten.
    pub fn clone_verified(
        &self,
        source: PathBuf,
        target: PathBuf,
        plan: Plan,
        timeout: Duration,
    ) -> Result<Transfer> {
        plan.validate()?;
        if timeout.is_zero() || timeout > Duration::from_secs(3600) {
            return Err(Error::invalid("Invalid private clone deadline"));
        }
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::exhausted())?;
        let (sender, result) = oneshot::channel();
        let end = tokio::time::Instant::now() + timeout;
        tokio::spawn(async move {
            let outcome = async {
                let (source, target) =
                    tokio::task::spawn_blocking(move || -> Result<(PathBuf, PathBuf)> {
                        for path in [&source, &target] {
                            let meta = std::fs::symlink_metadata(path)?;
                            if !meta.is_dir() || meta.file_type().is_symlink() {
                                return Err(Error::invalid(
                                    "Private clone roots must be directories",
                                ));
                            }
                        }
                        let source = source.canonicalize()?;
                        let target = target.canonicalize()?;
                        if source.starts_with(&target) || target.starts_with(&source) {
                            return Err(Error::invalid("Private clone roots overlap"));
                        }
                        Ok((source, target))
                    })
                    .await
                    .map_err(|_| Error::unavailable("Private clone preflight stopped"))??;
                let copied = pass(source.clone(), target.clone(), plan, false, end).await?;
                let verified = pass(source, target, plan, true, end).await?;
                if copied.archive != verified.archive {
                    return Err(Error::conflict("Frozen source clone changed"));
                }
                // Source reads may update atime, which is part of archive bytes.
                // The second pass independently compares every target byte and
                // metadata record; its stream hash need not equal the first.
                Ok(Verified {
                    copied: copied.bytes,
                    verified: verified.bytes,
                    archive: verified.archive,
                })
            }
            .await;
            drop(permit);
            let _ = sender.send(outcome);
        });
        Ok(Transfer { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{FileExt, MetadataExt, PermissionsExt},
    };
    fn private() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    fn plan() -> Plan {
        Plan {
            volume: Uuid::new_v4(),
            maximum_data: 4 * 1024 * 1024,
            maximum_archive: 8 * 1024 * 1024,
        }
    }
    #[tokio::test]
    async fn actual_sparse_hardlinks_symlinks_and_original_bytes_survive_clone() {
        let source = private();
        let target = private();
        fs::write(source.path().join("sentinel"), b"retained original data").unwrap();
        fs::hard_link(source.path().join("sentinel"), source.path().join("alias")).unwrap();
        std::os::unix::fs::symlink("sentinel", source.path().join("link")).unwrap();
        let sparse = fs::File::create(source.path().join("sparse")).unwrap();
        sparse.set_len(8 * 1024 * 1024 * 1024).unwrap();
        sparse
            .write_all_at(b"tail", 8 * 1024 * 1024 * 1024 - 4)
            .unwrap();
        sparse.sync_all().unwrap();
        drop(sparse);
        let proof = Pool::default()
            .clone_verified(
                source.path().into(),
                target.path().into(),
                plan(),
                Duration::from_secs(5),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(proof.copied.bytes > 0 && proof.copied.bytes < 65536);
        assert_eq!(proof.archive.entries, 5);
        assert!(proof.verified.digest.starts_with("sha256:"));
        assert_eq!(
            fs::read(target.path().join("sentinel")).unwrap(),
            b"retained original data"
        );
        assert_eq!(
            fs::read(source.path().join("sentinel")).unwrap(),
            b"retained original data"
        );
        assert_eq!(
            fs::metadata(target.path().join("sentinel")).unwrap().ino(),
            fs::metadata(target.path().join("alias")).unwrap().ino()
        );
        assert_eq!(
            fs::read_link(target.path().join("link")).unwrap(),
            PathBuf::from("sentinel")
        );
        let file = fs::File::open(target.path().join("sparse")).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 8 * 1024 * 1024 * 1024);
        assert!(file.metadata().unwrap().blocks() < 128);
        let mut tail = [0; 4];
        file.read_exact_at(&mut tail, 8 * 1024 * 1024 * 1024 - 4)
            .unwrap();
        assert_eq!(&tail, b"tail");
    }
    #[tokio::test]
    async fn failed_producer_and_nonempty_target_join_without_overwrite() {
        let source = private();
        let target = private();
        let _socket =
            std::os::unix::net::UnixListener::bind(source.path().join("unsupported.socket"))
                .unwrap();
        let pool = Pool::default();
        let result = pool
            .clone_verified(
                source.path().into(),
                target.path().into(),
                plan(),
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), result.wait())
            .await
            .unwrap()
            .is_err());
        assert_eq!(pool.permits.available_permits(), 2);
        let source = private();
        let target = private();
        fs::write(source.path().join("large"), vec![0x5a; 2 * 1024 * 1024]).unwrap();
        fs::write(target.path().join("keep"), b"existing private evidence").unwrap();
        let result = pool
            .clone_verified(
                source.path().into(),
                target.path().into(),
                plan(),
                Duration::from_secs(2),
            )
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), result.wait())
            .await
            .unwrap()
            .is_err());
        assert_eq!(
            fs::read(target.path().join("keep")).unwrap(),
            b"existing private evidence"
        );
        assert_eq!(fs::read_dir(target.path()).unwrap().count(), 1);
        assert_eq!(pool.permits.available_permits(), 2);
    }
    #[tokio::test]
    async fn dropped_waiters_keep_two_owned_transfers_and_release_after_join() {
        let source = private();
        let first = private();
        let second = private();
        let extra = private();
        fs::write(source.path().join("data"), vec![0x36; 1024 * 1024]).unwrap();
        let pool = Pool::default();
        let one = pool
            .clone_verified(
                source.path().into(),
                first.path().into(),
                plan(),
                Duration::from_secs(5),
            )
            .unwrap();
        let two = pool
            .clone_verified(
                source.path().into(),
                second.path().into(),
                plan(),
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(pool
            .clone_verified(
                source.path().into(),
                extra.path().into(),
                plan(),
                Duration::from_secs(5)
            )
            .is_err());
        drop(one);
        drop(two);
        for _ in 0..250 {
            if pool.permits.available_permits() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(pool.permits.available_permits(), 2);
        assert_eq!(
            fs::read(first.path().join("data")).unwrap(),
            vec![0x36; 1024 * 1024]
        );
        assert_eq!(
            fs::read(second.path().join("data")).unwrap(),
            vec![0x36; 1024 * 1024]
        );
        assert_eq!(fs::read_dir(extra.path()).unwrap().count(), 0);
    }
}
