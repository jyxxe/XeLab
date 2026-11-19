//! Manual resolution over the production router with fenced PostgreSQL history.
use super::{
    request,
    task_commit::{leased, publish, snapshot, submit, success},
    Fixture, Reply,
};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;
use xelab_kernel::{
    contracts::id,
    error::Error,
    task_engine::{claim, commit},
};

async fn unknown(f: &Fixture, worker: Uuid) -> (Value, Value) {
    let attempt = leased(f, worker).await;
    let mut input = success(&attempt, "");
    input["state"] = json!("unknown");
    input["output"] = Value::Null;
    input["error"] = Error::new(
        "OUTCOME_UNKNOWN",
        "Fixture outcome is unknown",
        StatusCode::CONFLICT,
    )
    .detail();
    let step = commit(&f.db, &f.auth, worker, &input).await.unwrap();
    (attempt, step)
}

async fn resolve(
    f: &Fixture,
    token: &str,
    step: &Value,
    key: &str,
    tag: Option<&str>,
    body: &Value,
) -> Reply {
    let mut headers = vec![("idempotency-key", key)];
    if let Some(tag) = tag {
        headers.push(("if-match", tag));
    }
    request(
        &f.router,
        Method::POST,
        &format!(
            "/api/platform/v1/task-steps/{}/resolve",
            step["id"].as_str().unwrap()
        ),
        Some(token),
        Some(body),
        &headers,
    )
    .await
}

fn etag(step: &Value) -> String {
    format!("\"rev-{}\"", step["revision"])
}

fn confirmation() -> Value {
    json!({"resolution":"confirmed_succeeded","evidence":"Provider ledger fixture REF-17 confirmed completion","output":{"message":"recovered"}})
}

#[tokio::test]
async fn confirmed_success_keeps_receipt_and_wakes_only_unattempted_dependencies() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("a", &[]), ("b", &["a"])]).await;
    let task = submit(&f, workflow, "manual-success-task").await;
    let worker = Uuid::new_v4();
    let (attempt, step) = unknown(&f, worker).await;
    let previous = snapshot(&db, task).await;
    assert_eq!(previous["task"]["state"], "unknown");

    let body = confirmation();
    let tag = etag(&step);
    resolve(&f, &f.token, &step, "no-tag", None, &body)
        .await
        .expect(428);
    resolve(&f, &f.token, &step, "stale-tag", Some("\"rev-999\""), &body)
        .await
        .expect(412);
    for invalid in [
        json!({"resolution":"confirmed_succeeded","evidence":"reference"}),
        json!({"resolution":"confirmed_succeeded","evidence":"reference","output":{"message":17}}),
        json!({"resolution":"confirmed_not_executed","evidence":"reference","output":null}),
        json!({"resolution":"confirmed_not_executed","evidence":"  "}),
        json!({"resolution":"confirmed_not_executed","evidence":"ref\u{0}erence"}),
    ] {
        resolve(&f, &f.token, &step, "bad-body", Some(&tag), &invalid)
            .await
            .expect(400);
    }
    assert_eq!(snapshot(&db, task).await, previous);

    let result = resolve(&f, &f.token, &step, "confirm-a", Some(&tag), &body)
        .await
        .expect(200);
    assert_eq!(result.body["state"], "succeeded");
    assert_eq!(result.etag.as_deref(), Some(etag(&result.body).as_str()));
    assert!(!result.body.to_string().contains("REF-17"));

    let settled = snapshot(&db, task).await;
    assert_eq!(settled["task"]["state"], "running");
    assert!(settled["task"]["error"].is_null());
    assert_eq!(settled["attempts"], previous["attempts"]);
    assert_eq!(settled["receipts"], previous["receipts"]);
    assert_eq!(settled["steps"]["a"]["output"], body["output"]);
    assert_eq!(settled["steps"]["b"]["state"], "ready");
    assert_eq!(settled["steps"]["b"]["attempt_count"], 0);

    let attempt_id = id(&attempt, "id").unwrap();
    let row = db
        .query_one(
            "SELECT r.evidence,r.subject_id,r.credential_id,r.credential_chain,a.resource,o.payload
           FROM task_step_resolutions r JOIN audit_events a USING (request_id)
           JOIN outbox o ON o.aggregate_id=$2 AND o.topic='task.step.resolved'
          WHERE r.attempt_id=$1 AND a.action='resolveUnknownStep' AND a.outcome='succeeded'
            AND r.credential_id=a.credential_id AND r.subject_id=a.subject_id",
            &[&attempt_id, &id(&step, "id").unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(
        row.get::<_, &str>("evidence"),
        body["evidence"].as_str().unwrap()
    );
    assert_eq!(row.get::<_, Uuid>("subject_id"), f.owner);
    assert!(!row.get::<_, Vec<Uuid>>("credential_chain").is_empty());
    assert_eq!(
        row.get::<_, Value>("resource"),
        json!({"kind":"task","match":"ids","ids":[task]})
    );
    assert!(!row
        .get::<_, Value>("payload")
        .to_string()
        .contains("REF-17"));

    assert_eq!(
        resolve(&f, &f.token, &step, "confirm-a", Some(&tag), &body)
            .await
            .expect(200)
            .body,
        result.body
    );
    let mut different = body.clone();
    different["evidence"] = json!("different upstream reference");
    assert_eq!(
        resolve(&f, &f.token, &step, "confirm-a", Some(&tag), &different)
            .await
            .expect(409)
            .body["error"]["code"],
        "IDEMPOTENCY_CONFLICT"
    );
    resolve(
        &f,
        &f.token,
        &result.body,
        "new-key",
        result.etag.as_deref(),
        &body,
    )
    .await
    .expect(409);
    assert_eq!(snapshot(&db, task).await, settled);

    let dependency = leased(&f, worker).await;
    assert_eq!(dependency["step_id"], settled["steps"]["b"]["id"]);
    assert_eq!(dependency["number"], 1);
}

#[tokio::test]
async fn not_executed_leaves_failure_and_does_not_enqueue_retry() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("a", &[]), ("b", &["a"])]).await;
    let task = submit(&f, workflow, "manual-failed-task").await;
    let worker = Uuid::new_v4();
    let (_, step) = unknown(&f, worker).await;
    let previous = snapshot(&db, task).await;
    let body = json!({"resolution":"confirmed_not_executed","evidence":"Provider ledger reports attempt absent"});
    let response = resolve(
        &f,
        &f.token,
        &step,
        "not-executed",
        Some(&etag(&step)),
        &body,
    )
    .await
    .expect(200);
    assert_eq!(response.body["state"], "failed");
    let after = snapshot(&db, task).await;
    assert_eq!(after["attempts"], previous["attempts"]);
    assert_eq!(after["receipts"], previous["receipts"]);
    assert_eq!(after["task"]["state"], "failed");
    assert_eq!(after["task"]["error"]["code"], "CONFIRMED_NOT_EXECUTED");
    assert_eq!(after["steps"]["b"]["state"], "pending");
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
}

