//! Fenced result submission after HTTP admission and a trusted worker claim.
use super::{request, Fixture};
use axum::http::Method;
use serde_json::{json, Value};
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{
    contracts,
    error::Error,
    task_engine::{claim, commit},
};

async fn publish(f: &Fixture, nodes: &[(&str, &[&str])]) -> Uuid {
    let mut spec = f.spec("commit");
    let template = &spec["steps"][0];
    spec["steps"] = nodes
        .iter()
        .map(|(key, dependencies)| {
            let mut step = template.clone();
            step["id"] = json!(key);
            step["depends_on"] = json!(dependencies);
            step["timeout_ms"] = json!(30_000);
            step
        })
        .collect();
    let workflow = f
        .publish(&f.token, &spec, "commit-workflow")
        .await
        .expect(201);
    contracts::id(&workflow.body, "id").unwrap()
}

async fn submit(f: &Fixture, workflow: Uuid, key: &str) -> Uuid {
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(&f.token),
        Some(&json!({"workflow_id":workflow,"input":{}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201);
    contracts::id(&task.body, "id").unwrap()
}

async fn leased(f: &Fixture, worker: Uuid) -> Value {
    claim(&f.db, &f.auth, worker)
        .await
        .unwrap()
        .expect("ready step")
}

fn success(attempt: &Value, message: &str) -> Value {
    json!({"attempt_id":attempt["id"],"execution_id":attempt["execution_id"],"fence":attempt["fence"],
           "state":"succeeded","output":{"message":message},"error":null})
}

async fn audit_count(db: &Client) -> i64 {
    db.query_one("SELECT count(*) FROM audit_events", &[])
        .await
        .unwrap()
        .get(0)
}

// Check committed state on an independent connection, including timestamps and revisions.
async fn snapshot(db: &Client, task: Uuid) -> Value {
    db.query_one(
        "SELECT jsonb_build_object(
            'task', to_jsonb(t) - 'admission',
            'steps', (SELECT jsonb_object_agg(s.step_key,to_jsonb(s)) FROM task_steps s WHERE s.task_id=t.id),
            'attempts', (SELECT jsonb_object_agg(a.id,to_jsonb(a)) FROM attempts a
                         JOIN task_steps s ON s.id=a.step_id WHERE s.task_id=t.id),
            'receipts', (SELECT jsonb_object_agg(r.attempt_id,to_jsonb(r)) FROM attempt_commit_receipts r
                         JOIN attempts a ON a.id=r.attempt_id JOIN task_steps s ON s.id=a.step_id
                         WHERE s.task_id=t.id),
            'outbox', (SELECT count(*) FROM outbox))
         FROM tasks t WHERE id=$1", &[&task]).await.unwrap().get(0)
}

#[tokio::test]
async fn success_releases_fan_in_then_finishes_task_and_replays_without_writes() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("a", &[]), ("b", &[]), ("sink", &["a", "b"])]).await;
    let task = submit(&f, workflow, "fan-in").await;
    let worker = Uuid::new_v4();

    for (index, expected_sink_state) in ["pending", "ready"].into_iter().enumerate() {
        let attempt = leased(&f, worker).await;
        let input = success(&attempt, &format!("root {index}"));
        let step = commit(&f.db, &f.auth, worker, &input).await.unwrap();
        contracts::validate("TaskStep", &step).unwrap();
        assert_eq!(step["id"], attempt["step_id"]);
        assert_eq!(step["task_id"], json!(task));
        assert_eq!(step["state"], "succeeded");
        let after = snapshot(&db, task).await;
        let root = &after["steps"][step["step_key"].as_str().unwrap()];
        assert_eq!(root["output"], input["output"]);
        assert_eq!(root["state"], "succeeded");
        assert_eq!(
            after["attempts"][attempt["id"].as_str().unwrap()]["state"],
            "succeeded"
        );
        assert_eq!(after["task"]["state"], "running");
        assert_eq!(after["steps"]["sink"]["state"], expected_sink_state);

        let sink_id = contracts::id(&after["steps"]["sink"], "id").unwrap();
        let ready_events: i64 = db
            .query_one(
                "SELECT count(*) FROM outbox WHERE aggregate_id=$1 AND topic='task.step.ready'",
                &[&sink_id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(ready_events, i64::from(expected_sink_state == "ready"));
        if expected_sink_state == "ready" {
            let audits = audit_count(&db).await;
            let replay = commit(&f.db, &f.auth, worker, &input).await.unwrap();
            assert_eq!(replay, step);
            assert_eq!(audit_count(&db).await, audits);
            assert_eq!(snapshot(&db, task).await, after);
        }
    }

    let sink = leased(&f, worker).await;
    let input = success(&sink, "joined");
    let step = commit(&f.db, &f.auth, worker, &input).await.unwrap();
    assert_eq!(step["step_key"], "sink");
    let finished = snapshot(&db, task).await;
    assert_eq!(finished["task"]["state"], "succeeded");
    assert_eq!(finished["task"]["output"], input["output"]);
    assert_eq!(finished["steps"]["sink"]["output"], input["output"]);
    assert_eq!(commit(&f.db, &f.auth, worker, &input).await.unwrap(), step);
    assert_eq!(snapshot(&db, task).await, finished);
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);

    // A matching receipt alone cannot authorize a late acknowledgement.
    let sink_id = contracts::id(&sink, "id").unwrap();
    assert_eq!(
        db.execute(
            "UPDATE attempts SET lease_expires_at=clock_timestamp() WHERE id=$1",
            &[&sink_id]
        )
        .await
        .unwrap(),
        1
    );
    let expired = snapshot(&db, task).await;
    let late = commit(&f.db, &f.auth, worker, &input).await.unwrap_err();
    assert_eq!(late.code, "STATE_CONFLICT");
    assert_eq!(snapshot(&db, task).await, expired);
}

