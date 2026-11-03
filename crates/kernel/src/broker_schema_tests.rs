//! Actual PostgreSQL call journal; metadata-only provider, no OCI/migration or
//! physical format claim. The missing runtime deliberately makes dispatch unknown.
use super::*;
use crate::{auth, backups::tests::Fixture};

#[tokio::test]
async fn actual_postgres_idempotent_schema_call_never_replays_unknown_dispatch() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let app = &f.app;
    let mut c = app.db.connect().await.unwrap();
    let peer = "127.0.0.1".parse().unwrap();
    let ctx = app
        .auth
        .authenticate(&c, &f.token, peer, Uuid::new_v4())
        .await
        .unwrap();
    let publisher: Uuid = c
        .query_one("SELECT id FROM publishers LIMIT 1", &[])
        .await
        .unwrap()
        .get(0);
    let installation = Uuid::new_v4();
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    manifest["requested_rules"] = json!([]);
    manifest["publisher_id"] = json!(publisher);
    manifest["data_schema_version"] = json!("1");
    manifest["volumes"] = json!([{"name":"data","mount_path":"/data","access":"rw","scope":"deployment","minimum_bytes":1048576}]);
    manifest["capabilities"][0]["side_effect"] = json!("idempotent");
    crate::artifact::validate_runtime_profile(&manifest).unwrap();
    assert!(
        crate::artifact::validate_profile(&manifest).is_err(),
        "public installation stays closed"
    );
    let cap = &manifest["capabilities"][0];
    let contract = cap["contract"].clone();
    let hash = digest(&serde_json::to_vec(&manifest).unwrap());
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$5,$6,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&manifest["version"].as_str().unwrap(),&publisher,&hash,&manifest]).await.unwrap();
    let spec = json!({"installation_id":installation,"display_name":"idempotent call metadata fixture","enabled":true,"replicas":1,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())", &[&deployment,&ctx.subject,&installation,&spec]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,2,0,'ready','alive',$3,now(),now())", &[&instance,&deployment,&spec["limits"]]).await.unwrap();
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
    c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',2,now())", &[&instance,&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap()]).await.unwrap();
    let request = json!({"contract":contract,"input":{"message":"one durable request"},"provider_deployment_id":deployment,"timeout_ms":1000});
    let key = "schema-unknown-call";
    for variant in ["unversioned", "other_contract", "other_schema"] {
        let mut wrong = manifest.clone();
        match variant {
            "unversioned" => wrong["data_schema_version"] = json!("none"),
            "other_contract" => {
                wrong["capabilities"][0]["contract"]["schema_digest"] =
                    json!(digest(b"other contract"))
            }
            "other_schema" => wrong["capabilities"][0]["input_schema"] = json!({"type":"object"}),
            _ => unreachable!(),
        }
        c.execute(
            "UPDATE installations SET manifest=$2 WHERE id=$1",
            &[&installation, &wrong],
        )
        .await
        .unwrap();
        assert!(
            invoke(app, &f.token, peer, Uuid::new_v4(), &request, key)
                .await
                .is_err(),
            "{variant}"
        );
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM calls WHERE provider_instance_id=$1",
                &[&instance]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0,
            "invalid provider semantics must not reserve or dispatch a call"
        );
    }
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    assert!(invoke(app, &f.token, peer, Uuid::new_v4(), &request, key)
        .await
        .is_err());
    let call = c
        .query_one(
            "SELECT id,state FROM calls WHERE provider_instance_id=$1",
            &[&instance],
        )
        .await
        .unwrap();
    let call_id: Uuid = call.get("id");
    assert_eq!(call.get::<_, String>("state"), "unknown");
    let charged: i64 = c
        .query_one(
            "SELECT concurrent_calls_used FROM quotas WHERE account_id=$1",
            &[&ctx.subject],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        charged, 1,
        "an unknown provider outcome must retain its slot until fenced cleanup"
    );
    let replay = invoke(app, &f.token, peer, Uuid::new_v4(), &request, key)
        .await
        .unwrap_err();
    assert_eq!(replay.code, "OUTCOME_UNKNOWN");
    assert_eq!(replay.resource_id, Some(call_id.to_string()));
    let mut changed = request.clone();
    changed["input"]["message"] = json!("a different write");
    assert_eq!(
        invoke(app, &f.token, peer, Uuid::new_v4(), &changed, key)
            .await
            .unwrap_err()
            .code,
        "IDEMPOTENCY_CONFLICT"
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM calls WHERE provider_instance_id=$1",
            &[&instance]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT concurrent_calls_used FROM quotas WHERE account_id=$1",
            &[&ctx.subject]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        charged
    );
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    tx.execute(
        "UPDATE grants SET status='revoked' WHERE subject_id=$1 AND scope->>'kind'='capability'",
        &[&ctx.subject],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        invoke(app, &f.token, peer, Uuid::new_v4(), &request, key)
            .await
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert!(auth::authorize(
        &c,
        &ctx,
        "capability.invoke",
        &Resource::new(
            "capability",
            format!(
                "{}@{}",
                contract["name"].as_str().unwrap(),
                contract["version"].as_str().unwrap()
            ),
            None
        )
    )
    .await
    .is_err());
}
