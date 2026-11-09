//! Real PostgreSQL metadata/authorization tests. These do not claim OCI startup
//! or formal host acceptance; the resolver must be safe before runtime launch.
use super::*;
use crate::{auth, backups::tests::Fixture, contracts, db};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio_postgres::GenericClient;
use uuid::Uuid;

fn requirement(alias: &str, target: &str, required: bool) -> Value {
    json!({"name":alias,"required":required,"target_kind":"capability","target":target,
        "version_range":">=1.0.0 <2.0.0","binding_scope":"deployment","failure_policy":"wait","features":[]})
}

fn manifest(plugin: &str, version: &str, capability: &str, capability_version: &str) -> Value {
    let schema = json!({"type":"object","additionalProperties":false});
    json!({"manifest_version":"1","plugin_id":plugin,"version":version,
        "publisher_id":Uuid::from_u128(1),"host_api_major":1,
        "platforms":[if cfg!(target_arch="aarch64") {"linux/arm64"} else {"linux/amd64"}],
        "entrypoint":["/usr/local/bin/fixture"],
        "capabilities":[{"contract":{"name":capability,"version":capability_version,"schema_digest":contracts::digest(capability_version.as_bytes())},
            "input_schema":schema,"output_schema":schema,"stream_item_schema":null,"side_effect":"none",
            "maximum_input_bytes":65536,"maximum_output_bytes":65536}],
        "dependencies":[],"requested_rules":[],"volumes":[],"routes":[],"event_topics":[],
        "minimum_resources":{"cpu_millis":100,"memory_bytes":67108864,"ephemeral_bytes":1048576,"pids":64},
        "health":{"startup_timeout_ms":60000,"interval_ms":5000,"timeout_ms":2000,"failure_threshold":3},
        "config_schema":schema,"data_schema_version":"none","upgrade_strategy":"recreate",
        "business_entry":"direct","concurrency":{"max_active_requests":4,"max_queue_requests":0,"queue_timeout_ms":100}})
}

async fn context(f: &Fixture) -> auth::Context {
    let c = f.app.db.connect().await.unwrap();
    f.app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap()
}

async fn seed(
    c: &(impl GenericClient + Sync),
    owner: Uuid,
    deployment: Uuid,
    mut manifest: Value,
    features: Value,
    ready: bool,
) -> Uuid {
    let publisher = Uuid::new_v4();
    let installation = Uuid::new_v4();
    let instance = Uuid::new_v4();
    manifest["publisher_id"] = json!(publisher);
    crate::contracts::validate("Manifest", &manifest).unwrap();
    let digest = contracts::digest(&serde_json::to_vec(&manifest).unwrap());
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'dependency fixture',$2,'fixture-only','trusted',now(),now())", &[&publisher,&publisher.to_string()]).await.unwrap();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$5,$6,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&digest,&manifest]).await.unwrap();
    let spec = json!({"installation_id":installation,"enabled":ready,"replicas":1,"display_name":"dependency fixture","limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,approval,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,now(),now())", &[&deployment,&owner,&installation,&spec,&json!({"rules":[],"egress":[]}),&if ready {"ready"} else {"disabled"}]).await.unwrap();
    if ready {
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,runtime_id,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,$3,'ready','healthy',$4,now(),now())", &[&instance,&deployment,&instance.to_string(),&manifest["minimum_resources"]]).await.unwrap();
        for capability in manifest["capabilities"].as_array().unwrap() {
            let contract = &capability["contract"];
            c.execute("INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4) ON CONFLICT(name,version) DO NOTHING", &[&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap(),&contract["schema_digest"].as_str().unwrap(),capability]).await.unwrap();
            c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready',$4,1,now())", &[&instance,&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap(),&features]).await.unwrap();
        }
    }
    installation
}

async fn bind_in_transaction(
    f: &Fixture,
    ctx: &auth::Context,
    deployment: Uuid,
) -> crate::error::Result<()> {
    let mut c = f.app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    dependency_lock(&tx).await?;
    bind(&tx, &f.app, ctx, deployment).await?;
    tx.commit().await?;
    Ok(())
}