#[tokio::test]
async fn malformed_commits_and_invalid_business_output_leave_the_lease_retryable() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("echo", &[])]).await;
    let task = submit(&f, workflow, "validation").await;
    let worker = Uuid::new_v4();
    let attempt = leased(&f, worker).await;
    let input = success(&attempt, "accepted");
    let before = snapshot(&db, task).await;
    let mut missing = input.clone();
    missing.as_object_mut().unwrap().remove("error");
    let mut bad_error = input.clone();
    bad_error["state"] = json!("failed");
    bad_error["output"] = Value::Null;
    bad_error["error"] = json!({"code":"INVALID_ARGUMENT"});
    let mut contradictory = input.clone();
    contradictory["error"] = Error::invalid("success with an error").detail();
    let mut unknown_output = input.clone();
    unknown_output["state"] = json!("unknown");
    unknown_output["error"] = Error::conflict("unconfirmed outcome").detail();
    let mut wrong_type = input.clone();
    wrong_type["output"]["message"] = json!(17);
    let mut too_long = input.clone();
    // Valid envelope and JSON type, but exceeds the installed echo output schema's maxLength.
    too_long["output"]["message"] = json!("x".repeat(1025));
    for (case, invalid) in [
        ("absent error field", missing),
        ("incomplete ErrorDetail", bad_error),
        ("success with error", contradictory),
        ("unknown with output", unknown_output),
        ("output property type", wrong_type),
        ("output maxLength", too_long),
    ] {
        let error = commit(&f.db, &f.auth, worker, &invalid)
            .await
            .expect_err(case);
        assert_eq!(error.code, "INVALID_ARGUMENT", "{case}: {error}");
        assert_eq!(snapshot(&db, task).await, before, "{case}");
    }
    commit(&f.db, &f.auth, worker, &input).await.unwrap();
    let after = snapshot(&db, task).await;
    assert_eq!(after["steps"]["echo"]["output"], input["output"]);
    assert_eq!(after["steps"]["echo"]["attempt_count"], 1);
}

