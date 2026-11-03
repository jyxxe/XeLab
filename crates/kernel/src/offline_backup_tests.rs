//! Publication/journal component regressions with PostgreSQL and real MinIO.
//! Synthetic metadata here is not a native snapshot or whole-host acceptance.
use super::*;
use crate::storage::s3;
use serde_json::json;
use std::{io::Write, os::unix::fs::PermissionsExt};
fn private_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap()
}

fn state(repository: Uuid, backup: Uuid, key: &[u8; 32], bytes: &[u8]) -> State {
    State {
        manifest: Manifest {
            format: 1,
            kind: "platform".into(),
            backup_id: backup,
            repository_id: repository,
            master_key_reference: digest(key),
            created_at: chrono::Utc::now(),
            stopped_lsn: "0/10".into(),
            metadata: RawReference {
                object: Uuid::new_v4(),
                plaintext: backup_crypto::Summary {
                    bytes: bytes.len() as u64,
                    digest: digest(bytes),
                },
                ciphertext: None,
            },
            metadata_summary: metadata_snapshot::Summary {
                snapshot: Uuid::new_v4(),
                schema_version: db::SCHEMA_VERSION,
                postgres_major: 16,
                lsn: "0/11".into(),
                bytes: bytes.len() as u64,
                rows: 0,
                sha256: digest(bytes)[7..].into(),
            },
            installations: vec![],
            images: None,
            volumes: vec![],
            blob_count: 1,
            blob_root: Some(Uuid::new_v4()),
            recovery_owners: vec![Uuid::new_v4()],
            off_host: false,
        },
        manifest_object: Uuid::new_v4(),
        manifest_digest: None,
    }
}
fn header(state: &State) -> Header {
    Header {
        format: 1,
        snapshot: state.manifest.backup_id,
        repository: state.manifest.repository_id,
        master_key_reference: state.manifest.master_key_reference.clone(),
        phase: "preparing".into(),
        context: Value::Null,
    }
}
fn file(root: &Path, name: &str, bytes: &[u8]) -> File {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(root.join(name))
        .unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    file
}
async fn session(root: &Path) -> Option<Arc<Session>> {
    let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: platform publication requires PostgreSQL");
        return None;
    };
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("platform_publication_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname={name}")).unwrap();
    db.migrate().await.unwrap();
    db.check().await.unwrap();
    let config: toml::Value =
        toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
    let mut value = serde_json::to_value(config).unwrap();
    value["storage"]["root_path"] = json!(root);
    let config = Config::from_value(value).unwrap();
    let leader = db.leader().await.unwrap();
    // Internal component harness: no host lease/Drain claim is made here.
    Some(Arc::new(Session {
        runtime: Runtime::new(config),
        db,
        leader,
        root: File::open(root).unwrap(),
        mutation: Mutex::new(()),
    }))
}
#[test]
fn source_checkpoint_validation_rejects_inconsistent_recovery_sets() {
    let key = [29; 32];
    let source = state(Uuid::new_v4(), Uuid::new_v4(), &key, b"component metadata");
    let h = header(&source);
    source.validate(&h, false).unwrap();
    assert!(source.validate(&h, true).is_err());
    for (path, value) in [
        ("/manifest/backup_id", json!(Uuid::new_v4())),
        ("/manifest/metadata_summary/sha256", json!("f".repeat(64))),
        ("/manifest/metadata_summary/schema_version", json!(0)),
        ("/manifest/metadata_summary/lsn", json!("0/F")),
        ("/manifest/stopped_lsn", json!("+1/11")),
        ("/manifest/blob_count", json!(65537)),
        ("/manifest/blob_root", Value::Null),
        ("/manifest/recovery_owners", json!([])),
        ("/manifest/metadata/object", json!(source.manifest_object)),
        ("/manifest/metadata/plaintext/digest", json!("not-a-digest")),
    ] {
        let mut broken = serde_json::to_value(&source).unwrap();
        *broken.pointer_mut(path).unwrap() = value;
        let broken: State = serde_json::from_value(broken).unwrap();
        assert!(broken.validate(&h, false).is_err(), "{path}");
    }
}
#[tokio::test]
async fn real_file_journal_and_s3_publish_only_complete_set_and_recover_without_source() {
    let root = private_root();
    let Some(session) = session(root.path()).await else {
        return;
    };
    let Some((client, _credentials)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    let repo = Repository {
        id: Uuid::new_v4(),
        objects: client.clone(),
    };
    let key = [29; 32];
    let metadata = b"protocol fixture only; not native database bytes";
    let state = state(repo.id, Uuid::new_v4(), &key, metadata);
    let workspace = Arc::new(Workspace::create(&session.root, header(&state)).unwrap());
    let blob = Blob {
        id: Uuid::new_v4(),
        plain: backup_crypto::Summary {
            bytes: 7,
            digest: digest(b"payload"),
        },
    };
    register(&workspace, &state, std::slice::from_ref(&blob)).unwrap();
    workspace.update_header("frozen", Value::Null).unwrap();
    workspace
        .update_header("transferring", serde_json::to_value(&state).unwrap())
        .unwrap();
    let journal = Arc::new(ObjectJournal {
        workspace: workspace.clone(),
        session: Arc::downgrade(&session),
    });
    let pool = backup_transfer::Pool::with_journal(journal.clone());
    let raw = pool.raw(Some(journal));
    let manifest_key = format!(
        "{}/{}.sealed",
        state.manifest.backup_id, state.manifest_object
    );
    upload_file(
        &workspace,
        &raw,
        &repo,
        state.manifest.backup_id,
        &state.manifest.metadata,
        file(root.path(), "meta.fixture", metadata),
        &key,
    )
    .await
    .unwrap();
    assert!(repo
        .objects
        .get_optional(&manifest_key)
        .await
        .unwrap()
        .is_none());
    assert!(
        finish(&session, &workspace, &repo, &key, state.clone(), false)
            .await
            .is_err(),
        "unprepared Blob must prevent publication"
    );
    let reference = RawReference {
        object: blob.id,
        plaintext: blob.plain.clone(),
        ciphertext: None,
    };
    upload_file(
        &workspace,
        &raw,
        &repo,
        state.manifest.backup_id,
        &reference,
        file(root.path(), "blob.fixture", b"payload"),
        &key,
    )
    .await
    .unwrap();
    // Original plaintext no longer exists: resume must use only saved prepared
    // identities and remote AEAD/SHA proofs, never mutable source files.
    std::fs::remove_file(root.path().join("meta.fixture")).unwrap();
    std::fs::remove_file(root.path().join("blob.fixture")).unwrap();
    let broken_key = [30; 32];
    assert!(finish(
        &session,
        &workspace,
        &repo,
        &broken_key,
        state.clone(),
        true
    )
    .await
    .is_err());
    assert!(repo
        .objects
        .get_optional(&manifest_key)
        .await
        .unwrap()
        .is_none());
    let receipt = finish(&session, &workspace, &repo, &key, state.clone(), true)
        .await
        .unwrap();
    assert!(!receipt.off_host);
    let published = repo
        .read_manifest(
            receipt.backup_id,
            receipt.manifest_object,
            &key,
            &receipt.manifest_digest,
        )
        .await
        .unwrap();
    assert_eq!(published["blob_count"], 1);
    assert_eq!(published["off_host"], false);
    assert!(published["metadata"]["ciphertext"].is_object());
    assert_eq!(workspace.header().unwrap().phase, "succeeded");
    assert!(
        cleanup_inner(&session, &workspace, &repo).await.is_err(),
        "published snapshots cannot enter failure cleanup"
    );
    s3::tests::cleanup(
        &client,
        &[
            manifest_key,
            format!(
                "{}/{}.sealed",
                state.manifest.backup_id, state.manifest.metadata.object
            ),
            format!("{}/{}.sealed", state.manifest.backup_id, blob.id),
        ],
    )
    .await;
}
#[tokio::test]
async fn exact_failed_upload_cleanup_finds_lost_initiation_and_preserves_neighbours() {
    let root = private_root();
    let Some(session) = session(root.path()).await else {
        return;
    };
    let Some((client, _credentials)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    let repo = Repository {
        id: Uuid::new_v4(),
        objects: client.clone(),
    };
    let state = state(repo.id, Uuid::new_v4(), &[29; 32], b"fixture");
    let workspace = Workspace::create(&session.root, header(&state)).unwrap();
    register(&workspace, &state, &[]).unwrap();
    workspace.update_header("frozen", Value::Null).unwrap();
    workspace
        .update_header("transferring", Value::Null)
        .unwrap();
    let id = state.manifest.metadata.object;
    workspace.upload_starting(id).unwrap();
    let key = format!("{}/{}.sealed", state.manifest.backup_id, id);
    let _lost = repo.objects.initiate(&key).await.unwrap();
    let neighbour = format!("{key}-neighbour");
    let other = repo.objects.initiate(&neighbour).await.unwrap();
    assert!(cleanup_inner(&session, &workspace, &repo).await.is_err());
    workspace.update_header("failed", Value::Null).unwrap();
    let result = cleanup_inner(&session, &workspace, &repo).await.unwrap();
    assert_eq!(result.objects_checked, 1);
    assert_eq!(result.uploads_aborted, 1);
    assert!(result.repeat_required);
    assert!(repo
        .objects
        .pending_uploads(&key)
        .await
        .unwrap()
        .0
        .is_empty());
    assert_eq!(
        repo.objects.pending_uploads(&neighbour).await.unwrap().0,
        vec![other.clone()]
    );
    // Simulate a late response after the earlier sweep: preserved intents make
    // the next sweep find it rather than declaring cleanup permanently done.
    repo.objects.initiate(&key).await.unwrap();
    assert_eq!(
        cleanup_inner(&session, &workspace, &repo)
            .await
            .unwrap()
            .uploads_aborted,
        1
    );
    repo.objects.abort(&neighbour, &other).await.unwrap();
    s3::tests::cleanup(&client, &[]).await;
}
