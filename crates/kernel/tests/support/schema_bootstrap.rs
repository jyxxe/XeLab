//! Real signed OCI/XFS initialization component. Public versioned installation
//! remains closed; only actually signature/image-verified bytes are registered
//! as internal component setup. No Ready provider or Verified format is seeded.
//! Synthetic metadata host capacity is inherited; this is not RUN-03 evidence.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use std::sync::atomic::Ordering;
use xelab_kernel::{artifact, controller, tls};

async fn terminal(f: &Fixture, operation: Uuid, expected: &str) -> Value {
    for _ in 0..180 {
        let response = f
            .api(
                "getOperation",
                Some(operation),
                Value::Null,
                "schema-poll",
                None,
                None,
            )
            .await;
        assert_eq!(response.0, 200, "{}", response.1);
        if matches!(response.1["state"].as_str(), Some("succeeded" | "failed")) {
            assert_eq!(response.1["state"], expected, "{}", response.1);
            return response.1;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("schema operation did not settle");
}
async fn current(f: &Fixture, deployment: Uuid) -> Value {
    let response = f
        .api(
            "getDeployment",
            Some(deployment),
            Value::Null,
            "schema-current",
            None,
            None,
        )
        .await;
    assert_eq!(response.0, 200, "{}", response.1);
    response.1
}
async fn volume(f: &Fixture, key: &str) -> Uuid {
    let created = f.api("createVolume",None,json!({"name":"data","capacity_bytes":8388608,"access":"single_writer","scope":"deployment"}),key,None,None).await;
    assert_eq!(created.0, 202, "{}", created.1);
    f.finish(&created.1).await;
    created.1["resource_id"].as_str().unwrap().parse().unwrap()
}
async fn data_path(f: &Fixture, volume: Uuid) -> std::path::PathBuf {
    let project: i32 = f
        .app
        .db
        .connect()
        .await
        .unwrap()
        .query_one(
            "SELECT project_id FROM volume_allocations WHERE volume_id=$1",
            &[&volume],
        )
        .await
        .unwrap()
        .get(0);
    ProjectStore::open(&f.app.config.root().join("volumes"))
        .unwrap()
        .data_path(volume, project as u32)
        .unwrap()
}
async fn create_deployment(f: &Fixture, installation: Uuid, volume: Uuid, key: &str) -> Uuid {
    let created = f.api("createDeployment",None,json!({"installation_id":installation,"display_name":"schema initialization acceptance","enabled":false,"replicas":1,"limits":{"cpu_millis":100,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64},"config":{},"volume_ids":[volume]}),key,None,None).await;
    assert_eq!(created.0, 202, "{}", created.1);
    let deployment: Uuid = created.1["resource_id"].as_str().unwrap().parse().unwrap();
    let before = current(f, deployment).await;
    let approved = f
        .api(
            "approvePlugin",
            Some(deployment),
            json!({"rules":[],"egress":[]}),
            &format!("{key}-approve"),
            before["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    deployment
}
async fn enable(f: &Fixture, deployment: Uuid, key: &str) -> (Uuid, i64) {
    let before = current(f, deployment).await;
    let revision = before["revision"].as_i64().unwrap();
    let response = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":true}),
            key,
            Some(revision),
            None,
        )
        .await;
    assert_eq!(response.0, 202, "{}", response.1);
    (
        response.1["id"].as_str().unwrap().parse().unwrap(),
        revision,
    )
}
async fn reap(f: &Fixture, scratch: Uuid) {
    for _ in 0..80 {
        volumes::tick(&f.app).await.unwrap();
        let state: String = f
            .app
            .db
            .connect()
            .await
            .unwrap()
            .query_one(
                "SELECT state FROM volume_allocations WHERE volume_id=$1",
                &[&scratch],
            )
            .await
            .unwrap()
            .get(0);
        if state == "deleted" {
            assert!(!f
                .app
                .config
                .root()
                .join("volumes")
                .join(scratch.to_string())
                .exists());
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("detached schema scratch was not physically reclaimed");
}

pub async fn exercise(f: &Fixture, publisher: Uuid) {
    assert_eq!(unsafe { libc::geteuid() }, 0);
    assert_eq!(std::env::var("XELAB_XFS_TEST").unwrap(), "1");
    let c = f.app.db.connect().await.unwrap();
    let image =
        std::env::var("XELAB_TEST_SCHEMA_IMAGE").expect("CI-imported versioned reference image");
    let raw = fs::read("examples/generated-schema/xelab/plugin.json").unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(manifest["data_schema_version"], "1");
    assert_eq!(manifest["publisher_id"], publisher.to_string());
    let digest = artifact::image_ref(&image).unwrap();
    let trusted = c
        .query_one(
            "SELECT public_key_pem,status FROM publishers WHERE id=$1",
            &[&publisher],
        )
        .await
        .unwrap();
    assert_eq!(trusted.get::<_, String>("status"), "trusted");
    let pem: String = trusted.get("public_key_pem");
    let envelope = artifact::signature_envelope(&manifest, digest, &raw).unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let signature = STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes());
    artifact::verify_signature(&pem, &signature, &manifest, digest, &raw).unwrap();
    f.app.runtime.verify_image(&image, digest).await.unwrap();
    let mounted = f.app.runtime.mount_image(&image, digest).await.unwrap();
    assert_eq!(
        artifact::read_image_file(&mounted, "xelab/plugin.json", 1048576).unwrap(),
        raw
    );
    let request = json!({"publisher_id":publisher,"image":image,"signature":signature});
    let rejected = match artifact::Artifact::inspect(&f.app.runtime, &request, &pem).await {
        Err(error) => error,
        Ok(_) => panic!("public versioned installation must stay closed"),
    };
    assert_eq!(rejected.message, "Unsupported plugin profile");
    // Internal component registration of real signature/content evidence, not
    // public install acceptance and not a fabricated format/health proof.
    let installation = Uuid::new_v4();
    let manifest_digest = contracts::digest(&raw);
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&digest,&manifest_digest,&manifest]).await.unwrap();
    for cap in manifest["capabilities"].as_array().unwrap() {
        let contract = &cap["contract"];
        c.execute(
            "INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4)",
            &[
                &contract["name"].as_str().unwrap(),
                &contract["version"].as_str().unwrap(),
                &contract["schema_digest"].as_str().unwrap(),
                &cap,
            ],
        )
        .await
        .unwrap();
    }
    tls::atomic_file(
        &f.app
            .config
            .root()
            .join("images")
            .join(&digest[7..])
            .with_extension("reference"),
        image.as_bytes(),
        0o600,
    )
    .unwrap();
    f.app.shutdown.store(false, Ordering::SeqCst);
    f.app.ready.store(false, Ordering::SeqCst);
    let controller = tokio::spawn(controller::run(f.app.clone()));
    let supervisor = tokio::spawn(xelab_kernel::schema_jobs::run(f.app.clone()));
    for _ in 0..120 {
        if f.app.ready.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        f.app.ready.load(Ordering::SeqCst),
        "actual startup fencing must complete before schema admission"
    );
    let business = volume(f, "schema-business-volume").await;
    let path = data_path(f, business).await;
    assert!(fs::read_dir(&path).unwrap().next().is_none());
    let deployment = create_deployment(f, installation, business, "schema-fresh-deployment").await;
    let (operation, revision) = enable(f, deployment, "schema-fresh-enable").await;
    terminal(f, operation, "succeeded").await;
    let job = c
        .query_one(
            "SELECT * FROM volume_schema_jobs WHERE operation_id=$1",
            &[&operation],
        )
        .await
        .unwrap();
    assert_eq!(job.get::<_, String>("state"), "verified");
    let instance: Uuid = job.get("instance_id");
    let generation: i64 = job.get("generation");
    let scratch: Uuid = job.get("scratch_volume_id");
    assert!(f.app.runtime.running(instance).await.unwrap());
    let bound = c
        .query_one(
            "SELECT * FROM volume_allocations WHERE volume_id=$1",
            &[&business],
        )
        .await
        .unwrap();
    assert_eq!(bound.get::<_, String>("schema_state"), "verified");
    assert_eq!(
        bound
            .get::<_, Option<String>>("data_schema_version")
            .as_deref(),
        Some("1")
    );
    assert_eq!(
        bound.get::<_, Option<Uuid>>("schema_operation_id"),
        Some(operation)
    );
    assert_eq!(
        bound.get::<_, Option<Uuid>>("schema_publisher_id"),
        Some(publisher)
    );
    assert_eq!(
        bound
            .get::<_, Option<String>>("schema_manifest_digest")
            .as_deref(),
        Some(manifest_digest.as_str())
    );
    let proof: Value = bound.get::<_, Option<Value>>("schema_proof").unwrap();
    assert_eq!(
        proof["plan_digest"],
        job.get::<_, Option<Value>>("plan").unwrap()["digest"]
    );
    assert_eq!(proof["instance_id"], instance.to_string());
    assert_eq!(proof["generation"], generation);
    let attached = c.query_one("SELECT a.instance_id,a.fence,v.writer_instance_id FROM volume_attachments a JOIN volumes v ON v.id=a.volume_id WHERE v.id=$1", &[&business]).await.unwrap();
    assert_eq!(attached.get::<_, Uuid>("instance_id"), instance);
    assert_eq!(
        attached.get::<_, Option<Uuid>>("writer_instance_id"),
        Some(instance)
    );
    assert_eq!(
        Some(attached.get::<_, i64>("fence")),
        job.get::<_, Option<i64>>("volume_fence")
    );
    assert_eq!(c.query_one("SELECT count(*) FROM reservations WHERE instance_id=$1 AND status IN ('reserved','active')", &[&instance]).await.unwrap().get::<_,i64>(0),1);
    let runtime = f
        .app
        .runtime
        .ctr(&[
            "containers".into(),
            "info".into(),
            "--spec".into(),
            instance.to_string(),
        ])
        .await
        .unwrap();
    let runtime: Value = serde_json::from_slice(&runtime).unwrap();
    assert!(
        runtime["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["destination"] != "/xelab-migration"),
        "published runtime cannot retain the private scratch mount"
    );
    assert!(runtime["annotations"][format!("org.xelab.volume.{scratch}")].is_null());
    let scratch_row = c.query_one("SELECT project_id,state,desired_capacity+desired_metadata_charge AS charge FROM volume_allocations WHERE volume_id=$1", &[&scratch]).await.unwrap();
    assert_eq!(scratch_row.get::<_, String>("state"), "quarantined");
    assert_eq!(
        ProjectStore::open(&f.app.config.root().join("volumes"))
            .unwrap()
            .inspect(scratch, scratch_row.get::<_, i32>("project_id") as u32)
            .unwrap()
            .limits
            .capacity_bytes,
        67108864
    );
    let before: i64 = c
        .query_one("SELECT reserved_bytes FROM volume_storage_root", &[])
        .await
        .unwrap()
        .get(0);
    reap(f, scratch).await;
    let after: i64 = c
        .query_one("SELECT reserved_bytes FROM volume_storage_root", &[])
        .await
        .unwrap()
        .get(0);
    assert!(
        after <= before - scratch_row.get::<_, i64>("charge") + 65536,
        "actual deletion must release scratch capacity while retaining its tombstone"
    );
    let replay = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":true}),
            "schema-fresh-enable",
            Some(revision),
            None,
        )
        .await;
    assert_eq!(replay.0, 202, "{}", replay.1);
    assert_eq!(replay.1["id"], operation.to_string());
    let input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"格式初始化后的一次持久写入"},"provider_deployment_id":deployment,"timeout_ms":30000});
    let called = f
        .api(
            "invokeCapability",
            None,
            input.clone(),
            "schema-business-call",
            None,
            None,
        )
        .await;
    assert_eq!(called.0, 200, "{}", called.1);
    let replay = f
        .api(
            "invokeCapability",
            None,
            input,
            "schema-business-call",
            None,
            None,
        )
        .await;
    assert_eq!(replay, called);
    let actual: Value =
        serde_json::from_slice(&fs::read(path.join("store.json")).unwrap()).unwrap();
    assert_eq!(actual["schema"], "1");
    assert_eq!(actual["items"].as_array().unwrap().len(), 1);
    assert_eq!(actual["items"][0]["id"], called.1["call_id"]);
    let before = current(f, deployment).await;
    let disabled = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":false}),
            "schema-disable",
            before["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    terminal(
        f,
        disabled.1["id"].as_str().unwrap().parse().unwrap(),
        "succeeded",
    )
    .await;
    assert!(!f.app.runtime.running(instance).await.unwrap());
    // A zero observation counter is insufficient: an actual nonempty unknown
    // directory must remain untouched and unversioned after failed admission.
    let unknown = volume(f, "schema-nonempty-volume").await;
    let unknown_path = data_path(f, unknown).await;
    fs::write(
        unknown_path.join("retain"),
        b"unknown business format must not be overwritten",
    )
    .unwrap();
    let deployment =
        create_deployment(f, installation, unknown, "schema-nonempty-deployment").await;
    let (failed, _) = enable(f, deployment, "schema-nonempty-enable").await;
    terminal(f, failed, "failed").await;
    assert_eq!(
        fs::read(unknown_path.join("retain")).unwrap(),
        b"unknown business format must not be overwritten"
    );
    assert_eq!(current(f, deployment).await["spec"]["enabled"], false);
    let source = c
        .query_one(
            "SELECT schema_state,data_schema_version FROM volume_allocations WHERE volume_id=$1",
            &[&unknown],
        )
        .await
        .unwrap();
    assert_eq!(source.get::<_, String>("schema_state"), "unversioned");
    assert!(source
        .get::<_, Option<String>>("data_schema_version")
        .is_none());
    let failed_job = c
        .query_one(
            "SELECT instance_id,scratch_volume_id FROM volume_schema_jobs WHERE operation_id=$1",
            &[&failed],
        )
        .await
        .unwrap();
    assert!(!f
        .app
        .runtime
        .running(failed_job.get("instance_id"))
        .await
        .unwrap());
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM deployment_storage_holds WHERE operation_id=$1",
            &[&failed]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    reap(f, failed_job.get("scratch_volume_id")).await;
    f.app.shutdown.store(true, Ordering::SeqCst);
    controller.await.unwrap();
    supervisor.await.unwrap();
    println!("Schema bootstrap: actual signed OCI and XFS empty-volume proof, RO Plan/Verify, one initialized format, healthy scratch-detached publication, real idempotent business write, physical scratch reclamation and nonempty-source refusal; internal registration, public installation closed, not RUN-03/off-host acceptance");
}