#[tokio::test]
async fn outbox_commit_failure_rolls_back_receipt_and_allows_the_same_submission() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("root", &[]), ("next", &["root"])]).await;
    let task = submit(&f, workflow, "retry").await;
    let worker = Uuid::new_v4();
    let attempt = leased(&f, worker).await;
    let input = success(&attempt, "retry");
    let before = snapshot(&db, task).await;
    // Force a database COMMIT failure after receipt, output and ready writes.
    db.batch_execute(
        "CREATE FUNCTION reject_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected outbox failure'; END; $$;
         CREATE CONSTRAINT TRIGGER reject_outbox AFTER INSERT ON outbox
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_outbox()",
    )
    .await
    .unwrap();
    commit(&f.db, &f.auth, worker, &input)
        .await
        .expect_err("deferred commit failed");
    assert_eq!(snapshot(&db, task).await, before);
    db.batch_execute("DROP TRIGGER reject_outbox ON outbox")
        .await
        .unwrap();
    let step = commit(&f.db, &f.auth, worker, &input).await.unwrap();
    assert_eq!(step["id"], attempt["step_id"]);
    let after = snapshot(&db, task).await;
    assert_eq!(after["steps"]["root"]["output"], input["output"]);
    assert_eq!(after["steps"]["next"]["state"], "ready");
    assert_eq!(after["receipts"].as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn mismatched_tokens_superseded_fence_and_expired_lease_cannot_publish() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("echo", &[]), ("next", &["echo"])]).await;
    let task = submit(&f, workflow, "tokens").await;
    let worker = Uuid::new_v4();
    let attempt = leased(&f, worker).await;
    let input = success(&attempt, "late");
    let before = snapshot(&db, task).await;
    let other_worker = commit(&f.db, &f.auth, Uuid::new_v4(), &input)
        .await
        .unwrap_err();
    assert!(
        matches!(other_worker.code, "STATE_CONFLICT" | "PERMISSION_DENIED"),
        "{other_worker}"
    );
    assert_eq!(snapshot(&db, task).await, before);
    for (field, value) in [
        ("attempt_id", json!(Uuid::new_v4())),
        ("execution_id", json!(Uuid::new_v4())),
        ("fence", json!(2)),
    ] {
        let mut wrong = input.clone();
        wrong[field] = value;
        let error = commit(&f.db, &f.auth, worker, &wrong).await.unwrap_err();
        assert_eq!(error.code, "STATE_CONFLICT", "{field}: {error}");
        assert_eq!(snapshot(&db, task).await, before);
    }

    // All three tokens still match the original attempt. Independently change
    // the current step generation and, on a second real claim, the lease cutoff.
    let expired_task = submit(&f, workflow, "expiry").await;
    let expired = leased(&f, worker).await;
    let expired_id = contracts::id(&expired, "id").unwrap();
    let step_id = contracts::id(&attempt, "step_id").unwrap();
    assert_eq!(
        db.execute(
            "UPDATE task_steps SET fence=fence+1 WHERE id=$1",
            &[&step_id]
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        db.execute(
            "UPDATE attempts SET lease_expires_at=clock_timestamp() WHERE id=$1",
            &[&expired_id]
        )
        .await
        .unwrap(),
        1
    );
    for (task, stale) in [(task, input), (expired_task, success(&expired, "expired"))] {
        let before = snapshot(&db, task).await;
        let audits = audit_count(&db).await;
        let error = commit(&f.db, &f.auth, worker, &stale).await.unwrap_err();
        assert_eq!(error.code, "STATE_CONFLICT", "{error}");
        assert_eq!(snapshot(&db, task).await, before);
        assert!(
            audit_count(&db).await > audits,
            "late result must be audited"
        );
    }
}

#[tokio::test]
async fn revocation_blocks_both_an_acknowledgement_and_an_active_result() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("echo", &[]), ("next", &["echo"])]).await;
    for (key, change) in [
        (
            "delegation",
            "UPDATE execution_delegations SET status='revoked'
             WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        ),
        (
            "issuer",
            "UPDATE credentials SET status='revoked' WHERE id=(
                 SELECT d.issuer_credential_id FROM execution_delegations d
                 JOIN tasks t ON t.delegation_id=d.id WHERE t.id=$1)",
        ),
    ] {
        let task = submit(&f, workflow, key).await;
        let worker = Uuid::new_v4();
        let predecessor = leased(&f, worker).await;
        let accepted = success(&predecessor, "before revocation");
        commit(&f.db, &f.auth, worker, &accepted).await.unwrap();
        let attempt = leased(&f, worker).await;
        assert_eq!(db.execute(change, &[&task]).await.unwrap(), 1);
        let acknowledgement = commit(&f.db, &f.auth, worker, &accepted).await.unwrap_err();
        assert_eq!(
            acknowledgement.code, "PERMISSION_DENIED",
            "{key}: {acknowledgement}"
        );
        let input = success(&attempt, "revoked");
        let error = commit(&f.db, &f.auth, worker, &input).await.unwrap_err();
        // An earlier denial may already have paused the task.
        assert!(
            matches!(error.code, "PERMISSION_DENIED" | "STATE_CONFLICT"),
            "{key}: {error}"
        );
        let after = snapshot(&db, task).await;
        assert_eq!(after["task"]["state"], "paused_authorization");
        assert!(after["task"]["output"].is_null());
        assert_eq!(after["steps"]["echo"]["output"], accepted["output"]);
        assert_eq!(after["steps"]["echo"]["state"], "succeeded");
        assert!(after["steps"]["next"]["output"].is_null());
        assert_ne!(
            after["attempts"][attempt["id"].as_str().unwrap()]["state"],
            "succeeded"
        );
        assert_ne!(after["steps"]["next"]["state"], "succeeded");
        assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    }
}

