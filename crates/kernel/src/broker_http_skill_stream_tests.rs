//! Actual PostgreSQL, production Router/Authorizer/Broker and independent mTLS
//! HTTP process. Metadata/heartbeat are fixtures; OCI fencing is tested in http.rs.
use super::*;

async fn register(f: &Component, index: usize) -> String {
    let value = management(
        &f.router,
        "registerSkill",
        &[],
        &f.token,
        json!({"installation_id":f.installation,"definition":f.manifest["skills"][index]}),
        &format!("stream-profile-register-{index}"),
        None,
    )
    .await;
    assert_eq!(value.0, 201, "{}", value.1);
    let id = value.1["id"].as_str().unwrap().to_owned();
    let enabled = management(
        &f.router,
        "setSkillEnabled",
        &[("skill_id", id.clone())],
        &f.token,
        json!({"enabled":true}),
        &format!("stream-profile-enable-{index}"),
        Some(1),
    )
    .await;
    assert_eq!(enabled.0, 200, "{}", enabled.1);
    id
}
async fn stream(
    f: &Component,
    skill: &str,
    token: &str,
    key: &str,
    message: &str,
    items: u64,
    interval: u64,
) -> Response {
    raw(&f.router,"POST",CORE,&format!("/api/platform/v1/skills/{skill}/calls:stream"),Some(token),
        serde_json::to_vec(&json!({"input":{"message":message,"items":items,"interval_ms":interval},"timeout_ms":10000,"idempotency_key":key,"provider_deployment_id":f.deployment})).unwrap(),
        &[("content-type","application/json".into()),("idempotency-key",key.into())]).await
}
async fn frames(response: Response) -> Vec<Value> {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 4194304).await.unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let values: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|raw| serde_json::from_str(raw).unwrap())
        .collect();
    for (index, value) in values.iter().enumerate() {
        contracts::validate("SkillStreamEvent", value).unwrap();
        assert_eq!(value["event"]["sequence"], index + 1);
    }
    values
}
async fn usage(f: &Component) -> (i64, i64, i64) {
    let c = f.app.db.connect().await.unwrap();
    let row=c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) AS calls,daily_calls_used,concurrent_calls_used FROM quotas WHERE account_id=$1",&[&f.owner]).await.unwrap();
    (
        row.get("calls"),
        row.get("daily_calls_used"),
        row.get("concurrent_calls_used"),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_http_skill_stream_and_fixed_methods_execute_with_original_authority_and_accounting()
{
    let Some(mut f) = Component::with_exact_route_budget(true).await else {
        return;
    };
    for (index, input, expected) in [
        (2, Value::Null, json!({"message":"HTTP fixed read"})),
        (
            3,
            json!({"message":"actual PUT201"}),
            json!({"message":"actual PUT201"}),
        ),
        (
            4,
            json!({"message":"actual PATCH200"}),
            json!({"message":"actual PATCH200"}),
        ),
        (5, Value::Null, Value::Null),
        (6, Value::Null, Value::Null),
    ] {
        let skill = register(&f, index).await;
        let called=management(&f.router,"invokeSkill",&[("skill_id",skill)],&f.token,
            json!({"input":input,"provider_deployment_id":f.deployment,"idempotency_key":format!("fixed-method-{index}")}),
            &format!("fixed-method-{index}"),None).await;
        assert_eq!(called.0, 200, "{index}: {}", called.1);
        assert_eq!(called.1["call"]["output"], expected);
        assert_eq!(f.active().await, 0);
    }
    let stream_skill = register(&f, 1).await;
    let stream_rule = json!({"action":"skill.invoke","resource":{"kind":"skill","match":"ids","ids":[stream_skill]}});
    let target_rule = json!({"action":"plugin.http.invoke","resource":{"kind":"plugin","match":"ids","ids":["org.xelab.http-echo"]}});
    let before = usage(&f).await;
    for (index, rules) in [json!([stream_rule]), json!([target_rule])]
        .into_iter()
        .enumerate()
    {
        let key=management(&f.router,"createApiKey",&[("account_id",f.owner.to_string())],&f.token,
            json!({"name":"one HTTP stream permission","inheritance":{"mode":"selected","rules":rules},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::days(1)}}),&format!("stream-one-layer-{index}"),None).await;
        assert_eq!(key.0, 201, "{}", key.1);
        let denied = stream(
            &f,
            &stream_skill,
            key.1["secret"].as_str().unwrap(),
            &format!("stream-one-layer-{index}"),
            "must not dispatch",
            1,
            0,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    let wrong_endpoint = management(
        &f.router,
        "invokeSkill",
        &[("skill_id", stream_skill.clone())],
        &f.token,
        json!({"input":{"message":"wrong endpoint","items":1,"interval_ms":0}}),
        "stream-on-unary",
        None,
    )
    .await;
    assert_eq!(wrong_endpoint.0, 400, "{}", wrong_endpoint.1);
    assert_eq!(
        usage(&f).await,
        before,
        "Rejected requests must not allocate or charge"
    );
    let good = frames(
        stream(
            &f,
            &stream_skill,
            &f.token,
            "actual-http-skill-stream",
            "real HTTP Skill stream",
            2,
            1000,
        )
        .await,
    )
    .await;
    assert_eq!(good.len(), 4);
    assert_eq!(good[0]["event"]["kind"], "accepted");
    assert_eq!(
        good[1]["event"]["payload"],
        json!({"message":"real HTTP Skill stream","index":0})
    );
    assert_eq!(good[3]["event"]["kind"], "completed");
    let result = &good[3]["event"]["payload"];
    assert_eq!(result["skill_id"], stream_skill);
    assert_eq!(
        result["skill_definition_digest"],
        good[3]["definition_digest"]
    );
    assert_eq!(result["contract"]["name"], f.manifest["skills"][1]["name"]);
    assert_eq!(
        result["contract"]["schema_digest"],
        good[3]["definition_digest"]
    );
    assert_eq!(
        result["output"],
        json!({"message":"real HTTP Skill stream"})
    );
    let call = contracts::id(result, "call_id").unwrap();
    let c = f.app.db.connect().await.unwrap();
    let stored = c
        .query_one(
            "SELECT state,result,http_binding FROM calls WHERE id=$1",
            &[&call],
        )
        .await
        .unwrap();
    assert_eq!(stored.get::<_, String>("state"), "succeeded");
    assert_eq!(stored.get::<_, Value>("result"), *result);
    assert_eq!(
        stored.get::<_, Value>("http_binding")["operation_id"],
        "streamMessages"
    );
    crate::skills::validate_snapshot_bindings(&c).await.unwrap();
    crate::http_contracts::validate_snapshot_bindings(&c)
        .await
        .unwrap();
    // A completed stream persists the same virtual Skill result contract as
    // unary HTTP Skills. Raw HTTP identity may not replace it during restore.
    let original: Value = stored.get("result");
    let forged = json!({"name":"org.xelab.http-echo.http","version":"1.0.0","schema_digest":f.manifest["skills"][1]["target"]["http_contract_digest"]});
    c.execute(
        "UPDATE calls SET contract=$2 WHERE id=$1",
        &[&call, &forged],
    )
    .await
    .unwrap();
    assert!(crate::http_contracts::validate_snapshot_bindings(&c)
        .await
        .is_err());
    c.execute(
        "UPDATE calls SET contract=$2 WHERE id=$1",
        &[&call, &original["contract"]],
    )
    .await
    .unwrap();
    crate::http_contracts::validate_snapshot_bindings(&c)
        .await
        .unwrap();
    let charged = usage(&f).await;
    assert_eq!(charged, (before.0 + 1, before.1 + 1, 0));
    let replay = stream(
        &f,
        &stream_skill,
        &f.token,
        "actual-http-skill-stream",
        "real HTTP Skill stream",
        2,
        10,
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    assert_eq!(
        usage(&f).await,
        charged,
        "A generation stream must not replay or charge twice"
    );
    // The malformed item comes from a real independent TLS process. Neither
    // signed OpenAPI nor Skill output validation may become an empty success.
    let malformed = frames(
        stream(
            &f,
            &stream_skill,
            &f.token,
            "malformed-http-skill-item",
            "fault:skill-item",
            1,
            0,
        )
        .await,
    )
    .await;
    assert_eq!(malformed[0]["event"]["kind"], "accepted");
    assert_eq!(malformed.len(), 2);
    assert_eq!(malformed[1]["event"]["kind"], "failed");
    let malformed_call = contracts::id(&malformed[0]["event"], "call_id").unwrap();
    let failed = c
        .query_one(
            "SELECT state,result FROM calls WHERE id=$1",
            &[&malformed_call],
        )
        .await
        .unwrap();
    assert_eq!(failed.get::<_, String>("state"), "unknown");
    assert!(failed.get::<_, Option<Value>>("result").is_none());
    assert_eq!(
        f.active().await,
        1,
        "A component fixture cannot claim that OCI process fencing ran"
    );
    let prior = usage(&f).await;
    assert_eq!(
        stream(
            &f,
            &stream_skill,
            &f.token,
            "malformed-http-skill-item",
            "fault:skill-item",
            1,
            0
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(usage(&f).await, prior);
    let malformed_output = frames(
        stream(
            &f,
            &stream_skill,
            &f.token,
            "malformed-http-skill-output",
            "fault:skill-output",
            1,
            0,
        )
        .await,
    )
    .await;
    assert_eq!(malformed_output.len(), 3);
    assert_eq!(malformed_output[1]["event"]["kind"], "item");
    assert_eq!(malformed_output[2]["event"]["kind"], "failed");
    assert!(!malformed_output
        .iter()
        .any(|event| event["event"]["kind"] == "completed"));
    let output_call = contracts::id(&malformed_output[0]["event"], "call_id").unwrap();
    let failed = c
        .query_one(
            "SELECT state,result FROM calls WHERE id=$1",
            &[&output_call],
        )
        .await
        .unwrap();
    assert_eq!(failed.get::<_, String>("state"), "unknown");
    assert!(failed.get::<_, Option<Value>>("result").is_none());
    assert_eq!(f.active().await, 2);
    let full = usage(&f).await;
    let denied = stream(
        &f,
        &stream_skill,
        &f.token,
        "full-physical-http-stream-pool",
        "must not dispatch",
        1,
        0,
    )
    .await;
    assert_eq!(denied.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        usage(&f).await,
        full,
        "Physical exhaustion must not create a Call or leak quota"
    );
    f.stop().await;
}
