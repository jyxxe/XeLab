//! Task cancellation through the main router and the fenced worker entry points.
use super::{
    request,
    task_commit::{leased, publish, snapshot, submit, success},
    workflow_authority_wait::wait_for_rule_expiry,
    Fixture, Reply,
};
use axum::http::{Method, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use xelab_kernel::{
    error::Error,
    task_engine::{claim, commit, prepare_dispatch, renew, AttemptLease},
};

fn path(task: Uuid) -> String {
    format!("/api/platform/v1/tasks/{task}")
}

async fn cancel(
    f: &Fixture,
    task: Uuid,
    token: &str,
    tag: Option<&str>,
    key: &str,
    body: &Value,
) -> Reply {
    let mut headers = vec![("idempotency-key", key)];
    if let Some(tag) = tag {
        headers.push(("if-match", tag));
    }
    request(
        &f.router,
        Method::POST,
        &format!("{}/cancel", path(task)),
        Some(token),
        Some(body),
        &headers,
    )
    .await
}

async fn etag(f: &Fixture, task: Uuid) -> String {
    f.read(&f.token, &path(task))
        .await
        .expect(200)
        .etag
        .unwrap()
}

#[tokio::test]
async fn queued_cancel_checks_revision_and_is_idempotent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[]), ("b", &["a"])]).await;
    let task = submit(&f, workflow, "queued-cancel-task").await;
    let db = f.db.connect().await.unwrap();
    let queued = snapshot(&db, task).await;
    let tag = etag(&f, task).await;
    let body = json!({"reason":"no longer needed"});

    cancel(&f, task, &f.token, None, "missing-tag", &body)
        .await
        .expect(428);
    cancel(&f, task, &f.token, Some("\"rev-999\""), "stale-tag", &body)
        .await
        .expect(412);
    request(
        &f.router,
        Method::POST,
        &format!("{}/cancel?unused=1", path(task)),
        Some(&f.token),
        Some(&body),
        &[("if-match", &tag), ("idempotency-key", "unexpected-query")],
    )
    .await
    .expect(400);
    assert_eq!(snapshot(&db, task).await, queued);

    let result = cancel(&f, task, &f.token, Some(&tag), "cancel-queued", &body)
        .await
        .expect(200);
    assert_eq!(result.body["state"], "canceled");
    assert_eq!(result.body["revision"], 2);
    assert_eq!(result.etag.as_deref(), Some("\"rev-2\""));
    let stopped = snapshot(&db, task).await;
    for key in ["a", "b"] {
        assert_eq!(stopped["steps"][key]["state"], "canceled");
        assert_eq!(stopped["steps"][key]["attempt_count"], 0);
        assert_eq!(stopped["steps"][key]["fence"], 0);
    }
    assert!(stopped["attempts"].is_null());
    assert_eq!(
        stopped["task"]["delegation_id"],
        queued["task"]["delegation_id"]
    );
    assert_eq!(stopped["outbox"], queued["outbox"].as_i64().unwrap() + 1);
    assert_eq!(
        cancel(&f, task, &f.token, Some(&tag), "cancel-queued", &body)
            .await
            .expect(200)
            .body,
        result.body
    );
    assert_eq!(
        cancel(
            &f,
            task,
            &f.token,
            Some(&tag),
            "cancel-queued",
            &json!({"reason":"different"})
        )
        .await
        .expect(409)
        .body["error"]["code"],
        "IDEMPOTENCY_CONFLICT"
    );
    assert_eq!(
        cancel(
            &f,
            task,
            &f.token,
            result.etag.as_deref(),
            "already-canceled",
            &body
        )
        .await
        .expect(200)
        .body,
        result.body
    );
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
    assert_eq!(snapshot(&db, task).await, stopped);
}