#[test]
fn explicit_semver_intersections_reject_implicit_ranges_and_prerelease_drift() {
    assert!(version_matches("1.2.3", "1.2.3").unwrap());
    assert!(!version_matches("1.2.3", "1.2.4").unwrap());
    assert!(version_matches(">=1.0.0 <2.0.0", "1.9.9").unwrap());
    assert!(!version_matches(">=1.0.0 <2.0.0", "2.0.0").unwrap());
    assert!(!version_matches(">=1.0.0 <2.0.0", "1.2.0-rc.1").unwrap());
    assert!(version_matches("1.2.0-rc.1", "1.2.0-rc.1").unwrap());
    for invalid in [
        "^1.0.0",
        "~1.0.0",
        "*",
        "1",
        "1.0",
        ">=1.0.0 || <2.0.0",
        ">=1.0.0,<2.0.0",
        "",
        ">=1.0.0-rc.1 <2.0.0",
    ] {
        assert!(
            version_matches(invalid, "1.2.3").is_err(),
            "accepted unsupported range {invalid}"
        );
    }
}

#[test]
fn graph_rejects_self_cycles_indirect_cycles_and_excess_depth() {
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(2);
    let c = Uuid::from_u128(3);
    assert!(validate_graph(&BTreeMap::from([(a, vec![a])])).is_err());
    assert!(validate_graph(&BTreeMap::from([(a, vec![b]), (b, vec![c]), (c, vec![a])])).is_err());
    assert!(validate_graph(&BTreeMap::from([(a, vec![c]), (b, vec![c])])).is_ok());
    let allowed: BTreeMap<Uuid, Vec<Uuid>> = (1..16)
        .map(|i| (Uuid::from_u128(i), vec![Uuid::from_u128(i + 1)]))
        .collect();
    assert!(
        validate_graph(&allowed).is_ok(),
        "sixteen levels must be accepted"
    );
    let deep: BTreeMap<Uuid, Vec<Uuid>> = (1..18)
        .map(|i| (Uuid::from_u128(i), vec![Uuid::from_u128(i + 1)]))
        .collect();
    assert!(
        validate_graph(&deep).is_err(),
        "deep graphs must be bounded"
    );
}