#[tokio::test]
async fn resolution_retains_cancellation_intent_on_unknown_parent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("a", &[])]).await;
    let task = submit(&f, workflow, "manual-canceled-task").await;
    let worker = Uuid::new_v4();
    let (_, step) = unknown(&f, worker).await;
    let task_path = format!("/api/platform/v1/tasks/{task}");
    let revision = f.read(&f.token, &task_path).await.expect(200).etag.unwrap();
    let cancel = request(
        &f.router,
        Method::POST,
        &format!("{task_path}/cancel"),
        Some(&f.token),
        Some(&json!({})),
        &[
            ("idempotency-key", "cancel-unknown"),
            ("if-match", &revision),
        ],
    )
    .await
    .expect(200);
    assert_eq!(cancel.body["state"], "unknown");

    let before = snapshot(&db, task).await;
    let result = resolve(
        &f,
        &f.token,
        &step,
        "resolved-canceled",
        Some(&etag(&step)),
        &confirmation(),
    )
    .await
    .expect(200);
    assert_eq!(result.body["state"], "succeeded");
    let after = snapshot(&db, task).await;
    assert_eq!(after["task"]["state"], "canceled");
    assert!(after["task"]["error"].is_null());
    assert_eq!(after["attempts"], before["attempts"]);
    assert_eq!(after["receipts"], before["receipts"]);
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
}