#[tokio::test]
async fn cancel_blocks_new_work_and_accepts_only_fenced_terminal_results() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[]), ("b", &[]), ("c", &["a"])]).await;
    let task = submit(&f, workflow, "active-cancel-task").await;
    let db = f.db.connect().await.unwrap();
    let worker_id = Uuid::new_v4();
    let attempt = leased(&f, worker_id).await;
    let before = snapshot(&db, task).await;
    let tag = etag(&f, task).await;
    let result = cancel(&f, task, &f.token, Some(&tag), "cancel-active", &json!({}))
        .await
        .expect(200);
    assert_eq!(result.body["state"], "cancel_requested");
    let requested = snapshot(&db, task).await;
    assert_eq!(requested["attempts"], before["attempts"]);
    let running = before["steps"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, step)| step["state"] == "running")
        .unwrap()
        .0;
    for (key, step) in requested["steps"].as_object().unwrap() {
        if key == running {
            assert_eq!(step, &before["steps"][key]);
        } else {
            assert_eq!(step["state"], "canceled");
        }
    }
    let lease = AttemptLease {
        worker_id,
        attempt_id: xelab_kernel::contracts::id(&attempt, "id").unwrap(),
        execution_id: xelab_kernel::contracts::id(&attempt, "execution_id").unwrap(),
        fence: attempt["fence"].as_i64().unwrap(),
    };
    assert_eq!(claim(&f.db, &f.auth, worker_id).await.unwrap(), None);
    assert_eq!(
        renew(&f.db, &f.auth, &lease).await.unwrap_err().code,
        "STATE_CONFLICT"
    );
    assert_eq!(
        prepare_dispatch(&f.db, &f.auth, &lease)
            .await
            .unwrap_err()
            .code,
        "STATE_CONFLICT"
    );
    let input = success(&attempt, "real output");
    let mut wrong = input.clone();
    wrong["fence"] = json!(lease.fence + 1);
    assert_eq!(
        commit(&f.db, &f.auth, worker_id, &wrong)
            .await
            .unwrap_err()
            .code,
        "STATE_CONFLICT"
    );
    assert_eq!(snapshot(&db, task).await, requested);
    commit(&f.db, &f.auth, worker_id, &input).await.unwrap();
    let settled = snapshot(&db, task).await;
    assert_eq!(settled["task"]["state"], "canceled");
    assert_eq!(settled["steps"][running]["state"], "succeeded");
    assert_eq!(settled["steps"][running]["output"], input["output"]);
    assert_eq!(settled["steps"]["c"]["state"], "canceled");
    assert_eq!(
        settled["receipts"][lease.attempt_id.to_string()]["state"],
        "succeeded"
    );
    let topics: Vec<String> = db
        .query(
            "SELECT topic FROM outbox WHERE aggregate_id=$1 ORDER BY topic",
            &[&task],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(topics, ["task.cancel.requested", "task.canceled"]);
    assert_eq!(
        cancel(&f, task, &f.token, Some(&tag), "cancel-active", &json!({}))
            .await
            .expect(200)
            .body,
        result.body
    );
}

#[tokio::test]
async fn cancel_keeps_unknown_results_and_legacy_expiry_uncertain() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[]), ("b", &["a"])]).await;
    let task = submit(&f, workflow, "unknown-cancel-task").await;
    let db = f.db.connect().await.unwrap();
    let worker_id = Uuid::new_v4();
    let attempt = leased(&f, worker_id).await;
    let mut terminal = success(&attempt, "");
    terminal["state"] = json!("unknown");
    terminal["output"] = Value::Null;
    terminal["error"] = Error::new("OUTCOME_UNKNOWN", "unsettled", StatusCode::CONFLICT).detail();
    commit(&f.db, &f.auth, worker_id, &terminal).await.unwrap();
    let before = snapshot(&db, task).await;
    assert_eq!(before["task"]["state"], "unknown");
    let result = cancel(
        &f,
        task,
        &f.token,
        Some(&etag(&f, task).await),
        "cancel-unknown",
        &json!({}),
    )
    .await
    .expect(200);
    assert_eq!(result.body["state"], "unknown");
    let after = snapshot(&db, task).await;
    assert_eq!(before["task"]["error"], after["task"]["error"]);
    assert_eq!(before["steps"]["a"], after["steps"]["a"]);
    assert_eq!(before["attempts"], after["attempts"]);
    assert_eq!(before["receipts"], after["receipts"]);
    assert_eq!(after["steps"]["b"]["state"], "canceled");

    // An old snapshot can contain an expired attempt without a matching
    // unknown step. The attempt remains evidence of uncertain external work.
    let legacy = submit(&f, workflow, "legacy-expired-task").await;
    let attempt = leased(&f, worker_id).await;
    let attempt_id = xelab_kernel::contracts::id(&attempt, "id").unwrap();
    db.execute(
        "UPDATE attempts SET state='expired' WHERE id=$1",
        &[&attempt_id],
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE task_steps SET state='ready' WHERE task_id=$1 AND step_key='a'",
        &[&legacy],
    )
    .await
    .unwrap();
    let before = snapshot(&db, legacy).await;
    cancel(
        &f,
        legacy,
        &f.token,
        Some(&etag(&f, legacy).await),
        "legacy-expired",
        &json!({}),
    )
    .await
    .expect(200);
    let after = snapshot(&db, legacy).await;
    assert_eq!(after["task"]["state"], "cancel_requested");
    assert_eq!(after["attempts"], before["attempts"]);
    assert_eq!(after["steps"]["a"], before["steps"]["a"]);
    assert_eq!(after["steps"]["b"]["state"], "canceled");
    assert_eq!(claim(&f.db, &f.auth, worker_id).await.unwrap(), None);

    // A terminal step projection also cannot erase that expired attempt.
    db.execute(
        "UPDATE task_steps SET state='succeeded' WHERE task_id=$1 AND step_key='a'",
        &[&legacy],
    )
    .await
    .unwrap();
    assert_eq!(
        cancel(
            &f,
            legacy,
            &f.token,
            Some(&etag(&f, legacy).await),
            "legacy-terminal",
            &json!({})
        )
        .await
        .expect(200)
        .body["state"],
        "cancel_requested"
    );
}

