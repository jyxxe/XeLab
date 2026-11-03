//! Real signed OCI + XFS same-schema upgrade, current-data rollback and failure.
//! This owned-host component drill makes no RUN-03/off-host acceptance claim.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use xelab_kernel::{artifact, controller, upgrades};

async fn terminal(f: &Fixture, operation: Uuid, expected: &str) -> Value {
    for _ in 0..180 {
        let response = f
            .api(
                "getOperation",
                Some(operation),
                Value::Null,
                "volume-upgrade-poll",
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
    panic!("volume upgrade operation did not settle");
}
async fn ready(f: &Fixture, deployment: Uuid) -> (Uuid, i64) {
    let c = f.app.db.connect().await.unwrap();
    for _ in 0..180 {
        if let Some(row) = c.query_opt("SELECT i.id,i.generation FROM instances i JOIN provider_records p ON p.instance_id=i.id JOIN deployments d ON d.id=i.deployment_id WHERE d.id=$1 AND d.phase='ready' AND i.phase='ready' AND p.availability='ready'", &[&deployment]).await.unwrap() {
            let instance: Uuid = row.get(0);
            if f.app.runtime.running(instance).await.unwrap() { return (instance, row.get(1)); }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("volume deployment did not become actually ready");
}
async fn install(f: &Fixture, publisher: Uuid, variant: &str, variable: &str) -> Uuid {
    let image = std::env::var(variable).unwrap();
    let raw = fs::read(format!("examples/generated-{variant}/xelab/plugin.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    let envelope =
        artifact::signature_envelope(&manifest, artifact::image_ref(&image).unwrap(), &raw)
            .unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let response = f.api("installPlugin", None, json!({"publisher_id":publisher,"image":image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())}), &format!("install-{variant}"), None, None).await;
    assert_eq!(response.0, 202, "{}", response.1);
    let operation: Uuid = response.1["id"].as_str().unwrap().parse().unwrap();
    terminal(f, operation, "succeeded").await["result"]["installation_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
async fn current(f: &Fixture, deployment: Uuid) -> Value {
    let response = f
        .api(
            "getDeployment",
            Some(deployment),
            Value::Null,
            "volume-upgrade-current",
            None,
            None,
        )
        .await;
    assert_eq!(response.0, 200, "{}", response.1);
    response.1
}
async fn volume_path(f: &Fixture, volume: Uuid) -> std::path::PathBuf {
    let c = f.app.db.connect().await.unwrap();
    let project: i32 = c
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

pub async fn exercise(f: &Fixture, source: Uuid, publisher: Uuid, volume_operation: &Value) {
    let c = f.app.db.connect().await.unwrap();
    let original_volume: Uuid = volume_operation["resource_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let original_path = volume_path(f, original_volume).await;
    fs::write(
        original_path.join("sentinel"),
        b"volume survives a container restart\n",
    )
    .unwrap();
    let controller = tokio::spawn(controller::run(f.app.clone()));
    for _ in 0..120 {
        if f.app.ready.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let target = install(
        f,
        publisher,
        "volume-upgrade",
        "XELAB_TEST_VOLUME_UPGRADE_IMAGE",
    )
    .await;
    let broken = install(
        f,
        publisher,
        "volume-failed-upgrade",
        "XELAB_TEST_VOLUME_FAILED_UPGRADE_IMAGE",
    )
    .await;
    let created = f.api("createDeployment", None, json!({"installation_id":source,"display_name":"same schema clone acceptance","enabled":false,"replicas":1,"limits":{"cpu_millis":100,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64},"config":{},"volume_ids":[original_volume]}), "volume-upgrade-deployment", None, None).await;
    assert_eq!(created.0, 202, "{}", created.1);
    let deployment: Uuid = created.1["resource_id"].as_str().unwrap().parse().unwrap();
    let before = current(f, deployment).await;
    let approved = f
        .api(
            "approvePlugin",
            Some(deployment),
            json!({"rules":[],"egress":[]}),
            "volume-upgrade-approve",
            before["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    // approvePlugin returns its approval body, not a Deployment revision.
    let approved_deployment = current(f, deployment).await;
    let enabled = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":true}),
            "volume-upgrade-enable",
            Some(approved_deployment["revision"].as_i64().unwrap()),
            None,
        )
        .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    let (source_instance, _) = ready(f, deployment).await;
    probe(f, source_instance, "read").await;
    let supervisor = tokio::spawn(upgrades::run(f.app.clone()));
    let mut previous_volume = original_volume;
    let mut previous_instance = source_instance;
    for (name, installation, key, mode) in [
        ("upgradeDeployment", target, "volume-upgrade", "append"),
        ("rollbackDeployment", source, "volume-rollback", "current"),
    ] {
        let before = current(f, deployment).await;
        let revision = before["revision"].as_i64();
        let body =
            json!({"installation_id":installation,"strategy":"recreate","allow_downtime":true});
        let submitted = f
            .api(name, Some(deployment), body.clone(), key, revision, None)
            .await;
        assert_eq!(submitted.0, 202, "{}", submitted.1);
        let operation: Uuid = submitted.1["id"].as_str().unwrap().parse().unwrap();
        let complete = terminal(f, operation, "succeeded").await;
        let next: Uuid = complete["result"]["spec"]["volume_ids"][0]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(next, previous_volume);
        assert_ne!(next, original_volume);
        assert!(!f.app.runtime.running(previous_instance).await.unwrap());
        let (candidate, generation) = ready(f, deployment).await;
        let journal = c.query_one("SELECT state,proof,source_fence,target_fence FROM deployment_upgrade_volumes WHERE operation_id=$1", &[&operation]).await.unwrap();
        assert_eq!(journal.get::<_, String>("state"), "published");
        assert!(journal.get::<_, Option<Value>>("proof").is_some());
        assert!(
            journal.get::<_, Option<i64>>("target_fence")
                > journal.get::<_, Option<i64>>("source_fence")
        );
        assert_eq!(c.query_one("SELECT count(*) FROM volume_attachments WHERE volume_id=$1 AND instance_id=$2 AND generation=$3", &[&next,&candidate,&generation]).await.unwrap().get::<_,i64>(0), 1);
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM volume_attachments WHERE volume_id=$1",
                &[&previous_volume]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0
        );
        let mounts = volumes::mounts(
            &f.app,
            deployment,
            candidate,
            generation,
            &c.query_one(
                "SELECT manifest FROM installations WHERE id=$1",
                &[&installation],
            )
            .await
            .unwrap()
            .get::<_, Value>(0),
        )
        .await
        .unwrap();
        assert_eq!(mounts[0].volume, next);
        assert_eq!(
            Some(mounts[0].fence),
            journal.get::<_, Option<i64>>("target_fence")
        );
        probe(f, candidate, mode).await;
        let replay = f
            .api(name, Some(deployment), body, key, revision, None)
            .await;
        assert_eq!(replay.0, 202, "{}", replay.1);
        assert_eq!(
            replay.1["id"], submitted.1["id"],
            "replay after the UUID switch must retain the original operation"
        );
        assert_eq!(
            fs::read(original_path.join("sentinel")).unwrap(),
            b"volume survives a container restart\n"
        );
        assert!(!original_path.join("after-upgrade").exists());
        previous_volume = next;
        previous_instance = candidate;
    }
    assert_eq!(
        fs::read(volume_path(f, previous_volume).await.join("after-upgrade")).unwrap(),
        b"written by the version 2 workload\n"
    );
    let before = current(f, deployment).await;
    let failed = f
        .api(
            "upgradeDeployment",
            Some(deployment),
            json!({"installation_id":broken,"strategy":"recreate","allow_downtime":true}),
            "volume-failed-upgrade",
            before["revision"].as_i64(),
            None,
        )
        .await;
    assert_eq!(failed.0, 202, "{}", failed.1);
    let operation: Uuid = failed.1["id"].as_str().unwrap().parse().unwrap();
    terminal(f, operation, "failed").await;
    let job = c.query_one("SELECT candidate_id,source_recovery,recovery_instance_id FROM deployment_upgrade_jobs WHERE operation_id=$1", &[&operation]).await.unwrap();
    assert_eq!(job.get::<_, String>("source_recovery"), "intent_created");
    assert!(!f
        .app
        .runtime
        .running(job.get("candidate_id"))
        .await
        .unwrap());
    let (recovery, _) = ready(f, deployment).await;
    assert_eq!(
        Some(recovery),
        job.get::<_, Option<Uuid>>("recovery_instance_id")
    );
    probe(f, recovery, "current").await;
    assert_eq!(
        current(f, deployment).await["spec"]["volume_ids"],
        json!([previous_volume])
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM volume_holds WHERE operation_id=$1",
            &[&operation]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert_eq!(c.query_one("SELECT count(*) FROM volume_allocations WHERE operation_id=$1 AND state='quarantined' AND NOT EXISTS(SELECT 1 FROM volumes v WHERE v.id=volume_allocations.volume_id)", &[&operation]).await.unwrap().get::<_,i64>(0), 1);
    // Reclaim only after the worker recorded positive candidate-stop proofs.
    volumes::tick(&f.app).await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM volume_allocations WHERE operation_id=$1 AND state='deleted'",
            &[&operation]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    let current = current(f, deployment).await;
    let disabled = f
        .api(
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":false}),
            "volume-upgrade-cleanup",
            current["revision"].as_i64(),
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
    f.app
        .shutdown
        .store(true, std::sync::atomic::Ordering::SeqCst);
    controller.await.unwrap();
    supervisor.await.unwrap();
    assert!(!f.app.runtime.running(recovery).await.unwrap());
    println!("real same-schema volume clone upgrade, current-data rollback, replay, quota/fences and failed-candidate source recovery verified");
}
