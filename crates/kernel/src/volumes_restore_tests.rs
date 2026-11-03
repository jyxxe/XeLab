use super::*;
use crate::{
    api::App,
    backups::tests::Fixture,
    config::Config,
    runtime::Runtime,
    storage::{
        backup_transfer::{Object, Pool},
        s3,
    },
};
use std::{fs, io::Write, os::unix::ffi::OsStrExt, sync::atomic::AtomicBool};
use zeroize::Zeroizing;

#[tokio::test]
#[ignore = "requires a newly created isolated XFS restore mount and real MinIO; tools/test-xfs.sh"]
async fn actual_private_restore_allocator_and_verified_files() {
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let Some(f) = Fixture::new().await else {
        panic!("real PostgreSQL is required");
    };
    let Some((client, source_root)) = s3::tests::fixture().await else {
        panic!("real MinIO is required");
    };
    let root = std::env::var("XELAB_RESTORE_TEST_ROOT").expect("new owned restore filesystem");
    let mut value = f.app.config.value.clone();
    value["storage"]["root_path"] = json!(root);
    let config = Config::from_value(value).unwrap();
    let app = App {
        config: config.clone(),
        db: f.app.db.clone(),
        auth: f.app.auth.clone(),
        runtime: Runtime::new(config),
        leader: f.app.leader.clone(),
        ready: AtomicBool::new(true),
        shutdown: AtomicBool::new(false),
        blob_uploads: tokio::sync::Semaphore::new(4),
    };
    let mut c = app.db.connect().await.unwrap();
    let ctx = app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    let volume_spec = |capacity| json!({"name":"restored-data","capacity_bytes":capacity,"access":"single_writer","scope":"deployment"});
    let huge = plan_restore(
        &app,
        vec![(
            Uuid::new_v4(),
            volume_spec(63u64 * 1024 * 1024 * 1024),
            None,
        )],
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    let failed = db::operation(&tx, ctx.subject, "restore_component", None)
        .await
        .unwrap();
    assert!(reserve_restore(&tx, &ctx, failed, huge).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        c.query_one("SELECT reserved_bytes FROM volume_storage_root", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let fresh = Uuid::new_v4();
    let capacity = 8u64 * 1024 * 1024;
    let plan = plan_restore(&app, vec![(fresh, volume_spec(capacity), None)])
        .await
        .unwrap();
    let tx = c.transaction().await.unwrap();
    let operation = db::operation(
        &tx,
        ctx.subject,
        "restore_component",
        Some(fresh.to_string()),
    )
    .await
    .unwrap();
    reserve_restore(&tx, &ctx, operation, plan).await.unwrap();
    tx.commit().await.unwrap();
    assert!(c
        .query_opt("SELECT id FROM volumes WHERE id=$1", &[&fresh])
        .await
        .unwrap()
        .is_none());
    assert!(
        c.query_one("SELECT reserved_bytes FROM volume_storage_root", &[])
            .await
            .unwrap()
            .get::<_, i64>(0)
            > capacity as i64
    );
    let source = source_root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("sentinel"),
        b"restored to a newly reserved XFS project",
    )
    .unwrap();
    let sparse = fs::File::create(source.join("sparse")).unwrap();
    sparse.set_len(8 * 1024 * 1024 * 1024).unwrap();
    use std::os::unix::fs::FileExt;
    sparse
        .write_all_at(b"tail", 8 * 1024 * 1024 * 1024 - 4)
        .unwrap();
    drop(sparse);
    let name = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::chown(name.as_ptr(), 65532, 65532) }, 0);
    let client = Arc::new(client);
    let pool = Pool::default();
    let object = Object {
        backup: Uuid::new_v4(),
        object: Uuid::new_v4(),
        volume: Uuid::new_v4(),
        maximum_data: capacity,
        maximum_archive: capacity + 65 * 1024 * 1024,
    };
    let upload = client.initiate(&object.key()).await.unwrap();
    let verified = pool
        .upload(
            client.clone(),
            source,
            object,
            Zeroizing::new([45; 32]),
            upload,
            Duration::from_secs(60),
        )
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(!verified.off_host);
    let target = private_restore_path(&app, operation, fresh, true)
        .await
        .unwrap();
    assert_eq!(
        pool.restore(
            client.clone(),
            target.path.clone(),
            object,
            Zeroizing::new([45; 32]),
            verified.clone(),
            Duration::from_secs(60)
        )
        .unwrap()
        .wait()
        .await
        .unwrap(),
        verified
    );
    assert_eq!(
        pool.verify_restored(
            client.clone(),
            target.path.clone(),
            object,
            Zeroizing::new([45; 32]),
            verified.clone(),
            Duration::from_secs(60)
        )
        .unwrap()
        .wait()
        .await
        .unwrap(),
        verified
    );
    assert!(c
        .query_opt("SELECT id FROM volumes WHERE id=$1", &[&fresh])
        .await
        .unwrap()
        .is_none());
    c.execute(
        "UPDATE volume_allocations SET state='restored' WHERE volume_id=$1",
        &[&fresh],
    )
    .await
    .unwrap();
    let publication = prepare_restore_publication(&app, operation, &[fresh])
        .await
        .unwrap();
    let tx = c.transaction().await.unwrap();
    publish_restored(&tx, publication).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        fs::read(target.path.join("sentinel")).unwrap(),
        b"restored to a newly reserved XFS project"
    );
    let mut file = fs::File::create(target.path.join("quota-fill")).unwrap();
    let mut full = false;
    for _ in 0..256 {
        if let Err(error) = file.write_all(&[0; 65536]) {
            assert_eq!(error.raw_os_error(), Some(libc::EDQUOT));
            full = true;
            break;
        }
    }
    assert!(full, "new restore project must enforce its actual quota");
    drop(file);
    let public = c
        .query_one(
            "SELECT write_fence,state FROM volumes WHERE id=$1",
            &[&fresh],
        )
        .await
        .unwrap();
    assert!(public.get::<_, i64>(0) > 0);
    assert_eq!(public.get::<_, String>(1), "available");
    // The compatibility fixture never turns a Backup into off_host/ready.
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM backups WHERE off_host OR state='ready'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    s3::tests::cleanup(&client, &[object.key()]).await;
}
