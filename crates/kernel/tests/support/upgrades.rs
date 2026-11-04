//! Actual signed OCI upgrade/rollback and failed candidate admission. No data
//! schema migration or production capacity/off-host claim is made here.
use super::*;

async fn install(
    router: &Router,
    token: &str,
    publisher: Uuid,
    signing: &SigningKey,
    variant: &str,
    variable: &str,
) -> Uuid {
    let image = std::env::var(variable).expect("CI-imported upgrade image");
    let raw = fs::read(format!("examples/generated-{variant}/xelab/plugin.json")).unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    let envelope =
        artifact::signature_envelope(&manifest, artifact::image_ref(&image).unwrap(), &raw)
            .unwrap();
    let submitted=api(router,"installPlugin",&[],Some(token),json!({"publisher_id":publisher,"image":image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())}),&format!("install-{variant}"),None).await;
    assert_eq!(submitted.0, 202, "{}", submitted.1);
    let result = operation(router, submitted.1["id"].as_str().unwrap(), token).await;
    result["installation_id"].as_str().unwrap().parse().unwrap()
}
async fn current(router: &Router, token: &str, deployment: &str) -> Value {
    let response = api(
        router,
        "getDeployment",
        &[("deployment_id", deployment.into())],
        Some(token),
        Value::Null,
        "upgrade-current",
        None,
    )
    .await;
    assert_eq!(response.0, 200, "{}", response.1);
    response.1
}
async fn terminal(router: &Router, token: &str, operation_id: &str, expected: &str) -> Value {
    for _ in 0..180 {
        let response = api(
            router,
            "getOperation",
            &[("operation_id", operation_id.into())],
            Some(token),
            Value::Null,
            "upgrade-poll",
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
    panic!("upgrade operation did not settle");
}
async fn ready_capabilities(
    c: &tokio_postgres::Client,
    deployment: Uuid,
    instance: Uuid,
    generation: i64,
    manifest: &Value,
) {
    let expected: std::collections::BTreeSet<_> = manifest["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cap| {
            (
                cap["contract"]["name"].as_str().unwrap().to_owned(),
                cap["contract"]["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let providers = c
        .query(
            "SELECT name,version,availability,observed_generation FROM provider_records WHERE instance_id=$1",
            &[&instance],
        )
        .await
        .unwrap();
    assert_eq!(providers.len(), expected.len());
    let mut observed = std::collections::BTreeSet::new();
    for provider in providers {
        assert_eq!(provider.get::<_, String>("availability"), "ready");
        assert_eq!(provider.get::<_, i64>("observed_generation"), generation);
        observed.insert((
            provider.get::<_, String>("name"),
            provider.get::<_, String>("version"),
        ));
    }
    assert_eq!(observed, expected, "every signed capability must be ready");
    assert_eq!(
        c.query_one("SELECT count(*) FROM provider_records p JOIN instances i ON i.id=p.instance_id WHERE i.deployment_id=$1 AND i.id<>$2 AND p.availability='ready'",&[&deployment,&instance]).await.unwrap().get::<_,i64>(0),
        0,
        "superseded instances must not retain ready capability routes"
    );
}
pub async fn exercise(
    app: &Arc<App>,
    router: &Router,
    token: &str,
    publisher: Uuid,
    signing: &SigningKey,
    deployment: &str,
) {
    for _ in 0..120 {
        if app.ready.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        app.ready.load(Ordering::SeqCst),
        "upgrade must wait for real startup readiness"
    );
    let target = install(
        router,
        token,
        publisher,
        signing,
        "upgrade",
        "XELAB_TEST_UPGRADE_IMAGE",
    )
    .await;
    let broken = install(
        router,
        token,
        publisher,
        signing,
        "failed-upgrade",
        "XELAB_TEST_FAILED_UPGRADE_IMAGE",
    )
    .await;
    let supervisor = tokio::spawn(xelab_kernel::upgrades::run(app.clone()));
    let c = app.db.connect().await.unwrap();
    let deployment_id: Uuid = deployment.parse().unwrap();
    // Capture the original installation from this deployment. The parent test
    // also installs Python plugins, whose local bindings may shadow its v1.
    let source: Uuid = c
        .query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&deployment_id],
        )
        .await
        .unwrap()
        .get(0);
    for (name, installation, key) in [
        ("upgradeDeployment", target, "actual-upgrade"),
        ("rollbackDeployment", source, "actual-rollback"),
    ] {
        let before = current(router, token, deployment).await;
        let revision = before["revision"].as_i64();
        let body =
            json!({"installation_id":installation,"strategy":"recreate","allow_downtime":true});
        let params = [("deployment_id", deployment.into())];
        let submitted = api(
            router,
            name,
            &params,
            Some(token),
            body.clone(),
            key,
            revision,
        )
        .await;
        assert_eq!(submitted.0, 202, "{}", submitted.1);
        let replay = api(router, name, &params, Some(token), body, key, revision).await;
        assert_eq!(replay.0, 202, "{}", replay.1);
        assert_eq!(replay.1["id"], submitted.1["id"]);
        let completed = terminal(
            router,
            token,
            submitted.1["id"].as_str().unwrap(),
            "succeeded",
        )
        .await;
        assert_eq!(
            completed["result"]["spec"]["installation_id"],
            json!(installation)
        );
        assert_eq!(completed["result"]["phase"], "ready");
        assert_eq!(
            c.query_one(
                "SELECT installation_id FROM deployments WHERE id=$1",
                &[&deployment_id]
            )
            .await
            .unwrap()
            .get::<_, Uuid>(0),
            installation
        );
        let rows = c
            .query(
                "SELECT id,generation FROM instances WHERE deployment_id=$1 AND phase='ready'",
                &[&deployment_id],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(app.runtime.running(rows[0].get("id")).await.unwrap());
        let manifest: Value = c
            .query_one(
                "SELECT manifest FROM installations WHERE id=$1",
                &[&installation],
            )
            .await
            .unwrap()
            .get(0);
        ready_capabilities(
            &c,
            deployment_id,
            rows[0].get("id"),
            rows[0].get("generation"),
            &manifest,
        )
        .await;
        let input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":key},"provider_deployment_id":deployment,"timeout_ms":30000});
        let called = api(
            router,
            "invokeCapability",
            &[],
            Some(token),
            input,
            &format!("{key}-invoke"),
            None,
        )
        .await;
        assert_eq!(called.0, 200, "{}", called.1);
        assert_eq!(called.1["output"]["message"], key);
    }
    let before = current(router, token, deployment).await;
    let submitted = api(
        router,
        "upgradeDeployment",
        &[("deployment_id", deployment.into())],
        Some(token),
        json!({"installation_id":broken,"strategy":"recreate","allow_downtime":true}),
        "failed-upgrade",
        before["revision"].as_i64(),
    )
    .await;
    assert_eq!(submitted.0, 202, "{}", submitted.1);
    let failed = terminal(router, token, submitted.1["id"].as_str().unwrap(), "failed").await;
    assert!(!failed["error"].is_null());
    assert!(failed["result"].is_null());
    assert_eq!(
        c.query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&deployment_id]
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0),
        source
    );
    let operation_id: Uuid = submitted.1["id"].as_str().unwrap().parse().unwrap();
    let candidate: Uuid = c
        .query_one(
            "SELECT candidate_id FROM deployment_upgrade_jobs WHERE operation_id=$1",
            &[&operation_id],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!app.runtime.running(candidate).await.unwrap());
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM reservations WHERE instance_id=$1 AND status<>'released'",
            &[&candidate]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM deployment_storage_holds WHERE deployment_id=$1",
            &[&deployment_id]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    let mut ready = false;
    for _ in 0..120 {
        if current(router, token, deployment).await["phase"] == "ready" {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        ready,
        "original installation must recover after candidate failure"
    );
    let restored=c.query_one("SELECT source_recovery,recovery_instance_id FROM deployment_upgrade_jobs WHERE operation_id=$1",&[&operation_id]).await.unwrap();
    assert_eq!(restored.get::<_, String>(0), "intent_created");
    let restored_instance = restored.get::<_, Option<Uuid>>(1).unwrap();
    assert_ne!(restored_instance, candidate);
    let restored_state = c
        .query_one(
            "SELECT phase,generation FROM instances WHERE id=$1",
            &[&restored_instance],
        )
        .await
        .unwrap();
    assert_eq!(restored_state.get::<_, String>(0), "ready");
    assert!(app.runtime.running(restored_instance).await.unwrap());
    let manifest: Value = c
        .query_one("SELECT manifest FROM installations WHERE id=$1", &[&source])
        .await
        .unwrap()
        .get(0);
    ready_capabilities(
        &c,
        deployment_id,
        restored_instance,
        restored_state.get("generation"),
        &manifest,
    )
    .await;
    supervisor.abort();
    let _ = supervisor.await;
    println!("Upgrade: actual signed v1 -> v2 -> v1, mTLS health, capability routing, idempotent intent, failed candidate stopped/reservation released and original version recovered; stateless recreate only");
}
