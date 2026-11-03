use super::*;
use std::{fs, os::unix::fs::PermissionsExt};
fn plain(data: &[u8]) -> backup_crypto::Summary {
    summary(data.len() as u64, Sha256::new().chain_update(data))
}
fn source(path: &std::path::Path, data: &[u8]) -> Source {
    fs::write(path, data).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    Source::new(File::open(path).unwrap(), plain(data)).unwrap()
}
fn target(path: &std::path::Path) -> Target {
    use std::os::unix::fs::OpenOptionsExt;
    Target::new(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .unwrap(),
    )
    .unwrap()
}
fn object(maximum_bytes: u64) -> Object {
    Object {
        backup: Uuid::new_v4(),
        object: Uuid::new_v4(),
        maximum_bytes,
    }
}
#[derive(Default)]
struct TestJournal {
    saved: std::sync::Mutex<Option<SavedUpload>>,
    reject: AtomicBool,
}
#[tonic::async_trait]
impl Journal for TestJournal {
    async fn prepared(&self, _object: Object, saved: &SavedUpload) -> Result<()> {
        *self.saved.lock().unwrap() = Some(saved.clone());
        if self.reject.load(Ordering::SeqCst) {
            return Err(Error::denied());
        }
        Ok(())
    }
}

#[tokio::test]
async fn actual_s3_raw_empty_and_multipart_round_trip_and_lost_completion() {
    let Some((client, root)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    for data in [Vec::new(), vec![47u8; 10 * 1024 * 1024]] {
        let journal = Arc::new(TestJournal::default());
        let pool = Pool::default().raw(Some(journal.clone()));
        let object = object(data.len() as u64);
        let key = object.key();
        let upload = client.initiate(&key).await.unwrap();
        let input = source(
            &root.path().join(format!("source-{}", object.object)),
            &data,
        );
        let verified = pool
            .upload_file(
                client.clone(),
                input,
                object,
                Zeroizing::new([27; 32]),
                upload,
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(verified.plaintext, plain(&data));
        assert!(!verified.off_host);
        let saved = journal.saved.lock().unwrap().clone().unwrap();
        assert_eq!(saved.plaintext, plain(&data));
        if data.len() > 8 * 1024 * 1024 {
            assert!(saved.etags.len() > 1);
        }
        let recovered = pool
            .recover(
                client.clone(),
                object,
                Zeroizing::new([27; 32]),
                saved.clone(),
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(recovered, verified);
        let output = root.path().join(format!("target-{}", object.object));
        let mut restored = pool
            .download_file(
                client.clone(),
                target(&output),
                object,
                Zeroizing::new([27; 32]),
                Expected {
                    plaintext: plain(&data),
                    ciphertext: Some(verified.ciphertext.clone()),
                },
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(restored.verified, verified);
        let mut actual = Vec::new();
        restored.file.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, data);
        // Native metadata's plaintext digest is sufficient even if the top-level
        // manifest derives Blob object keys instead of listing all cipher hashes.
        let restored = pool
            .download_file(
                client.clone(),
                target(&root.path().join(format!("native-target-{}", object.object))),
                object,
                Zeroizing::new([27; 32]),
                Expected {
                    plaintext: plain(&data),
                    ciphertext: None,
                },
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert_eq!(restored.verified.plaintext, plain(&data));
        let mut bad = saved;
        bad.ciphertext.digest = format!("sha256:{}", "0".repeat(64));
        assert!(pool
            .recover(
                client.clone(),
                object,
                Zeroizing::new([27; 32]),
                bad,
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
    }
}

#[tokio::test]
async fn actual_s3_wrong_source_or_journal_cannot_publish_and_private_downloads_fail_closed() {
    let Some((client, root)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    let journal = Arc::new(TestJournal::default());
    let parent = Pool::default();
    let pool = parent.raw(Some(journal.clone()));
    let data = b"raw immutable content";
    let original = plain(data);
    for reject_journal in [false, true] {
        journal.reject.store(reject_journal, Ordering::SeqCst);
        let object = object(1024);
        let key = object.key();
        let upload = client.initiate(&key).await.unwrap();
        let mut input = source(
            &root.path().join(format!("bad-source-{}", object.object)),
            data,
        );
        if !reject_journal {
            input.expected.digest = format!("sha256:{}", "0".repeat(64));
        }
        assert!(pool
            .upload_file(
                client.clone(),
                input,
                object,
                Zeroizing::new([27; 32]),
                upload,
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
        assert!(client.get(&key).await.is_err());
        assert!(client.pending_uploads(&key).await.unwrap().0.is_empty());
    }
    journal.reject.store(false, Ordering::SeqCst);
    // A producer can fail before writing while encryption still authenticates
    // a valid empty object. Its failure must independently prevent completion.
    let empty_object = object(0);
    let empty_key = empty_object.key();
    let path = root.path().join("changed-empty-source");
    let empty = source(&path, b"");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    let upload = client.initiate(&empty_key).await.unwrap();
    *journal.saved.lock().unwrap() = None;
    assert!(pool
        .upload_file(
            client.clone(),
            empty,
            empty_object,
            Zeroizing::new([27; 32]),
            upload,
            Duration::from_secs(30)
        )
        .unwrap()
        .wait()
        .await
        .is_err());
    assert!(journal.saved.lock().unwrap().is_none());
    assert!(client.get(&empty_key).await.is_err());
    let object = object(1024);
    let key = object.key();
    let upload = client.initiate(&key).await.unwrap();
    let verified = pool
        .upload_file(
            client.clone(),
            source(&root.path().join("good-source"), data),
            object,
            Zeroizing::new([27; 32]),
            upload,
            Duration::from_secs(30),
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
    for (key_byte, digest) in [
        (28, original.clone()),
        (27, plain(b"a different expected body")),
    ] {
        assert!(pool
            .download_file(
                client.clone(),
                target(&root.path().join(format!("bad-target-{key_byte}"))),
                object,
                Zeroizing::new([key_byte; 32]),
                Expected {
                    plaintext: digest,
                    ciphertext: Some(verified.ciphertext.clone())
                },
                Duration::from_secs(30)
            )
            .unwrap()
            .wait()
            .await
            .is_err());
    }
    // Archive and raw variants cannot each consume two slots independently.
    let _one = parent.permits.clone().acquire_owned().await.unwrap();
    let _two = parent.permits.clone().acquire_owned().await.unwrap();
    assert!(pool
        .download_file(
            client,
            target(&root.path().join("busy-target")),
            object,
            Zeroizing::new([27; 32]),
            Expected {
                plaintext: original,
                ciphertext: None
            },
            Duration::from_secs(30)
        )
        .is_err());
}

#[test]
fn source_and_target_require_private_single_link_regular_files() {
    let root = tempfile::tempdir().unwrap();
    let input = root.path().join("input");
    fs::write(&input, b"private").unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(Target::new(File::open(&input).unwrap()).is_err());
    fs::hard_link(&input, root.path().join("alias")).unwrap();
    assert!(Source::new(File::open(&input).unwrap(), plain(b"private")).is_err());
    fs::remove_file(root.path().join("alias")).unwrap();
    fs::set_permissions(&input, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Source::new(File::open(&input).unwrap(), plain(b"private")).is_err());
    assert!(Source::new(File::open(root.path()).unwrap(), plain(b"")).is_err());
}