#[test]
fn manifest_aliases_and_features_are_unambiguous() {
    let mut m = manifest(
        "org.test.consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    m["dependencies"] = json!([requirement("lookup", "org.test.lookup", true)]);
    validate_manifest(&m).unwrap();
    m["dependencies"] = json!([
        requirement("lookup", "org.test.lookup", true),
        requirement("lookup", "org.test.other", false)
    ]);
    assert!(validate_manifest(&m).is_err());
    m["dependencies"] = json!([requirement("lookup", "org.test.lookup", true)]);
    m["dependencies"][0]["features"] = json!(["vector", "vector"]);
    assert!(validate_manifest(&m).is_err());
}

#[tokio::test]
async fn postgres_resolver_is_deterministic_filters_features_and_does_not_reserve_shared_resources()
{
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let smaller = Uuid::from_u128(100);
    let larger = Uuid::from_u128(200);
    seed(
        &c,
        ctx.subject,
        smaller,
        manifest("org.test.first", "7.0.0", "org.test.lookup", "1.4.0"),
        json!(["vector"]),
        true,
    )
    .await;
    seed(
        &c,
        ctx.subject,
        larger,
        manifest("org.test.second", "2.0.0", "org.test.lookup", "1.4.0"),
        json!(["vector"]),
        true,
    )
    .await;
    let excluded = Uuid::from_u128(50);
    seed(
        &c,
        ctx.subject,
        excluded,
        manifest("org.test.excluded", "1.0.0", "org.test.lookup", "1.4.0"),
        json!([]),
        true,
    )
    .await;
    let mut consumer = manifest(
        "org.test.consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    consumer["dependencies"] = json!([requirement("lookup", "org.test.lookup", true)]);
    consumer["dependencies"][0]["features"] = json!(["vector"]);
    let deployment = Uuid::from_u128(300);
    let installation = seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    let first = plan(&c, &ctx, deployment, installation).await.unwrap();
    let second = plan(&c, &ctx, deployment, installation).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first["bindings"][0]["deployment_id"], json!(smaller));
    assert_eq!(first["bindings"][0]["contract"]["version"], "1.4.0");
    assert_eq!(first["required_cpu_millis"], 0);
    assert_eq!(first["required_memory_bytes"], 0);
    assert_eq!(first["conflicts"], json!([]));
    assert_eq!(
        c.query_one("SELECT count(*) FROM reservations", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn postgres_plugin_range_uses_plugin_version_and_optional_missing_does_not_block() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Uuid::from_u128(101);
    seed(
        &c,
        ctx.subject,
        provider,
        manifest("org.test.plugin", "5.1.0", "org.test.lookup", "1.3.0"),
        json!([]),
        true,
    )
    .await;
    let mut consumer = manifest(
        "org.test.consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    let mut plugin = requirement("plugin", "org.test.plugin", true);
    plugin["target_kind"] = json!("plugin");
    plugin["version_range"] = json!(">=5.0.0 <6.0.0");
    consumer["dependencies"] = json!([plugin, requirement("optional", "org.test.absent", false)]);
    let deployment = Uuid::from_u128(301);
    let installation = seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    let result = plan(&c, &ctx, deployment, installation).await.unwrap();
    assert_eq!(result["conflicts"], json!([]));
    assert_eq!(result["bindings"].as_array().unwrap().len(), 1);
    assert_eq!(result["bindings"][0]["deployment_id"], json!(provider));
    assert!(result["bindings"][0]["contract"].is_null());
    c.execute("UPDATE installations SET manifest=jsonb_set(manifest,'{dependencies,0,version_range}','\"1.3.0\"'::jsonb) WHERE id=$1",&[&installation]).await.unwrap();
    let mismatch = plan(&c, &ctx, deployment, installation).await.unwrap();
    assert!(
        !mismatch["conflicts"].as_array().unwrap().is_empty(),
        "capability version must not satisfy a plugin version range"
    );
}

#[tokio::test]
async fn postgres_binding_stays_fixed_and_enable_session_is_not_execution_authority() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let original = Uuid::from_u128(100);
    let replacement = Uuid::from_u128(200);
    let original_installation = seed(
        &c,
        ctx.subject,
        original,
        manifest("org.test.first", "1.0.0", "org.test.lookup", "1.0.0"),
        json!(["lookup-v1"]),
        true,
    )
    .await;
    seed(
        &c,
        ctx.subject,
        replacement,
        manifest("org.test.second", "1.0.0", "org.test.lookup", "1.0.0"),
        json!(["lookup-v1"]),
        true,
    )
    .await;
    let mut consumer = manifest(
        "org.test.consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    consumer["dependencies"] = json!([requirement("lookup", "org.test.lookup", true)]);
    consumer["dependencies"][0]["features"] = json!(["lookup-v1"]);
    let deployment = Uuid::from_u128(300);
    seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original]).await.unwrap();
    assert!(check_ready(&c, &f.app, deployment).await.is_err());
    let bound:Uuid=c.query_one("SELECT provider_deployment_id FROM dependency_bindings WHERE consumer_deployment_id=$1",&[&deployment]).await.unwrap().get(0);
    assert_eq!(
        bound, original,
        "provider failure cannot silently rebind to a healthy alternative"
    );
    c.execute("UPDATE provider_records SET availability='ready',observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original]).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    c.execute("UPDATE provider_records SET features='[]'::jsonb WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original]).await.unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "removing required advertised features must invalidate a fixed binding"
    );
    c.execute("UPDATE provider_records SET features='[\"lookup-v1\"]'::jsonb,observed_generation=999 WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original]).await.unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "stale health generation cannot make the original provider ready"
    );
    c.execute("UPDATE provider_records SET observed_generation=1,observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original]).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    // Even a valid schema/contract cannot authorize a changed fixed installation.
    c.execute("UPDATE deployments SET installation_id=(SELECT installation_id FROM deployments WHERE id=$2) WHERE id=$1",&[&original,&replacement]).await.unwrap();
    assert!(check_ready(&c, &f.app, deployment).await.is_err());
    c.execute(
        "UPDATE deployments SET installation_id=$2 WHERE id=$1",
        &[&original, &original_installation],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&ctx.credential],
    )
    .await
    .unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    // A healthy fixed route survives its creator's session expiry/revocation.
    // Actual Host caller authority is independently tested below.
}

