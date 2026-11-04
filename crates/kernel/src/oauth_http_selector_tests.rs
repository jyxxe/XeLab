//! Fixed HTTP Skill deployment regression using the parent test's real
//! PostgreSQL/main router and independent mTLS echo process. Extra A/C targets
//! are explicit metadata fixtures, not OCI execution or admission evidence.
use super::*;

async fn ledger(c: &tokio_postgres::Client, owner: Uuid) -> Value {
    let row = c.query_one("SELECT (SELECT count(*) FROM calls) AS calls,(SELECT count(*) FROM reservations WHERE kind='call' AND status<>'released') AS active,(SELECT count(*) FROM call_credential_charges) AS charges,concurrent_calls_used,daily_calls_used FROM quotas WHERE account_id=$1",&[&owner]).await.unwrap();
    json!({"calls":row.get::<_,i64>("calls"),"active":row.get::<_,i64>("active"),
        "charges":row.get::<_,i64>("charges"),"concurrent":row.get::<_,i64>("concurrent_calls_used"),
        "daily":row.get::<_,i64>("daily_calls_used")})
}

pub(super) async fn fixed_http_skill_deployment_selector(f: &Component, skill: &str) {
    let c = f.app.db.connect().await.unwrap();
    let params = [("skill_id", skill.to_owned())];
    let unavailable = Uuid::new_v4();
    let other_deployment = Uuid::new_v4();
    let other_installation = Uuid::new_v4();
    let mut other_manifest = f.manifest.clone();
    other_manifest["version"] = json!("2.0.0");
    let raw_manifest = serde_json::to_vec(&other_manifest).unwrap();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) SELECT $1,plugin_id,'2.0.0',publisher_id,$2,$3,$4,'installed',now(),now() FROM installations WHERE id=$5",&[&other_installation,&digest(b"selector other installation metadata"),&digest(&raw_manifest),&other_manifest,&f.installation]).await.unwrap();
    let openapi: Vec<u8> = c
        .query_one(
            "SELECT raw FROM plugin_http_contracts WHERE installation_id=$1",
            &[&f.installation],
        )
        .await
        .unwrap()
        .get(0);
    crate::http_contracts::store(&c, other_installation, &other_manifest, Some(&openapi))
        .await
        .unwrap();
    let template: Value = c
        .query_one("SELECT spec FROM deployments WHERE id=$1", &[&f.deployment])
        .await
        .unwrap()
        .get(0);
    for (deployment, installation, enabled, key) in [
        (unavailable, f.installation, false, "selector-A"),
        (other_deployment, other_installation, true, "selector-C"),
    ] {
        let mut spec = template.clone();
        spec["installation_id"] = json!(installation);
        spec["enabled"] = json!(false);
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'disabled',now(),now())",&[&deployment,&f.owner,&installation,&spec]).await.unwrap();
        let approved = management(
            &f.router,
            "approvePlugin",
            &[("deployment_id", deployment.to_string())],
            &f.token,
            json!({"rules":[],"egress":[]}),
            &format!("{key}-approve"),
            Some(1),
        )
        .await;
        assert_eq!(approved.0, 200, "{}", approved.1);
        let phase = if enabled { "ready" } else { "disabled" };
        c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{enabled}',$2),phase=$3,observed_generation=1 WHERE id=$1",&[&deployment,&json!(enabled),&phase]).await.unwrap();
        // Deliberately fresh metadata isolates selector routing from a stale
        // heartbeat failure. No process or fabricated mTLS identity is created.
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive',$3,now(),now(),now())",&[&Uuid::new_v4(),&deployment,&spec["limits"]]).await.unwrap();
    }
    let before = ledger(&c, f.owner).await;
    let input = json!({"input":{"message":"actual fixed B HTTP Skill"},"timeout_ms":5000,"idempotency_key":"fixed-HTTP-business-key"});
    for (index, deployment) in [unavailable, other_deployment].into_iter().enumerate() {
        let mut selected = input.clone();
        selected["provider_deployment_id"] = json!(deployment);
        let refused = management(
            &f.router,
            "invokeSkill",
            &params,
            &f.token,
            selected,
            &format!("selector-unavailable-{index}"),
            None,
        )
        .await;
        assert_eq!(refused.0, 503, "{}", refused.1);
        assert_eq!(refused.1["error"]["code"], "NOT_READY");
        assert_eq!(
            ledger(&c, f.owner).await,
            before,
            "selector cannot fall back to healthy B or charge rejected work"
        );
    }
    let mut selected = input.clone();
    selected["provider_deployment_id"] = json!(f.deployment);
    let called = management(
        &f.router,
        "invokeSkill",
        &params,
        &f.token,
        selected.clone(),
        "selector-success-stable-key",
        None,
    )
    .await;
    assert_eq!(called.0, 200, "{}", called.1);
    assert_eq!(called.1["call"]["output"], input["input"]);
    let call = contracts::id(&called.1["call"], "call_id").unwrap();
    let binding=c.query_one("SELECT c.http_binding,c.skill_id,d.installation_id,i.deployment_id,i.id FROM calls c JOIN instances i ON i.id=c.provider_instance_id JOIN deployments d ON d.id=i.deployment_id WHERE c.id=$1",&[&call]).await.unwrap();
    assert_eq!(binding.get::<_, Uuid>("deployment_id"), f.deployment);
    assert_eq!(binding.get::<_, Uuid>("installation_id"), f.installation);
    assert_eq!(binding.get::<_, Uuid>("skill_id").to_string(), skill);
    assert_eq!(
        binding.get::<_, Value>("http_binding")["deployment_id"],
        f.deployment.to_string()
    );
    assert_eq!(
        called.1["call"]["provider_instance_id"],
        binding.get::<_, Uuid>("id").to_string()
    );
    let committed = ledger(&c, f.owner).await;
    assert_eq!(
        committed["calls"].as_i64().unwrap(),
        before["calls"].as_i64().unwrap() + 1
    );
    assert_eq!(
        committed["daily"].as_i64().unwrap(),
        before["daily"].as_i64().unwrap() + 1
    );
    assert_eq!(committed["active"], before["active"]);
    assert_eq!(committed["concurrent"], before["concurrent"]);
    let replay = management(
        &f.router,
        "invokeSkill",
        &params,
        &f.token,
        selected,
        "selector-success-stable-key",
        None,
    )
    .await;
    assert_eq!(replay, called);
    // Make A eligible only as metadata, so a changed fixed selector reaches
    // fingerprint comparison. It must conflict before any transport or charge.
    c.execute(
        "UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','true'),phase='ready' WHERE id=$1",
        &[&unavailable],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE instances SET last_heartbeat_at=now() WHERE deployment_id=$1",
        &[&unavailable],
    )
    .await
    .unwrap();
    let mut changed = input;
    changed["provider_deployment_id"] = json!(unavailable);
    let conflict = management(
        &f.router,
        "invokeSkill",
        &params,
        &f.token,
        changed,
        "selector-success-stable-key",
        None,
    )
    .await;
    assert_eq!(conflict.0, 409, "{}", conflict.1);
    assert_eq!(conflict.1["error"]["code"], "IDEMPOTENCY_CONFLICT");
    assert_eq!(
        ledger(&c, f.owner).await,
        committed,
        "selector replay/conflict cannot add calls, quota or source charges"
    );
    // Keep the parent test's later unqualified Skill call unambiguous. These
    // two rows remain historical metadata, with no claimed process lifecycle.
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','false'),phase='disabled' WHERE id=ANY($1)",&[&vec![unavailable,other_deployment]]).await.unwrap();
    println!("HTTP Skill fixed selector: disabled A/other-installation C refused without charge; B executed through real independent mTLS echo; changed selector replay conflicted without new charge");
}
