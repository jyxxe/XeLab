//! Metadata fixtures are not an OCI/XFS or off-host production acceptance.
use super::*;
use crate::{
    auth::Auth,
    config::Config,
    db::Db,
    runtime::Runtime,
    storage::{
        backup_transfer::{Journal, Object, Pool},
        s3,
    },
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
struct Fixture {
    app: Arc<App>,
    token: String,
    credential: Uuid,
    operation: Uuid,
    backup: Uuid,
    volumes: [Uuid; 2],
    objects: [Uuid; 2],
    _root: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Option<Self> {
        crate::init_crypto();
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: backup journal needs PostgreSQL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("backup_journal_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let mut value = serde_json::to_value(t).unwrap();
        value["storage"]["root_path"] = json!(root.path());
        let config = Config::from_value(value).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(
            &tx,
            "Backup fixture owner",
            "correct horse backup fixture password",
            &config.value["quota_defaults"],
        )
        .await
        .unwrap();
        crate::admission::initialize(&tx, &config.value["resources"], 8000, 34359738368)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let auth = Auth::new(vec![69; 32]).unwrap();
        let issued = auth
            .login(
                &c,
                owner,
                "correct horse backup fixture password".into(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let token = issued["access_token"].as_str().unwrap().to_owned();
        let credential = issued["session"]["id"].as_str().unwrap().parse().unwrap();
        let publisher = Uuid::new_v4();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','backup-fixture','fixture-only','trusted',now(),now())",&[&publisher]).await.unwrap();
        let installation = Uuid::new_v4();
        let hash = digest(b"fixture installation");
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.backup-fixture','1.0.0',$2,$3,$3,'{}','installed',now(),now())",&[&installation,&publisher,&hash]).await.unwrap();
        let deployment = Uuid::new_v4();
        let volumes = [Uuid::new_v4(), Uuid::new_v4()];
        let objects = [Uuid::new_v4(), Uuid::new_v4()];
        let spec = json!({"enabled":false,"volume_ids":volumes});
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'disabled',now(),now())",&[&deployment,&owner,&installation,&spec]).await.unwrap();
        let repository = Uuid::new_v4();
        c.execute("INSERT INTO backup_repositories(id,location,region,state) VALUES($1,'s3://test-only-backup-bucket/node/','us-east-1','ready')",&[&repository]).await.unwrap();
        let backup = Uuid::new_v4();
        let operation = db::operation(&c, owner, "createBackup", Some(backup.to_string()))
            .await
            .unwrap();
        c.execute("INSERT INTO backups(id,deployment_id,created_by,volume_ids,off_host,state,created_at,updated_at) VALUES($1,$2,$3,$4,false,'creating',now(),now())",&[&backup,&deployment,&owner,&json!(volumes)]).await.unwrap();
        let paths = Policy::new(&c, "127.0.0.1".parse().unwrap())
            .credential(credential, 0)
            .await
            .unwrap();
        let metadata =
            json!({"id":installation,"image_digest":hash,"manifest_digest":hash,"manifest":{}});
        c.execute("INSERT INTO storage_recovery_jobs(operation_id,kind,backup_id,deployment_id,repository_id,credential_id,subject_id,peer,authority,state,original_spec,installation_digest,installation_metadata,instances,manifest_object_id,master_key_id) VALUES($1,'backup',$2,$3,$4,$5,$6,'127.0.0.1',$7,'transferring',$8,$9,$10,'[]',$11,$12)",&[&operation,&backup,&deployment,&repository,&credential,&owner,&serde_json::to_value(paths).unwrap(),&spec,&hash,&metadata,&Uuid::new_v4(),&digest(&[23;32])]).await.unwrap();
        c.execute(
            "INSERT INTO deployment_storage_holds(deployment_id,operation_id) VALUES($1,$2)",
            &[&deployment, &operation],
        )
        .await
        .unwrap();
        for (volume, object) in volumes.iter().zip(objects) {
            let spec = json!({"name":"fixture","capacity_bytes":1048576,"scope":"deployment","access":"single_writer"});
            c.execute("INSERT INTO volumes(id,owner_id,spec,backend_id,capacity_bytes,state,created_at,updated_at) VALUES($1,$2,$3,$4,1048576,'available',now(),now())",&[volume,&owner,&spec,&volume.to_string()]).await.unwrap();
            c.execute(
                "INSERT INTO volume_holds(volume_id,operation_id,kind) VALUES($1,$2,'backup')",
                &[volume, &operation],
            )
            .await
            .unwrap();
            c.execute("INSERT INTO backup_job_volumes(operation_id,source_volume_id,object_id,state,volume_spec,capacity_bytes) VALUES($1,$2,$3,'pending',$4,1048576)",&[&operation,volume,&object,&spec]).await.unwrap();
        }
        let app = Arc::new(App {
            config: config.clone(),
            db: db.clone(),
            auth,
            runtime: Runtime::new(config),
            leader: Arc::new(db.leader().await.unwrap()),
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        Some(Self {
            app,
            token,
            credential,
            operation,
            backup,
            volumes,
            objects,
            _root: root,
        })
    }
}

#[tokio::test]
async fn actual_postgres_and_s3_prepare_before_complete_and_revoke_without_ready() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let Some((client, root)) = s3::tests::fixture().await else {
        return;
    };
    let client = Arc::new(client);
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("data"), b"private consistent bytes").unwrap();
    let pool = Pool::with_journal(Arc::new(JobJournal { app: f.app.clone() }));
    let c = f.app.db.connect().await.unwrap();
    let mut keys = Vec::new();
    for index in 0..2 {
        let object = Object {
            backup: f.backup,
            object: f.objects[index],
            volume: f.volumes[index],
            maximum_data: 1048576,
            maximum_archive: 2 * 1048576,
        };
        let key = object.key();
        let upload = client.initiate(&key).await.unwrap();
        c.execute("UPDATE backup_job_volumes SET state='uploading',upload_id=$3 WHERE operation_id=$1 AND object_id=$2",&[&f.operation,&f.objects[index],&upload]).await.unwrap();
        if index == 1 {
            c.execute(
                "UPDATE credentials SET status='revoked' WHERE id=$1",
                &[&f.credential],
            )
            .await
            .unwrap();
        }
        let result = pool
            .upload(
                client.clone(),
                source.clone(),
                object,
                Zeroizing::new([23; 32]),
                upload,
                Duration::from_secs(30),
            )
            .unwrap()
            .wait()
            .await;
        if index == 0 {
            assert!(!result.unwrap().off_host);
            let row=c.query_one("SELECT state,transfer_summary FROM backup_job_volumes WHERE operation_id=$1 AND object_id=$2",&[&f.operation,&f.objects[index]]).await.unwrap();
            assert_eq!(row.get::<_, String>("state"), "prepared");
            let saved: crate::storage::backup_transfer::SavedUpload =
                serde_json::from_value(row.get("transfer_summary")).unwrap();
            assert!(!saved.etags.is_empty());
            assert!(saved.ciphertext.bytes > saved.plaintext.bytes);
            keys.push(key);
        } else {
            assert!(result.is_err());
            assert!(client.get(&key).await.is_err());
            assert!(client.pending_uploads(&key).await.unwrap().0.is_empty());
        }
    }
    let row = c
        .query_one(
            "SELECT state,off_host FROM backups WHERE id=$1",
            &[&f.backup],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>("state"), "creating");
    assert!(!row.get::<_, bool>("off_host"));
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM volume_holds WHERE operation_id=$1",
            &[&f.operation]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        2
    );
    s3::tests::cleanup(&client, &keys).await;
}

#[tokio::test]
async fn backup_metadata_http_and_hold_fences_use_current_authority() {
    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::Request,
    };
    use tower::ServiceExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    let router = api::router(f.app.clone());
    for path in [
        format!("/api/platform/v1/backups/{}", f.backup),
        "/api/platform/v1/backups".into(),
    ] {
        let mut request = Request::builder()
            .uri(path)
            .header("authorization", format!("Bearer {}", f.token))
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:30000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status().as_u16(), 200);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1048576).await.unwrap())
                .unwrap();
        assert!(!value.to_string().contains("test-only-backup-bucket"));
    }
    let c = f.app.db.connect().await.unwrap();
    let row = job(&f.app, f.operation).await.unwrap();
    let deployment: Uuid = row.get("deployment_id");
    assert!(ensure_unheld(&c, deployment).await.is_err());
    c.execute(
        "DELETE FROM volume_holds WHERE volume_id=$1",
        &[&f.volumes[0]],
    )
    .await
    .unwrap();
    let summary = crate::storage::backup_crypto::Summary {
        bytes: 0,
        digest: digest(b""),
    };
    let result = JobJournal { app: f.app.clone() }
        .prepared(
            Object {
                backup: f.backup,
                object: f.objects[0],
                volume: f.volumes[0],
                maximum_data: 1048576,
                maximum_archive: 2 * 1048576,
            },
            "missing",
            &["etag".into()],
            &summary,
            &summary,
            &crate::storage::archive::Summary {
                entries: 1,
                data_bytes: 0,
            },
        )
        .await;
    assert!(result.is_err());
    assert!(revalidate(&c, &f.app, &row).await.is_err());
}