#[tokio::test]
async fn postgres_required_missing_bind_is_atomic_and_strong_cycles_never_commit() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let mut missing = manifest("org.test.missing", "1.0.0", "org.test.missing.run", "1.0.0");
    missing["dependencies"] = json!([requirement("required", "org.test.absent", true)]);
    let missing_id = Uuid::from_u128(300);
    let installation = seed(&c, ctx.subject, missing_id, missing, json!([]), false).await;
    let result = plan(&c, &ctx, missing_id, installation).await.unwrap();
    assert!(!result["conflicts"].as_array().unwrap().is_empty());
    assert!(bind_in_transaction(&f, &ctx, missing_id).await.is_err());
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM dependency_bindings WHERE consumer_deployment_id=$1",
            &[&missing_id]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
    let a = Uuid::from_u128(400);
    let b = Uuid::from_u128(500);
    let mut am = manifest("org.test.a", "1.0.0", "org.test.a.run", "1.0.0");
    let mut bm = manifest("org.test.b", "1.0.0", "org.test.b.run", "1.0.0");
    am["dependencies"] = json!([requirement("b", "org.test.b.run", true)]);
    bm["dependencies"] = json!([requirement("a", "org.test.a.run", true)]);
    seed(&c, ctx.subject, a, am, json!([]), true).await;
    seed(&c, ctx.subject, b, bm, json!([]), true).await;
    let left = bind_in_transaction(&f, &ctx, a);
    let right = bind_in_transaction(&f, &ctx, b);
    let (left, right) = tokio::join!(left, right);
    assert!(
        !(left.is_ok() && right.is_ok()),
        "concurrent opposite strong edges must not both commit"
    );
    assert!(
        c.query_one(
            "SELECT count(*) FROM dependency_bindings WHERE consumer_deployment_id=ANY($1)",
            &[&vec![a, b]]
        )
        .await
        .unwrap()
        .get::<_, i64>(0)
            <= 1
    );
}

async fn dependency_request(f: &Fixture, body: Value, key: &str, token: &str) -> (u16, Value) {
    use axum::{
        body::{to_bytes, Body},
        extract::ConnectInfo,
        http::Request,
    };
    use tower::ServiceExt;
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/platform/v1/dependency-plans")
        .header(
            "host",
            crate::plugin_http_gateway::core_authority(&f.app.config).unwrap(),
        )
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("idempotency-key", key)
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:54321".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = crate::api::router(f.app.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn postgres_public_dependency_plan_is_schema_valid_idempotent_and_rejects_revoked_replay() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Uuid::from_u128(100);
    seed(
        &c,
        ctx.subject,
        provider,
        manifest(
            "org.test.public-provider",
            "1.0.0",
            "org.test.lookup",
            "1.0.0",
        ),
        json!([]),
        true,
    )
    .await;
    let deployment = Uuid::from_u128(300);
    let mut consumer = manifest(
        "org.test.public-consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    consumer["dependencies"] = json!([requirement("lookup", "org.test.lookup", true)]);
    let installation = seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    let body = json!({"deployment_id":deployment,"installation_id":installation});
    let first = dependency_request(&f, body.clone(), "public-plan", &f.token).await;
    assert_eq!(first.0, 200, "{}", first.1);
    contracts::validate("DependencyPlan", &first.1).unwrap();
    assert_eq!(first.1["bindings"][0]["deployment_id"], json!(provider));
    let replay = dependency_request(&f, body.clone(), "public-plan", &f.token).await;
    assert_eq!(replay, first);
    let mut invalid = body.clone();
    invalid["provider_url"] = json!("https://forged.example.invalid/");
    assert_eq!(
        dependency_request(&f, invalid, "invalid-field", &f.token)
            .await
            .0,
        400
    );
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&ctx.credential],
    )
    .await
    .unwrap();
    assert_eq!(
        dependency_request(&f, body, "public-plan", &f.token)
            .await
            .0,
        401,
        "cached preview must not bypass credential revocation"
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM dependency_bindings WHERE consumer_deployment_id=$1",
            &[&deployment]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0,
        "preview must not install bindings"
    );
}