#[tokio::test]
async fn failed_and_unknown_results_retain_the_error_and_do_not_release_dependents() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("echo", &[]), ("next", &["echo"])]).await;
    let worker = Uuid::new_v4();
    for (state, error) in [
        ("failed", Error::invalid("Execution failed").detail()),
        ("unknown", Value::Null),
    ] {
        let task = submit(&f, workflow, state).await;
        let attempt = leased(&f, worker).await;
        let mut input = success(&attempt, "");
        input["state"] = json!(state);
        input["output"] = Value::Null;
        input["error"] = error;
        let step = commit(&f.db, &f.auth, worker, &input).await.unwrap();
        assert_eq!(step["state"], state);
        let finished = snapshot(&db, task).await;
        assert_eq!(
            finished["attempts"][attempt["id"].as_str().unwrap()]["state"],
            state
        );
        assert_eq!(finished["steps"]["echo"]["state"], state);
        assert!(finished["steps"]["echo"]["output"].is_null());
        assert!(matches!(
            finished["steps"]["next"]["state"].as_str(),
            Some("pending" | "canceled")
        ));
        assert_eq!(finished["steps"]["next"]["attempt_count"], 0);
        assert_eq!(finished["task"]["state"], state);
        assert_eq!(finished["task"]["error"], input["error"]);
        assert!(finished["task"]["output"].is_null());
        assert_eq!(commit(&f.db, &f.auth, worker, &input).await.unwrap(), step);
        input["error"] = Error::invalid("different result for the same attempt").detail();
        let conflict = commit(&f.db, &f.auth, worker, &input).await.unwrap_err();
        assert_eq!(conflict.code, "STATE_CONFLICT");
        assert_eq!(snapshot(&db, task).await, finished);
        assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    }
}

#[tokio::test]
async fn competing_outputs_on_one_attempt_persist_only_the_winners_result() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, &[("left-sink", &[]), ("right-sink", &[])]).await;
    let task = submit(&f, workflow, "contended").await;
    let worker = Uuid::new_v4();
    let attempt = leased(&f, worker).await;
    let left = success(&attempt, "left");
    let right = success(&attempt, "right");
    let results = tokio::join!(
        commit(&f.db, &f.auth, worker, &left),
        commit(&f.db, &f.auth, worker, &right)
    );
    let (winner, step, conflict) = match results {
        (Ok(step), Err(conflict)) => (left, step, conflict),
        (Err(conflict), Ok(step)) => (right, step, conflict),
        results => panic!("expected one accepted output: {results:?}"),
    };
    assert_eq!(conflict.code, "STATE_CONFLICT");
    assert_eq!(step["state"], "succeeded");
    let after = snapshot(&db, task).await;
    assert_eq!(after["task"]["state"], "running");
    let first_sink = step["step_key"].as_str().unwrap();
    assert_eq!(after["steps"][first_sink]["output"], winner["output"]);
    assert_eq!(
        after["attempts"][attempt["id"].as_str().unwrap()]["revision"],
        attempt["revision"].as_i64().unwrap() + 1
    );

    let remaining = leased(&f, worker).await;
    let other = success(&remaining, "other sink");
    let second = commit(&f.db, &f.auth, worker, &other).await.unwrap();
    let second_sink = second["step_key"].as_str().unwrap();
    assert_ne!(first_sink, second_sink);
    let finished = snapshot(&db, task).await;
    assert_eq!(finished["task"]["state"], "succeeded");
    let task_output: Option<Value> = db
        .query_one("SELECT output FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        task_output, None,
        "multiple sinks leave tasks.output SQL NULL"
    );
    assert_eq!(finished["steps"][first_sink]["output"], winner["output"]);
    assert_eq!(finished["steps"][second_sink]["output"], other["output"]);
}
