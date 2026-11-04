//! Actual signed OCI initialization before whole-platform capture. No schema
//! state, proof, provider readiness or successful migration row is seeded.
use super::*;
use xelab_kernel::{storage::xfs::ProjectStore, volumes};

pub struct Source {
    pub volume: Uuid,
    pub deployment: Uuid,
    pub data_digest: String,
    pub schema: String,
}

impl Source {
    pub fn report(&self) -> Value {
        json!({"volume_id":self.volume,"deployment_id":self.deployment,"data_digest":self.data_digest,"schema":self.schema})
    }
}

pub async fn data_path(app: &App, volume: Uuid) -> std::path::PathBuf {
    let project: i32 = app
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
    let until = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match ProjectStore::open(&app.config.root().join("volumes")) {
            Ok(store) => return store.data_path(volume, project as u32).unwrap(),
            Err(error)
                if error.code == "STATE_CONFLICT"
                    && error.message == "Storage backend is already in use"
                    && tokio::time::Instant::now() < until =>
            {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            Err(error) => panic!("actual source path unavailable: {error}"),
        }
    }
}

pub async fn create(
    app: &Arc<App>,
    router: &Router,
    token: &str,
    publisher: Uuid,
    signing: &SigningKey,
    schema: &str,
) -> Source {
    let c = app.db.connect().await.unwrap();
    let image = std::env::var(format!("XELAB_TEST_PLATFORM_SCHEMA_{schema}_IMAGE"))
        .expect("actual imported whole-platform schema image");
    let raw = fs::read(format!(
        "examples/generated-platform-schema-{schema}/xelab/plugin.json"
    ))
    .unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    contracts::validate("Manifest", &manifest).unwrap();
    assert_eq!(manifest["data_schema_version"], schema);
    assert_eq!(manifest["publisher_id"], publisher.to_string());
    let digest = artifact::image_ref(&image).unwrap();
    let pem = signing
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    let envelope = artifact::signature_envelope(&manifest, digest, &raw).unwrap();
    let signature = STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes());
    artifact::verify_signature(&pem, &signature, &manifest, digest, &raw).unwrap();
    app.runtime.verify_image(&image, digest).await.unwrap();
    let mount = app.runtime.mount_image(&image, digest).await.unwrap();
    assert_eq!(
        artifact::read_image_file(&mount, "xelab/plugin.json", 1048576).unwrap(),
        raw
    );
    let request = json!({"publisher_id":publisher,"image":image,"signature":signature});
    match artifact::Artifact::inspect(&app.runtime, &request, &pem).await {
        Err(error) => assert_eq!(error.message, "Unsupported plugin profile"),
        Ok(_) => panic!("public versioned admission must remain closed"),
    }
    let installation = Uuid::new_v4();
    let manifest_digest = contracts::digest(&raw);
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&digest,&manifest_digest,&manifest]).await.unwrap();
    for cap in manifest["capabilities"].as_array().unwrap() {
        let contract = &cap["contract"];
        c.execute("INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4) ON CONFLICT(name,version) DO NOTHING", &[&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap(),&contract["schema_digest"].as_str().unwrap(),&cap]).await.unwrap();
        let existing = c
            .query_one(
                "SELECT schema_digest,spec FROM capability_contracts WHERE name=$1 AND version=$2",
                &[
                    &contract["name"].as_str().unwrap(),
                    &contract["version"].as_str().unwrap(),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            existing.get::<_, String>("schema_digest"),
            contract["schema_digest"].as_str().unwrap()
        );
        assert_eq!(existing.get::<_, Value>("spec"), *cap);
    }
    tls::atomic_file(
        &app.config
            .root()
            .join("images")
            .join(&digest[7..])
            .with_extension("reference"),
        image.as_bytes(),
        0o600,
    )
    .unwrap();
    let volume = api(router,"createVolume",&[],Some(token),json!({"name":"data","capacity_bytes":8388608,"access":"single_writer","scope":"deployment"}),&format!("whole-versioned-{schema}-volume"),None).await;
    assert_eq!(volume.0, 202, "{}", volume.1);
    let allocation: Uuid = volume.1["id"].as_str().unwrap().parse().unwrap();
    for _ in 0..120 {
        volumes::tick(app).await.unwrap();
        let state = c
            .query_one("SELECT state FROM operations WHERE id=$1", &[&allocation])
            .await
            .unwrap()
            .get::<_, String>(0);
        assert_ne!(state, "failed");
        if state == "succeeded" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    operation(router, &allocation.to_string(), token).await;
    let volume: Uuid = volume.1["resource_id"].as_str().unwrap().parse().unwrap();
    let deployment = api(router,"createDeployment",&[],Some(token),json!({"installation_id":installation,"display_name":"whole-platform versioned source","enabled":false,"replicas":1,"limits":{"cpu_millis":100,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64},"config":{},"volume_ids":[volume]}),&format!("whole-versioned-{schema}-deployment"),None).await;
    assert_eq!(deployment.0, 202, "{}", deployment.1);
    operation(router, deployment.1["id"].as_str().unwrap(), token).await;
    let deployment: Uuid = deployment.1["resource_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let params = [("deployment_id", deployment.to_string())];
    let current = api(
        router,
        "getDeployment",
        &params,
        Some(token),
        Value::Null,
        &format!("whole-versioned-{schema}-current"),
        None,
    )
    .await;
    let approved = api(
        router,
        "approvePlugin",
        &params,
        Some(token),
        json!({"rules":[],"egress":[]}),
        &format!("whole-versioned-{schema}-approve"),
        current.1["revision"].as_i64(),
    )
    .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    let current = api(
        router,
        "getDeployment",
        &params,
        Some(token),
        Value::Null,
        &format!("whole-versioned-{schema}-approved"),
        None,
    )
    .await;
    let enabled = api(
        router,
        "setDeploymentEnabled",
        &params,
        Some(token),
        json!({"enabled":true}),
        &format!("whole-versioned-{schema}-enable"),
        current.1["revision"].as_i64(),
    )
    .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    operation(router, enabled.1["id"].as_str().unwrap(), token).await;
    let called = api(router,"invokeCapability",&[],Some(token),json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"versioned data before whole recovery"},"provider_deployment_id":deployment,"timeout_ms":30000}),&format!("whole-versioned-{schema}-write"),None).await;
    assert_eq!(called.0, 200, "{}", called.1);
    let path = data_path(app, volume).await;
    let bytes = fs::read(path.join("store.json")).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["schema"], schema);
    assert_eq!(
        match schema {
            "1" => value["items"].as_array().unwrap().len(),
            "2" => value["records"].as_object().unwrap().len(),
            _ => panic!("unsupported reference format"),
        },
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT schema_state FROM volume_allocations WHERE volume_id=$1",
            &[&volume]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "verified"
    );
    for _ in 0..120 {
        volumes::tick(app).await.unwrap();
        if !c.query_one("SELECT EXISTS(SELECT 1 FROM volume_allocations WHERE state NOT IN ('ready','deleted'))", &[]).await.unwrap().get::<_,bool>(0) {break;}
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!c.query_one("SELECT EXISTS(SELECT 1 FROM volume_allocations WHERE state NOT IN ('ready','deleted'))", &[]).await.unwrap().get::<_,bool>(0));
    Source {
        volume,
        deployment,
        data_digest: contracts::digest(&bytes),
        schema: schema.to_owned(),
    }
}