#[tokio::test]
async fn postgres_plugin_only_binding_needs_no_capability_and_pins_config_and_generation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Uuid::from_u128(110);
    let mut plugin_only = manifest("org.test.plugin-only", "5.2.0", "org.test.unused", "1.0.0");
    plugin_only["capabilities"] = json!([]);
    seed(&c, ctx.subject, provider, plugin_only, json!([]), true).await;
    c.execute(
        "UPDATE instances SET last_heartbeat_at=now() WHERE deployment_id=$1",
        &[&provider],
    )
    .await
    .unwrap();
    let deployment = Uuid::from_u128(310);
    let mut consumer = manifest(
        "org.test.plugin-consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    let mut dependency = requirement("plugin", "org.test.plugin-only", true);
    dependency["target_kind"] = json!("plugin");
    dependency["version_range"] = json!("5.2.0");
    consumer["dependencies"] = json!([dependency]);
    let installation = seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    let preview = plan(&c, &ctx, deployment, installation).await.unwrap();
    assert_eq!(preview["conflicts"], json!([]));
    assert_eq!(preview["bindings"][0]["deployment_id"], json!(provider));
    assert!(
        preview["bindings"][0]["contract"].is_null(),
        "a lifecycle dependency must not invent a capability contract"
    );
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    let original_spec: Value = c
        .query_one("SELECT spec FROM deployments WHERE id=$1", &[&provider])
        .await
        .unwrap()
        .get(0);
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config}','{\"new\":true}'::jsonb) WHERE id=$1",&[&provider]).await.unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "configuration drift requires explicit binding"
    );
    c.execute(
        "UPDATE deployments SET spec=$2 WHERE id=$1",
        &[&provider, &original_spec],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE instances SET generation=2,last_heartbeat_at=now() WHERE deployment_id=$1",
        &[&provider],
    )
    .await
    .unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "healthy new instance generation cannot reuse the old binding"
    );
    c.execute(
        "UPDATE instances SET generation=1,last_heartbeat_at=now() WHERE deployment_id=$1",
        &[&provider],
    )
    .await
    .unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    c.execute("UPDATE dependency_bindings SET required_features='[\"unavailable\"]'::jsonb WHERE consumer_deployment_id=$1",&[&deployment]).await.unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "plugin-only binding cannot fabricate required features"
    );
    c.execute("UPDATE dependency_bindings SET required_features='[]'::jsonb WHERE consumer_deployment_id=$1",&[&deployment]).await.unwrap();
    c.execute("UPDATE installations SET manifest=jsonb_set(manifest,'{dependencies,0,features}','[\"unavailable\"]'::jsonb) WHERE id=$1",&[&installation]).await.unwrap();
    let missing_feature = plan(&c, &ctx, deployment, installation).await.unwrap();
    assert!(!missing_feature["conflicts"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn postgres_fail_policy_latches_until_explicit_authorized_rebind() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Uuid::from_u128(120);
    seed(
        &c,
        ctx.subject,
        provider,
        manifest(
            "org.test.fail-provider",
            "1.0.0",
            "org.test.lookup",
            "1.0.0",
        ),
        json!([]),
        true,
    )
    .await;
    let deployment = Uuid::from_u128(320);
    let mut consumer = manifest(
        "org.test.fail-consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    let mut dependency = requirement("lookup", "org.test.lookup", true);
    dependency["failure_policy"] = json!("fail");
    consumer["dependencies"] = json!([dependency]);
    seed(&c, ctx.subject, deployment, consumer, json!([]), false).await;
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&provider]).await.unwrap();
    assert!(check_ready(&c, &f.app, deployment).await.is_err());
    assert!(
        c.query_one(
            "SELECT failure_latched FROM dependency_bindings WHERE consumer_deployment_id=$1",
            &[&deployment]
        )
        .await
        .unwrap()
        .get::<_, bool>(0),
        "autocommit latches a fail-policy violation durably"
    );
    c.execute("UPDATE provider_records SET availability='ready',observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&provider]).await.unwrap();
    assert!(
        check_ready(&c, &f.app, deployment).await.is_err(),
        "provider recovery must not clear fail policy"
    );
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    check_ready(&c, &f.app, deployment).await.unwrap();
    assert!(!c
        .query_one(
            "SELECT failure_latched FROM dependency_bindings WHERE consumer_deployment_id=$1",
            &[&deployment]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));
}

async fn seed_parent_call(
    c: &(impl GenericClient + Sync),
    ctx: &auth::Context,
    consumer: Uuid,
) -> Uuid {
    let instance: Uuid = c
        .query_one(
            "SELECT id FROM instances WHERE deployment_id=$1",
            &[&consumer],
        )
        .await
        .unwrap()
        .get(0);
    let contract:Value=c.query_one("SELECT n.manifest->'capabilities'->0->'contract' FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1",&[&consumer]).await.unwrap().get(0);
    let call = Uuid::new_v4();
    c.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'running',now()+interval '5 minutes',now())",&[&call,&ctx.subject,&ctx.credential,&instance,&contract,&contracts::digest(b"{}"),&call.to_string()]).await.unwrap();
    call
}