#[tokio::test]
async fn cancel_rechecks_timed_authority_after_step_wait_and_on_replay() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[]), ("b", &[])]).await;
    let completed = submit(&f, workflow, "completed-cancel-task").await;
    let blocked = submit(&f, workflow, "blocked-cancel-task").await;
    let completed_tag = etag(&f, completed).await;
    let blocked_tag = etag(&f, blocked).await;
    let read_workflows = f.workflow_key(f.owner, &f.token, "workflow.read").await;
    cancel(
        &f,
        blocked,
        &read_workflows,
        Some(&blocked_tag),
        "hidden",
        &json!({}),
    )
    .await
    .expect(404);
    let observer = f.db.connect().await.unwrap();
    let mut peer = f.db.connect().await.unwrap();
    let pid = peer
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let hold = peer.transaction().await.unwrap();
    hold.query_one(
        "SELECT id FROM task_steps WHERE task_id=$1 AND step_key='a' FOR UPDATE",
        &[&blocked],
    )
    .await
    .unwrap();
    let before = snapshot(&observer, blocked).await;
    let until: DateTime<Utc> = observer
        .query_one("SELECT clock_timestamp()+interval '5 seconds'", &[])
        .await
        .unwrap()
        .get(0);
    let scope = json!({"kind":"task","match":"owned_by","owner_id":f.owner});
    let key = request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", f.owner),
        Some(&f.token),
        Some(&json!({
            "name":"temporarily cancel tasks",
            "inheritance":{"mode":"selected","rules":[
                {"action":"task.read","resource":scope},
                {"action":"task.cancel","resource":scope,"conditions":{"expires_at":until}}
            ]},
            "conditions":{"expires_at":Utc::now()+chrono::Duration::days(1)}
        })),
        &[("idempotency-key", "timed-task-key")],
    )
    .await
    .expect(201);
    let token = key.body["secret"].as_str().unwrap();
    cancel(
        &f,
        completed,
        token,
        Some(&completed_tag),
        "cancel-while-live",
        &json!({}),
    )
    .await
    .expect(200);
    let router = f.router.clone();
    let pending_token = token.to_owned();
    let pending = tokio::spawn(async move {
        request(
            &router,
            Method::POST,
            &format!("{}/cancel", path(blocked)),
            Some(&pending_token),
            Some(&json!({})),
            &[
                ("if-match", &blocked_tag),
                ("idempotency-key", "cancel-after-wait"),
            ],
        )
        .await
    });
    wait_for_rule_expiry(&observer, pid, until, &pending).await;
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
    hold.rollback().await.unwrap();
    pending.await.unwrap().expect(403);
    let after = snapshot(&observer, blocked).await;
    assert_eq!(before["task"], after["task"]);
    assert_eq!(before["steps"], after["steps"]);
    let counts = observer
        .query_one(
            "SELECT (SELECT count(*) FROM outbox WHERE aggregate_id=$1),
                    (SELECT count(*) FROM idempotency_records WHERE key='cancel-after-wait')",
            &[&blocked],
        )
        .await
        .unwrap();
    assert_eq!((counts.get::<_, i64>(0), counts.get::<_, i64>(1)), (0, 0));
    cancel(
        &f,
        completed,
        token,
        Some(&completed_tag),
        "cancel-while-live",
        &json!({}),
    )
    .await
    .expect(403);
    assert!(claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn cancellation_outbox_failure_rolls_back_task_steps_and_idempotency() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &[("a", &[])]).await;
    let task = submit(&f, workflow, "rollback-cancel-task").await;
    let db = f.db.connect().await.unwrap();
    let tag = etag(&f, task).await;
    let initial = snapshot(&db, task).await;
    db.batch_execute(
        "ALTER TABLE outbox ADD CONSTRAINT cancel_test CHECK (topic <> 'task.canceled')",
    )
    .await
    .unwrap();
    cancel(&f, task, &f.token, Some(&tag), "cancel-retry", &json!({}))
        .await
        .expect(503);
    assert_eq!(snapshot(&db, task).await, initial);
    db.batch_execute("ALTER TABLE outbox DROP CONSTRAINT cancel_test")
        .await
        .unwrap();
    cancel(&f, task, &f.token, Some(&tag), "cancel-retry", &json!({}))
        .await
        .expect(200);
}