#[tokio::test]
async fn original_pinned_skill_schema_is_independent_of_enablement() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let skill = f.register_skill().await;
    let deployment = f.deployment(true).await;
    let mut spec = f.spec("historical");
    let step = spec["steps"][0].as_object_mut().unwrap();
    step.remove("contract");
    step.insert("skill_id".into(), json!(skill));
    step.insert("timeout_ms".into(), json!(10_000));
    let workflow = f
        .publish(&f.token, &spec, "manual-skill-workflow")
        .await
        .expect(201);
    let task = submit(&f, id(&workflow.body, "id").unwrap(), "manual-skill-task").await;
    let worker = Uuid::new_v4();
    let (_, step) = unknown(&f, worker).await;
    let tag = etag(&step);
    let initial = snapshot(&db, task).await;
    db.execute(
        "UPDATE skills SET enabled=false,archived_at=now() WHERE id=$1",
        &[&skill],
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','false') WHERE id=$1",
        &[&deployment],
    )
    .await
    .unwrap();
    let original: Value = db
        .query_one("SELECT admission FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap()
        .get(0);
    db.execute("UPDATE tasks SET admission=NULL WHERE id=$1", &[&task])
        .await
        .unwrap();
    resolve(
        &f,
        &f.token,
        &step,
        "legacy-binding",
        Some(&tag),
        &confirmation(),
    )
    .await
    .expect(409);
    db.execute(
        "UPDATE tasks SET admission=$2 WHERE id=$1",
        &[&task, &original],
    )
    .await
    .unwrap();

    db.execute("UPDATE tasks SET admission=jsonb_set(admission,'{steps,0,binding,definition_digest}','\"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"') WHERE id=$1", &[&task]).await.unwrap();
    resolve(
        &f,
        &f.token,
        &step,
        "bad-binding",
        Some(&tag),
        &confirmation(),
    )
    .await
    .expect(409);
    assert_eq!(snapshot(&db, task).await, initial);
    db.execute(
        "UPDATE tasks SET admission=$2 WHERE id=$1",
        &[&task, &original],
    )
    .await
    .unwrap();

    let replacement = Uuid::new_v4();
    db.execute(
        "INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at)
         SELECT $1,plugin_id,'9.0.0',publisher_id,image_digest,manifest_digest,'{}','installed',now(),now()
           FROM installations WHERE id=$2",
        &[&replacement,&f.installation],
    ).await.unwrap();
    db.execute(
        "UPDATE deployments SET installation_id=$2,spec=jsonb_set(spec,'{installation_id}',to_jsonb($2::uuid)) WHERE id=$1",
        &[&deployment,&replacement],
    ).await.unwrap();

    resolve(
        &f,
        &f.token,
        &step,
        "old-pinned-skill",
        Some(&tag),
        &confirmation(),
    )
    .await
    .expect(200);
    let after = snapshot(&db, task).await;
    assert_eq!(after["task"]["state"], "succeeded");
    assert_eq!(after["task"]["output"], confirmation()["output"]);
    assert_eq!(after["attempts"], initial["attempts"]);
}

#[tokio::test]
async fn concurrent_resolution_cannot_replace_a_decision_or_bypass_visibility() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[])]).await;
    let task = submit(&f, workflow, "manual-conflict-task").await;
    let worker = Uuid::new_v4();
    let (attempt, step) = unknown(&f, worker).await;
    let tag = etag(&step);

    let forged = f.workflow_key(f.owner, &f.token, "workflow.read").await;
    let body = confirmation();
    resolve(&f, &forged, &step, "outside-scope", Some(&tag), &body)
        .await
        .expect(404);
    let rejected =
        json!({"resolution":"confirmed_not_executed","evidence":"Provider reports no work"});
    let (left, right) = tokio::join!(
        resolve(&f, &f.token, &step, "success", Some(&tag), &body),
        resolve(&f, &f.token, &step, "not-executed", Some(&tag), &rejected),
    );
    let mut outcomes = [left.status, right.status];
    outcomes.sort_unstable();
    assert_eq!(outcomes, [200, 412]);
    let db = f.db.connect().await.unwrap();
    assert_eq!(
        db.query_one(
            "SELECT count(*) FROM task_step_resolutions WHERE attempt_id=$1",
            &[&id(&attempt, "id").unwrap()]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert!(matches!(
        snapshot(&db, task).await["task"]["state"].as_str(),
        Some("succeeded" | "failed")
    ));
}

#[tokio::test]
async fn evidence_survives_revoked_execution_but_replay_requires_live_operator_authority() {
    use chrono::{DateTime, Utc};
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[]), ("b", &["a"])]).await;
    let task = submit(&f, workflow, "manual-revoked-task").await;
    let worker = Uuid::new_v4();
    let (_, step) = unknown(&f, worker).await;
    let db = f.db.connect().await.unwrap();
    db.execute(
        "UPDATE execution_delegations SET status='revoked' WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        &[&task],
    ).await.unwrap();

    let until: DateTime<Utc> = db
        .query_one("SELECT clock_timestamp()+interval '5 seconds'", &[])
        .await
        .unwrap()
        .get(0);
    let selector = json!({"kind":"task","match":"ids","ids":[task]});
    let key = request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", f.owner),
        Some(&f.token),
        Some(&json!({
            "name":"Temporary resolution",
            "inheritance":{"mode":"selected","rules":[
                {"action":"task.read","resource":selector},
                {"action":"task.resolve","resource":selector,"conditions":{"expires_at":until}}
            ]},
            "conditions":{"expires_at":Utc::now()+chrono::Duration::days(1)}
        })),
        &[("idempotency-key", "resolution-operator")],
    )
    .await
    .expect(201);
    let actor = key.body["secret"].as_str().unwrap();
    let body = confirmation();
    let tag = etag(&step);
    resolve(&f, actor, &step, "resolve-revoked", Some(&tag), &body)
        .await
        .expect(200);
    let after = snapshot(&db, task).await;
    assert_eq!(after["task"]["state"], "paused_authorization");
    assert_eq!(after["steps"]["a"]["state"], "succeeded");
    assert_eq!(after["steps"]["b"]["state"], "pending");
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);

    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        while !db
            .query_one("SELECT clock_timestamp()>=$1", &[&until])
            .await
            .unwrap()
            .get::<_, bool>(0)
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    resolve(&f, actor, &step, "resolve-revoked", Some(&tag), &body)
        .await
        .expect(403);
    assert_eq!(snapshot(&db, task).await, after);
}