#[tokio::test]
async fn postgres_host_request_handle_is_call_bound_and_refuses_generation_and_config_drift() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Uuid::from_u128(130);
    let provider_manifest = manifest(
        "org.test.host-provider",
        "1.0.0",
        "org.test.lookup",
        "1.0.0",
    );
    let contract = provider_manifest["capabilities"][0]["contract"].clone();
    seed(
        &c,
        ctx.subject,
        provider,
        provider_manifest,
        json!([]),
        true,
    )
    .await;
    let deployment = Uuid::from_u128(330);
    let mut consumer = manifest(
        "org.test.host-consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    let mut dependency = requirement("lookup", "org.test.lookup", true);
    dependency["binding_scope"] = json!("request");
    consumer["dependencies"] = json!([dependency]);
    seed(&c, ctx.subject, deployment, consumer, json!([]), true).await;
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    let call = seed_parent_call(&c, &ctx, deployment).await;
    let other_call = seed_parent_call(&c, &ctx, deployment).await;
    let resolved = resolve_host(&c, &ctx, deployment, call, &call.to_string(), &contract)
        .await
        .unwrap();
    let binding = contracts::id(&resolved, "binding_id").unwrap();
    assert_eq!(resolved["provider_deployment_id"], json!(provider));
    assert_eq!(
        check_host(&c, &ctx, deployment, call, binding, &contract)
            .await
            .unwrap(),
        provider
    );
    assert!(
        check_host(&c, &ctx, deployment, other_call, binding, &contract)
            .await
            .is_err(),
        "request binding cannot be reused for another parent Call"
    );
    let again = resolve_host(&c, &ctx, deployment, call, &call.to_string(), &contract)
        .await
        .unwrap();
    assert_eq!(
        resolved["binding_id"], again["binding_id"],
        "same active parent/scope retains its fixed opaque handle"
    );
    assert!(
        resolve_host(
            &c,
            &ctx,
            deployment,
            call,
            &deployment.to_string(),
            &contract
        )
        .await
        .is_err(),
        "request scope cannot masquerade as deployment scope"
    );
    let original_spec: Value = c
        .query_one("SELECT spec FROM deployments WHERE id=$1", &[&provider])
        .await
        .unwrap()
        .get(0);
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config}','{\"changed\":true}'::jsonb) WHERE id=$1",&[&provider]).await.unwrap();
    assert!(check_host(&c, &ctx, deployment, call, binding, &contract)
        .await
        .is_err());
    c.execute(
        "UPDATE deployments SET spec=$2 WHERE id=$1",
        &[&provider, &original_spec],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE instances SET generation=2 WHERE deployment_id=$1",
        &[&provider],
    )
    .await
    .unwrap();
    c.execute("UPDATE provider_records SET observed_generation=2,observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&provider]).await.unwrap();
    assert!(
        check_host(&c, &ctx, deployment, call, binding, &contract)
            .await
            .is_err(),
        "new provider generation must not inherit the old opaque handle"
    );
    c.execute(
        "UPDATE instances SET generation=1 WHERE deployment_id=$1",
        &[&provider],
    )
    .await
    .unwrap();
    c.execute("UPDATE provider_records SET observed_generation=1,observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&provider]).await.unwrap();
    check_host(&c, &ctx, deployment, call, binding, &contract)
        .await
        .unwrap();
    c.execute("UPDATE calls SET state='unknown' WHERE id=$1", &[&call])
        .await
        .unwrap();
    assert!(
        check_host(&c, &ctx, deployment, call, binding, &contract)
            .await
            .is_err(),
        "terminal/unknown parent cannot reuse a downstream binding"
    );
}

#[tokio::test]
async fn postgres_optional_request_dependency_resolves_lazily_without_reenable_and_checks_current_caller(
) {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let c = f.app.db.connect().await.unwrap();
    let deployment = Uuid::from_u128(350);
    let mut consumer = manifest(
        "org.test.lazy-consumer",
        "1.0.0",
        "org.test.consumer.run",
        "1.0.0",
    );
    let mut dependency = requirement("optional", "org.test.lazy-provider", false);
    dependency["binding_scope"] = json!("request");
    dependency["failure_policy"] = json!("degrade");
    consumer["dependencies"] = json!([dependency]);
    seed(&c, ctx.subject, deployment, consumer, json!([]), true).await;
    bind_in_transaction(&f, &ctx, deployment).await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM dependency_bindings WHERE consumer_deployment_id=$1",
            &[&deployment]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0,
        "missing optional request provider does not need a placeholder binding"
    );
    check_ready(&c, &f.app, deployment).await.unwrap();
    let initially_available = Uuid::from_u128(220);
    let provider_manifest = manifest(
        "org.test.lazy-later",
        "1.0.0",
        "org.test.lazy-provider",
        "1.0.0",
    );
    let contract = provider_manifest["capabilities"][0]["contract"].clone();
    seed(
        &c,
        ctx.subject,
        initially_available,
        provider_manifest,
        json!([]),
        true,
    )
    .await;
    let first_parent = seed_parent_call(&c, &ctx, deployment).await;
    let first = resolve_host(
        &c,
        &ctx,
        deployment,
        first_parent,
        &first_parent.to_string(),
        &contract,
    )
    .await
    .unwrap();
    let first_binding = contracts::id(&first, "binding_id").unwrap();
    assert_eq!(first["provider_deployment_id"], json!(initially_available));
    assert_eq!(
        check_host(&c, &ctx, deployment, first_parent, first_binding, &contract)
            .await
            .unwrap(),
        initially_available
    );
    let preferred_later = Uuid::from_u128(120);
    seed(
        &c,
        ctx.subject,
        preferred_later,
        manifest(
            "org.test.lazy-preferred",
            "1.0.0",
            "org.test.lazy-provider",
            "1.0.0",
        ),
        json!([]),
        true,
    )
    .await;
    let same_parent = resolve_host(
        &c,
        &ctx,
        deployment,
        first_parent,
        &first_parent.to_string(),
        &contract,
    )
    .await
    .unwrap();
    assert_eq!(same_parent["binding_id"], first["binding_id"]);
    assert_eq!(
        same_parent["provider_deployment_id"],
        json!(initially_available),
        "a newly preferred provider cannot move an admitted request's fixed handle"
    );
    let second_parent = seed_parent_call(&c, &ctx, deployment).await;
    let second = resolve_host(
        &c,
        &ctx,
        deployment,
        second_parent,
        &second_parent.to_string(),
        &contract,
    )
    .await
    .unwrap();
    assert_eq!(
        second["provider_deployment_id"],
        json!(preferred_later),
        "new requests may resolve a newly available deterministic provider"
    );
    assert_ne!(second["binding_id"], first["binding_id"]);
    let state = c
        .query_one(
            "SELECT spec->>'enabled',revision FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
    assert_eq!(state.get::<_, String>(0), "true");
    assert_eq!(
        state.get::<_, i64>(1),
        1,
        "lazy request discovery does not require a disable/enable mutation"
    );
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&ctx.credential],
    )
    .await
    .unwrap();
    assert!(
        resolve_host(
            &c,
            &ctx,
            deployment,
            first_parent,
            &first_parent.to_string(),
            &contract
        )
        .await
        .is_err(),
        "a cached opaque handle is not a replacement for the actual caller's live source chain"
    );
    assert!(
        check_host(&c, &ctx, deployment, first_parent, first_binding, &contract)
            .await
            .is_err()
    );
    check_ready(&c, &f.app, deployment).await.unwrap();
}

