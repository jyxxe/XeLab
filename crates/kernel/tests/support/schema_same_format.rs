//! Actual already-verified format 1: signed program v1 -> v2 -> v1, fresh
//! private clones and current-data preservation. No schema conversion or
//! off-host backup admission is claimed. Public installation stays closed.
use super::*;

async fn current(f: &Fixture, deployment: Uuid) -> Value {
    let response = f
        .api(
            "getDeployment",
            Some(deployment),
            Value::Null,
            "same-format-current",
            None,
            None,
        )
        .await;
    assert_eq!(response.0, 200, "{}", response.1);
    response.1
}
async fn terminal(f: &Fixture, operation: Uuid) {
    for _ in 0..180 {
        let response = f
            .api(
                "getOperation",
                Some(operation),
                Value::Null,
                "same-format-poll",
                None,
                None,
            )
            .await;
        assert_eq!(response.0, 200, "{}", response.1);
        assert_ne!(response.1["state"], "failed", "{}", response.1);
        if response.1["state"] == "succeeded" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("same-format upgrade did not complete");
}
async fn data(f: &Fixture, volume: Uuid) -> Value {
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
    let path = ProjectStore::open(&f.app.config.root().join("volumes"))
        .unwrap()
        .data_path(volume, project as u32)
        .unwrap();
    serde_json::from_slice(&fs::read(path.join("store.json")).unwrap()).unwrap()
}

pub async fn exercise(
    f: &Fixture,
    publisher: Uuid,
    source: Uuid,
    deployment: Uuid,
    original_volume: Uuid,
) -> tokio::task::JoinHandle<()> {
    let (target, manifest, _) = super::schema_bootstrap::register(
        f,
        publisher,
        "schema-upgrade",
        "XELAB_TEST_SCHEMA_UPGRADE_IMAGE",
    )
    .await;
    assert_eq!(manifest["version"], "2.0.0");
    assert_eq!(manifest["data_schema_version"], "1");
    let supervisor = tokio::spawn(xelab_kernel::upgrades::run(f.app.clone()));
    let c = f.app.db.connect().await.unwrap();
    let original_data = data(f, original_volume).await;
    assert_eq!(original_data["items"].as_array().unwrap().len(), 1);
    let mut previous = original_volume;
    for (action, installation, key) in [
        ("upgradeDeployment", target, "schema-same-upgrade"),
        ("rollbackDeployment", source, "schema-same-rollback"),
    ] {
        let before = current(f, deployment).await;
        let revision = before["revision"].as_i64();
        let body =
            json!({"installation_id":installation,"strategy":"recreate","allow_downtime":true});
        let submitted = f
            .api(action, Some(deployment), body.clone(), key, revision, None)
            .await;
        assert_eq!(submitted.0, 202, "{}", submitted.1);
        let operation: Uuid = submitted.1["id"].as_str().unwrap().parse().unwrap();
        terminal(f, operation).await;
        let published = current(f, deployment).await;
        assert_eq!(
            published["spec"]["installation_id"],
            installation.to_string()
        );
        assert_eq!(published["phase"], "ready");
        let volume: Uuid = published["spec"]["volume_ids"][0]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(volume, previous);
        assert_ne!(volume, original_volume);
        let mapping = c.query_one("SELECT source_volume_id,target_volume_id,source_fence,target_fence,state FROM deployment_upgrade_volumes WHERE operation_id=$1", &[&operation]).await.unwrap();
        assert_eq!(mapping.get::<_, Uuid>("source_volume_id"), previous);
        assert_eq!(mapping.get::<_, Uuid>("target_volume_id"), volume);
        assert_eq!(mapping.get::<_, String>("state"), "published");
        let allocated = c.query_one("SELECT a.schema_state,a.data_schema_version,a.schema_plugin_id,a.schema_publisher_id,a.schema_operation_id,a.schema_proof,v.writer_instance_id,v.write_fence FROM volume_allocations a JOIN volumes v ON v.id=a.volume_id WHERE a.volume_id=$1", &[&volume]).await.unwrap();
        assert_eq!(allocated.get::<_, String>("schema_state"), "verified");
        assert_eq!(
            allocated
                .get::<_, Option<String>>("data_schema_version")
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            allocated.get::<_, Option<Uuid>>("schema_publisher_id"),
            Some(publisher)
        );
        assert_eq!(
            allocated.get::<_, Option<Uuid>>("schema_operation_id"),
            Some(operation)
        );
        assert_eq!(
            Some(allocated.get::<_, i64>("write_fence")),
            mapping.get::<_, Option<i64>>("target_fence")
        );
        let proof: Value = allocated.get::<_, Option<Value>>("schema_proof").unwrap();
        assert_eq!(proof["kind"], "same_schema_clone");
        assert_eq!(proof["source_volume_id"], previous.to_string());
        assert!(proof["source_binding"]["proof_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(
            serde_json::to_vec(&proof).unwrap().len() < 8192,
            "repeated clones must not recursively embed source proof history"
        );
        let instance = allocated
            .get::<_, Option<Uuid>>("writer_instance_id")
            .unwrap();
        assert!(f.app.runtime.running(instance).await.unwrap());
        assert_eq!(
            data(f, original_volume).await,
            original_data,
            "original initialized data remains unchanged"
        );
        if action == "upgradeDeployment" {
            let input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"new write made by the signed v2 program in format 1"},"provider_deployment_id":deployment,"timeout_ms":30000});
            let written = f
                .api(
                    "invokeCapability",
                    None,
                    input,
                    "schema-v2-current-write",
                    None,
                    None,
                )
                .await;
            assert_eq!(written.0, 200, "{}", written.1);
        }
        let current_data = data(f, volume).await;
        assert_eq!(current_data["schema"], "1");
        assert_eq!(
            current_data["items"].as_array().unwrap().len(),
            2,
            "same-format rollback must clone the current v2 writes"
        );
        assert_eq!(
            current_data["items"][1]["text"],
            "new write made by the signed v2 program in format 1"
        );
        let replay = f
            .api(action, Some(deployment), body, key, revision, None)
            .await;
        assert_eq!(replay.0, 202, "{}", replay.1);
        assert_eq!(replay.1["id"], submitted.1["id"]);
        previous = volume;
    }
    println!("Versioned same-format clone: actual signed v1->v2->v1, new UUIDs/proofs/fences, real v2 write preserved by rollback and original bytes retained; schema remains1, no format conversion/off-host claim");
    // Caller shuts down and joins all schedulers after final actual disable.
    supervisor
}
