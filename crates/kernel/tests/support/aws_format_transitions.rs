//! Optional actual AWS + OCI/XFS component drill. Never sets Ready/off_host or
//! format proofs in SQL. Missing resources mean this drill is unexecuted, not
//! passing. Measured host capacity still does not establish RUN-03 reserves.
use super::*;
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use rand::{rngs::OsRng, RngCore};
use std::{os::unix::fs::OpenOptionsExt, sync::atomic::Ordering};
use xelab_kernel::{backup_repository::Settings, backups, controller, schema_jobs, tls, upgrades};

fn id(value: &Value, field: &str) -> Uuid {
    value[field].as_str().unwrap().parse().unwrap()
}
async fn terminal(f: &Fixture, operation: Uuid) -> Value {
    for _ in 0..1200 {
        let response = f
            .api(
                "getOperation",
                Some(operation),
                Value::Null,
                "aws-poll",
                None,
                None,
            )
            .await;
        assert_eq!(response.0, 200, "{}", response.1);
        assert_ne!(response.1["state"], "failed", "{}", response.1);
        if response.1["state"] == "succeeded" {
            return response.1;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("AWS component operation did not settle within ten minutes");
}
async fn ready(f: &Fixture, deployment: Uuid) -> Value {
    for _ in 0..240 {
        let d = schema_bootstrap::current(f, deployment).await;
        if d["phase"] == "ready" {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("actual AWS test deployment did not become Ready");
}
async fn bytes(f: &Fixture, volume: Uuid) -> Vec<u8> {
    fs::read(
        schema_bootstrap::data_path(f, volume)
            .await
            .join("store.json"),
    )
    .unwrap()
}
async fn invoke(f: &Fixture, deployment: Uuid, manifest: &Value, key: &str) {
    let result = f.api("invokeCapability",None,json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":key},"provider_deployment_id":deployment,"timeout_ms":30000}),key,None,None).await;
    assert_eq!(result.0, 200, "{}", result.1);
}
async fn change(
    f: &Fixture,
    deployment: Uuid,
    installation: Uuid,
    backup: Uuid,
    action: &str,
    key: &str,
) -> Value {
    let d = ready(f, deployment).await;
    let response = f.api(action,Some(deployment),json!({"installation_id":installation,"strategy":"recreate","allow_downtime":true,"backup_id":backup}),key,d["revision"].as_i64(),None).await;
    assert_eq!(response.0, 202, "{}", response.1);
    let operation = terminal(f, id(&response.1, "id")).await;
    contracts::validate("Deployment", &operation["result"]).unwrap();
    operation
}

pub async fn exercise() {
    xelab_kernel::init_crypto();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    xelab_kernel::config::require_host_admin().unwrap();
    assert_eq!(std::env::var("XELAB_AWS_FORMAT_TEST").unwrap(), "1");
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    assert!(
        std::env::var_os("XELAB_TEST_S3_ENDPOINT").is_none(),
        "compatible endpoints cannot establish actual AWS acceptance"
    );
    let namespace = std::env::var("XELAB_AWS_FORMAT_NAMESPACE").unwrap();
    assert!(namespace.starts_with("xelab-aws-format-"));
    let base_uri = std::env::var("XELAB_AWS_FORMAT_URI").unwrap();
    let parsed_uri = url::Url::parse(&base_uri).unwrap();
    assert_eq!(parsed_uri.scheme(), "s3");
    assert!(
        !parsed_uri.path().trim_matches('/').is_empty(),
        "provide an explicitly authorized test prefix"
    );
    let evidence =
        std::path::PathBuf::from(std::env::var("XELAB_AWS_FORMAT_EVIDENCE_DIR").unwrap());
    let metadata = evidence.symlink_metadata().unwrap();
    use std::os::unix::fs::MetadataExt;
    assert!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.mode() & 0o077 == 0
    );
    assert!(
        fs::read_dir(&evidence).unwrap().next().is_none(),
        "use a new empty private evidence directory"
    );
    let capacity = admission::host_capacity().unwrap();
    let mut f = Fixture::new_with_capacity(true, Some(capacity))
        .await
        .expect("actual test PostgreSQL required");
    f.app.runtime.retain_daemon_host_lease().unwrap();
    let run = Uuid::new_v4();
    let uri = format!("{}/format-{run}/", base_uri.trim_end_matches('/'));
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    let master_path = evidence.join("master.key");
    let mut master_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&master_path)
        .unwrap();
    master_file.write_all(key.as_ref()).unwrap();
    master_file.sync_all().unwrap();
    fs::File::open(&evidence).unwrap().sync_all().unwrap();
    let mut value = f.app.config.value.clone();
    value["runtime"]["namespace"] = json!(namespace);
    value["storage"]["off_host_backup_uri"] = json!(uri);
    value["security"]["master_key_file"] = json!(master_path);
    let config = Config::from_value(value).unwrap();
    let app = Arc::new(App {
        config: config.clone(),
        runtime: Runtime::new(config),
        db: f.app.db.clone(),
        auth: Auth::new(vec![82; 32]).unwrap(),
        leader: f.app.leader.clone(),
        ready: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        blob_uploads: tokio::sync::Semaphore::new(4),
    });
    f.app = app;
    f.router = router(f.app.clone());
    assert!(
        f.app.runtime.inventory().await.unwrap().is_empty(),
        "the dedicated namespace must have no existing workloads"
    );
    let settings = Settings::from_host(&f.app.config).unwrap();
    assert!(settings.client().unwrap().off_host());
    let c = f.app.db.connect().await.unwrap();
    let database: String = c
        .query_one("SELECT current_database()", &[])
        .await
        .unwrap()
        .get(0);
    tls::atomic_file(&evidence.join("started.json"),&serde_json::to_vec(&json!({"run_id":run,"repository_uri":uri,"namespace":namespace,"database":database,"measured_cpu_millis":capacity.0,"measured_memory_bytes":capacity.1,"master_key_id":contracts::digest(key.as_ref())})).unwrap(),0o600).unwrap();
    let repository = settings.register(&c).await.unwrap();
    // Actual conditional S3 marker publication/read-back makes the repository
    // Ready. No direct state/off_host writes are permitted in this fixture.
    settings.bind(&f.app.db, repository).await.unwrap();
    f.app.runtime.prepare_host_service_network().await.unwrap();
    let publisher: Uuid = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let pem = SigningKey::from_bytes(&[7; 32])
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'AWS format test','aws-format-test',$2,'revoked',now(),now())",&[&publisher,&pem]).await.unwrap();
    let controller = tokio::spawn(controller::run(f.app.clone()));
    let bootstrap = tokio::spawn(schema_jobs::run(f.app.clone()));
    let backup_worker = tokio::spawn(backups::run(f.app.clone()));
    let upgrade_worker = tokio::spawn(upgrades::run(f.app.clone()));
    for _ in 0..240 {
        if f.app.ready.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(f.app.ready.load(Ordering::SeqCst));
    let trusted = f
        .api(
            "trustPublisher",
            None,
            json!({"name":"AWS format test","key_id":"aws-format-test","public_key_pem":pem}),
            "aws-explicit-publisher-trust",
            None,
            None,
        )
        .await;
    assert_eq!(trusted.0, 200, "{}", trusted.1);
    assert_eq!(trusted.1["id"], publisher.to_string());
    let (source, manifest, _) = schema_bootstrap::register(
        &f,
        publisher,
        "aws-source",
        "XELAB_TEST_AWS_FORMAT_SOURCE_IMAGE",
        "1",
    )
    .await;
    let (target, target_manifest, _) = schema_bootstrap::register(
        &f,
        publisher,
        "aws-target",
        "XELAB_TEST_AWS_FORMAT_TARGET_IMAGE",
        "2",
    )
    .await;
    assert!(manifest["entrypoint"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "--migration-observe-apply"));
    assert!(target_manifest["entrypoint"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "--migration-fail-after-write"));
    let original = schema_bootstrap::volume(&f, "aws-source-volume").await;
    let deployment =
        schema_bootstrap::create_deployment(&f, source, original, "aws-source-deployment").await;
    let (initialization, _) = schema_bootstrap::enable(&f, deployment, "aws-source-enable").await;
    terminal(&f, initialization).await;
    invoke(&f, deployment, &manifest, "aws-before-backup").await;
    let original_bytes = bytes(&f, original).await;
    let request = f
        .api(
            "createBackup",
            None,
            json!({"deployment_id":deployment,"volume_ids":[original],"destination":"off_host"}),
            "aws-real-backup",
            None,
            None,
        )
        .await;
    assert_eq!(request.0, 202, "{}", request.1);
    let backup_operation = terminal(&f, id(&request.1, "id")).await;
    let backup = id(&backup_operation["result"], "backup_id");
    let actual_backup = f
        .api(
            "getBackup",
            Some(backup),
            Value::Null,
            "aws-real-backup-read",
            None,
            None,
        )
        .await;
    assert_eq!(actual_backup.0, 200, "{}", actual_backup.1);
    assert_eq!(actual_backup.1["state"], "ready");
    assert_eq!(actual_backup.1["off_host"], true);
    let upgraded = change(
        &f,
        deployment,
        target,
        backup,
        "upgradeDeployment",
        "aws-format-upgrade",
    )
    .await;
    let upgraded_volume: Uuid = upgraded["result"]["spec"]["volume_ids"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(upgraded_volume, original);
    let data: Value = serde_json::from_slice(&bytes(&f, upgraded_volume).await).unwrap();
    assert_eq!(data["schema"], "2");
    assert_eq!(data["records"].as_object().unwrap().len(), 1);
    assert_eq!(bytes(&f, original).await, original_bytes);
    let upgrade_id = id(&upgraded, "id");
    let child = c
        .query_one(
            "SELECT scratch_volume_id,plan,state FROM volume_schema_jobs WHERE operation_id=$1",
            &[&upgrade_id],
        )
        .await
        .unwrap();
    assert_eq!(child.get::<_, String>("state"), "verified");
    let scratch = child.get::<_, Uuid>("scratch_volume_id");
    let attempt: Value = serde_json::from_slice(
        &fs::read(
            schema_bootstrap::data_path(&f, scratch)
                .await
                .join("apply-attempts.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let plan: Value = child.get::<_, Option<Value>>("plan").unwrap();
    assert_eq!(
        attempt,
        json!({"plan_id":plan["id"],"count":1}),
        "the ambiguous Apply RPC must never be resent"
    );
    invoke(&f, deployment, &target_manifest, "aws-v2-post-upgrade").await;
    let version_two_bytes = bytes(&f, upgraded_volume).await;
    let version_two: Value = serde_json::from_slice(&version_two_bytes).unwrap();
    assert_eq!(version_two["records"].as_object().unwrap().len(), 2);
    let rolled = change(
        &f,
        deployment,
        source,
        backup,
        "rollbackDeployment",
        "aws-format-rollback",
    )
    .await;
    let rolled_volume: Uuid = rolled["result"]["spec"]["volume_ids"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(rolled_volume, original);
    assert_ne!(rolled_volume, upgraded_volume);
    let rollback_child = c
        .query_one(
            "SELECT kind,state,scratch_volume_id FROM volume_schema_jobs WHERE operation_id=$1",
            &[&id(&rolled, "id")],
        )
        .await
        .unwrap();
    assert_eq!(rollback_child.get::<_, String>("kind"), "rollback");
    assert_eq!(rollback_child.get::<_, String>("state"), "verified");
    let rollback_scratch =
        schema_bootstrap::data_path(&f, rollback_child.get("scratch_volume_id")).await;
    assert!(
        !rollback_scratch.join("apply-attempts.json").exists(),
        "incompatible rollback must restore/Verify, never send inverse Apply"
    );
    assert_eq!(
        bytes(&f, rolled_volume).await,
        original_bytes,
        "incompatible rollback restores the old authenticated backup"
    );
    assert_eq!(
        bytes(&f, upgraded_volume).await,
        version_two_bytes,
        "current schema2 writes must remain in the retained source"
    );
    assert_eq!(bytes(&f, original).await, original_bytes);
    invoke(&f, deployment, &manifest, "aws-v1-post-rollback").await;
    let current = ready(&f, deployment).await;
    let disabled = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":false}),
            "aws-disable",
            current["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    terminal(&f, id(&disabled.1, "id")).await;
    assert!(f.app.runtime.inventory().await.unwrap().is_empty());
    let budget = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(budget.get::<_, i64>(0), 0);
    assert_eq!(budget.get::<_, i64>(1), 0);
    f.app.shutdown.store(true, Ordering::SeqCst);
    controller.await.unwrap();
    bootstrap.await.unwrap();
    backup_worker.await.unwrap();
    upgrade_worker.await.unwrap();
    let receipt = json!({"run_id":run,"repository_id":repository,"repository_uri":uri,"backup_id":backup,"backup_operation":id(&request.1,"id"),"deployment_id":deployment,"source_installation":source,"target_installation":target,"upgrade_operation":upgrade_id,"rollback_operation":id(&rolled,"id"),"original_volume":original,"upgraded_volume":upgraded_volume,"rolled_volume":rolled_volume,"backup_data_digest":contracts::digest(&original_bytes),"retained_schema2_digest":contracts::digest(&version_two_bytes),"actual_aws":true,"apply_rpc_attempts":1,"production_run03":false,"independent_host_recovery":false});
    tls::atomic_file(
        &evidence.join("completed.json"),
        &serde_json::to_vec(&receipt).unwrap(),
        0o600,
    )
    .unwrap();
    println!("AWS format component completed: actual signed OCI/XFS, real off-host backup, ambiguous Apply once then Verify, schema1->2 and incompatible backup rollback into new UUIDs; master key and receipts retained in the private evidence directory; separate RUN-03/independent-host acceptance remains required");
}