#[tokio::test]
async fn postgres_staged_upgrade_keeps_exact_provider_and_publishes_without_rediscovery() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let ctx = context(&f).await;
    let mut c = f.app.db.connect().await.unwrap();
    let original_provider = Uuid::from_u128(240);
    seed(
        &c,
        ctx.subject,
        original_provider,
        manifest(
            "org.test.upgrade-provider",
            "1.0.0",
            "org.test.upgrade-lookup",
            "1.0.0",
        ),
        json!([]),
        true,
    )
    .await;
    let consumer = Uuid::from_u128(340);
    let mut source_manifest = manifest(
        "org.test.upgrade-consumer",
        "1.0.0",
        "org.test.upgrade.run",
        "1.0.0",
    );
    source_manifest["dependencies"] =
        json!([requirement("lookup", "org.test.upgrade-lookup", true)]);
    seed(&c, ctx.subject, consumer, source_manifest, json!([]), false).await;
    bind_in_transaction(&f, &ctx, consumer).await.unwrap();
    let target_dummy = Uuid::from_u128(440);
    let mut target_manifest = manifest(
        "org.test.upgrade-consumer",
        "2.0.0",
        "org.test.upgrade.run",
        "1.0.0",
    );
    target_manifest["dependencies"] =
        json!([requirement("lookup", "org.test.upgrade-lookup", true)]);
    let target = seed(
        &c,
        ctx.subject,
        target_dummy,
        target_manifest,
        json!([]),
        false,
    )
    .await;
    let operation = db::operation(
        &c,
        ctx.subject,
        "upgradeDeployment",
        Some(consumer.to_string()),
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    stage_upgrade(&tx, &ctx, consumer, target, operation)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let planned: Value = c
        .query_one(
            "SELECT bindings FROM dependency_upgrade_plans WHERE operation_id=$1",
            &[&operation],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(planned[0]["deployment_id"], json!(original_provider));
    let later_preferred = Uuid::from_u128(140);
    seed(
        &c,
        ctx.subject,
        later_preferred,
        manifest(
            "org.test.upgrade-alternative",
            "1.0.0",
            "org.test.upgrade-lookup",
            "1.0.0",
        ),
        json!([]),
        true,
    )
    .await;
    check_upgrade(&c, &f.app, &ctx, operation).await.unwrap();
    let spec: Value = c
        .query_one(
            "SELECT spec FROM deployments WHERE id=$1",
            &[&original_provider],
        )
        .await
        .unwrap()
        .get(0);
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config}','{\"changed\":true}'::jsonb) WHERE id=$1",&[&original_provider]).await.unwrap();
    assert!(
        check_upgrade(&c, &f.app, &ctx, operation).await.is_err(),
        "staged provider config drift must fail despite a healthy alternative"
    );
    c.execute(
        "UPDATE deployments SET spec=$2 WHERE id=$1",
        &[&original_provider, &spec],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE instances SET generation=2 WHERE deployment_id=$1",
        &[&original_provider],
    )
    .await
    .unwrap();
    c.execute("UPDATE provider_records SET observed_generation=2,observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original_provider]).await.unwrap();
    assert!(
        check_upgrade(&c, &f.app, &ctx, operation).await.is_err(),
        "staged provider generation cannot be rediscovered"
    );
    c.execute(
        "UPDATE instances SET generation=1 WHERE deployment_id=$1",
        &[&original_provider],
    )
    .await
    .unwrap();
    c.execute("UPDATE provider_records SET observed_generation=1,observed_at=now() WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&original_provider]).await.unwrap();
    let tx = c.transaction().await.unwrap();
    publish_upgrade(&tx, &f.app, &ctx, consumer, operation)
        .await
        .unwrap();
    tx.execute("UPDATE deployments SET installation_id=$2,spec=jsonb_set(spec,'{installation_id}',to_jsonb($2::uuid)),revision=revision+1 WHERE id=$1",&[&consumer,&target]).await.unwrap();
    tx.commit().await.unwrap();
    let published=c.query_one("SELECT consumer_installation_id,provider_deployment_id FROM dependency_bindings WHERE consumer_deployment_id=$1 AND bound_call IS NULL",&[&consumer]).await.unwrap();
    assert_eq!(published.get::<_, Option<Uuid>>(0), Some(target));
    assert_eq!(
        published.get::<_, Uuid>(1),
        original_provider,
        "publication must use the staged pins, not the now-preferred provider"
    );
    check_ready(&c, &f.app, consumer).await.unwrap();
    let before:Value=c.query_one("SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM dependency_bindings b WHERE consumer_deployment_id=$1",&[&consumer]).await.unwrap().get(0);
    let mut bad_manifest = manifest(
        "org.test.upgrade-consumer",
        "3.0.0",
        "org.test.upgrade.run",
        "1.0.0",
    );
    bad_manifest["dependencies"] =
        json!([requirement("missing", "org.test.no-upgrade-provider", true)]);
    let bad_target = seed(
        &c,
        ctx.subject,
        Uuid::from_u128(540),
        bad_manifest,
        json!([]),
        false,
    )
    .await;
    let failed = db::operation(
        &c,
        ctx.subject,
        "upgradeDeployment",
        Some(consumer.to_string()),
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    assert!(stage_upgrade(&tx, &ctx, consumer, bad_target, failed)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    let after:Value=c.query_one("SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM dependency_bindings b WHERE consumer_deployment_id=$1",&[&consumer]).await.unwrap().get(0);
    assert_eq!(
        after, before,
        "failed staged plan must preserve all existing deployment bindings"
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM dependency_upgrade_plans WHERE operation_id=$1",
            &[&failed]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
}
