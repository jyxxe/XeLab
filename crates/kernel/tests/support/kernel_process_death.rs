//! The shell owns the disposable XFS mount and launches two separate kernel
//! component processes. The first owns real host/DB/runtime leases and aborts
//! after the plugin fsyncs fresh data while withholding Initialize's response.
//! The second uses the original database, configuration, credential and job.
//! Synthetic host capacity remains component setup, never RUN-03 acceptance.
use super::*;
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use std::sync::atomic::Ordering;
use xelab_kernel::{controller, schema_jobs, tls};

async fn ready(f: &Fixture) {
    for _ in 0..120 {
        if f.app.ready.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("actual startup fencing did not complete");
}
fn uuid(value: &Value, key: &str) -> Uuid {
    value[key].as_str().unwrap().parse().unwrap()
}
async fn journal(f: &Fixture, operation: Uuid) -> Value {
    f.app
        .db
        .connect()
        .await
        .unwrap()
        .query_one(
            "SELECT to_jsonb(j) FROM volume_schema_jobs j WHERE operation_id=$1",
            &[&operation],
        )
        .await
        .unwrap()
        .get(0)
}

pub async fn exercise() {
    xelab_kernel::init_crypto();
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    assert_eq!(unsafe { libc::geteuid() }, 0);
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    let mode =
        std::env::var("XELAB_KERNEL_CRASH_MODE").expect("explicit isolated shell crash mode");
    let receipt = std::path::PathBuf::from(std::env::var("XELAB_KERNEL_CRASH_RECEIPT").unwrap());
    if mode == "abort" {
        let f = Fixture::new(true)
            .await
            .expect("actual PostgreSQL required");
        f.app.runtime.retain_daemon_host_lease().unwrap();
        assert!(f.app.runtime.host_lease().is_err());
        f.app.runtime.prepare_host_service_network().await.unwrap();
        let c = f.app.db.connect().await.unwrap();
        let publisher: Uuid = "00000000-0000-4000-8000-000000000001".parse().unwrap();
        let pem = SigningKey::from_bytes(&[7; 32])
            .verifying_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'crash fixture','crash-fixture',$2,'trusted',now(),now())", &[&publisher,&pem]).await.unwrap();
        let (installation, _, _) = schema_bootstrap::register(
            &f,
            publisher,
            "schema-crash",
            "XELAB_TEST_SCHEMA_CRASH_IMAGE",
            "1",
        )
        .await;
        f.app.ready.store(false, Ordering::SeqCst);
        tokio::spawn(controller::run(f.app.clone()));
        tokio::spawn(schema_jobs::run(f.app.clone()));
        ready(&f).await;
        let business = schema_bootstrap::volume(&f, "crash-business").await;
        let deployment =
            schema_bootstrap::create_deployment(&f, installation, business, "crash-deployment")
                .await;
        let (operation, _) =
            schema_bootstrap::enable(&f, deployment, "crash-original-enable").await;
        let path = schema_bootstrap::data_path(&f, business).await;
        for _ in 0..600 {
            let job = journal(&f, operation).await;
            assert_ne!(job["state"], "failed", "{job}");
            if job["state"] != "initializing" {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            let scratch = uuid(&job, "scratch_volume_id");
            let scratch_path = schema_bootstrap::data_path(&f, scratch).await;
            let trace = scratch_path.join("initialization-probe.json");
            if let Ok(bytes) = fs::read(&trace) {
                let events: Value = serde_json::from_slice(&bytes).unwrap();
                if events
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["phase"] == "initialize_fresh")
                {
                    assert_eq!(job["state"], "initializing");
                    assert_eq!(
                        events,
                        json!([
                            {"phase":"plan","business_read_only":true},
                            {"phase":"initialize_fresh","business_read_only":false}
                        ])
                    );
                    let bytes = fs::read(path.join("store.json")).unwrap();
                    let data: Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(data, json!({"schema":"1","items":[]}));
                    let instance = uuid(&job, "instance_id");
                    assert!(f.app.runtime.running(instance).await.unwrap());
                    assert_eq!(c.query_one("SELECT count(*) FROM reservations WHERE instance_id=$1 AND status IN ('active','reserved')", &[&instance]).await.unwrap().get::<_,i64>(0),1);
                    assert_eq!(c.query_one("SELECT count(*) FROM provider_records WHERE instance_id=$1 AND availability='ready'", &[&instance]).await.unwrap().get::<_,i64>(0),0);
                    let database: String = c
                        .query_one("SELECT current_database()", &[])
                        .await
                        .unwrap()
                        .get(0);
                    let dsn = format!(
                        "{} dbname={database}",
                        std::env::var("XELAB_TEST_DATABASE_URL").unwrap()
                    );
                    let budget = c
                        .query_one(
                            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
                            &[],
                        )
                        .await
                        .unwrap();
                    let saved = json!({"config":f.app.config.value,"dsn":dsn,"token":f.token,"owner":f.owner,"job":job,"business":business,"data_digest":contracts::digest(&bytes),"cpu":budget.get::<_,i64>(0),"memory":budget.get::<_,i64>(1),"dead_pid":std::process::id()});
                    tls::atomic_file(&receipt, &serde_json::to_vec(&saved).unwrap(), 0o600)
                        .unwrap();
                    // This kills the process owning App, the live controller,
                    // schema supervisor, database leader and retained host lease.
                    // No graceful shutdown, worker cancellation or metadata edit.
                    unsafe {
                        // A piped host core handler can ignore RLIMIT_CORE.
                        // Disable dumpability before aborting a credential owner.
                        assert_eq!(libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0), 0);
                        libc::raise(libc::SIGABRT);
                    }
                    panic!("SIGABRT returned unexpectedly");
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("actual fsynced initialization pause was not reached");
    }
    assert_eq!(mode, "resume");
    let saved: Value =
        serde_json::from_slice(&xelab_kernel::config::read_private(&receipt, 1048576).unwrap())
            .unwrap();
    assert_ne!(saved["dead_pid"], std::process::id());
    let config = Config::from_value(saved["config"].clone()).unwrap();
    assert_eq!(
        config.root(),
        std::path::PathBuf::from(std::env::var("XELAB_VOLUME_TEST_ROOT").unwrap())
    );
    let db = Db::new(saved["dsn"].as_str().unwrap().to_owned()).unwrap();
    db.check().await.unwrap();
    let app = Arc::new(App {
        runtime: Runtime::new(config.clone()),
        config,
        db: db.clone(),
        auth: Auth::new(vec![82; 32]).unwrap(),
        leader: Arc::new(db.leader().await.unwrap()),
        ready: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        blob_uploads: tokio::sync::Semaphore::new(4),
    });
    app.runtime.retain_daemon_host_lease().unwrap();
    let f = Fixture {
        router: router(app.clone()),
        app,
        token: saved["token"].as_str().unwrap().to_owned(),
        owner: uuid(&saved, "owner"),
        _root: tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap(),
    };
    let operation = uuid(&saved["job"], "operation_id");
    let instance = uuid(&saved["job"], "instance_id");
    let business = uuid(&saved, "business");
    let deployment = uuid(&saved["job"], "deployment_id");
    let c = f.app.db.connect().await.unwrap();
    f.app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    assert!(c
        .query_one(
            "SELECT verification_digest=$2 AND status='active' FROM credentials WHERE id=$1",
            &[
                &uuid(&saved["job"], "credential_id"),
                &f.app.auth.token_digest(&f.token)
            ]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
    assert_eq!(
        journal(&f, operation).await,
        saved["job"],
        "restart must not manufacture a new checkpoint"
    );
    assert!(
        f.app.runtime.running(instance).await.unwrap(),
        "old paused OCI task must still require actual startup fencing"
    );
    let controller = tokio::spawn(controller::run(f.app.clone()));
    let supervisor = tokio::spawn(schema_jobs::run(f.app.clone()));
    ready(&f).await;
    schema_bootstrap::terminal(&f, operation, "succeeded").await;
    let finished = journal(&f, operation).await;
    for field in [
        "operation_id",
        "deployment_id",
        "instance_id",
        "generation",
        "credential_id",
        "subject_id",
        "authority",
        "plan",
        "scratch_volume_id",
        "volume_ids",
        "volume_fence",
    ] {
        assert_eq!(
            finished[field], saved["job"][field],
            "original {field} must survive actual process death"
        );
    }
    assert_eq!(finished["state"], "verified");
    let path = schema_bootstrap::data_path(&f, business).await;
    assert_eq!(
        contracts::digest(&fs::read(path.join("store.json")).unwrap()),
        saved["data_digest"]
    );
    let scratch = uuid(&finished, "scratch_volume_id");
    let trace: Value = serde_json::from_slice(
        &fs::read(
            schema_bootstrap::data_path(&f, scratch)
                .await
                .join("initialization-probe.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        trace,
        json!([
            {"phase":"plan","business_read_only":true},
            {"phase":"initialize_fresh","business_read_only":false},
            {"phase":"verify","business_read_only":true}
        ]),
        "resume must Verify read-only before writable activation; no Plan/initialization replay"
    );
    let budget = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(json!(budget.get::<_, i64>(0)), saved["cpu"]);
    assert_eq!(json!(budget.get::<_, i64>(1)), saved["memory"]);
    assert_eq!(c.query_one("SELECT count(*) FROM reservations WHERE instance_id=$1 AND status IN ('reserved','active')", &[&instance]).await.unwrap().get::<_,i64>(0),1);
    let runtime: Value = serde_json::from_slice(
        &f.app
            .runtime
            .ctr(&[
                "containers".into(),
                "info".into(),
                "--spec".into(),
                instance.to_string(),
            ])
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(runtime["mounts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["destination"] != "/xelab-migration"));
    let manifest = &finished["installation_metadata"]["manifest"];
    let called = f.api("invokeCapability",None,json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"business write after actual kernel process death"},"provider_deployment_id":deployment,"timeout_ms":30000}),"after-kernel-death",None,None).await;
    assert_eq!(called.0, 200, "{}", called.1);
    let data: Value = serde_json::from_slice(&fs::read(path.join("store.json")).unwrap()).unwrap();
    assert_eq!(data["items"].as_array().unwrap().len(), 1);
    assert_eq!(data["items"][0]["id"], called.1["call_id"]);
    let current = schema_bootstrap::current(&f, deployment).await;
    let disabled = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":false}),
            "kernel-death-disable",
            current["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    schema_bootstrap::terminal(&f, uuid(&disabled.1, "id"), "succeeded").await;
    assert!(!f.app.runtime.running(instance).await.unwrap());
    f.app.shutdown.store(true, Ordering::SeqCst);
    controller.await.unwrap();
    supervisor.await.unwrap();
    fs::remove_file(receipt).unwrap();
    println!("Kernel component process death: actual SIGABRT after fsynced initialization, original leases reacquired, old OCI task stopped, original credential/job/instance/plan/reservation retained, RO Verify first, scratch detached and business write succeeded; not production-capacity daemon/RUN-03 or off-host acceptance");
}
