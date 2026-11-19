//! Explicit retry uses retained operator decisions and the original claim authority.
use super::{
    login, request,
    task_commit::{leased, snapshot, submit, success},
    task_resolution::{mark_unknown, resolve, unknown},
    Fixture, Reply,
};
use axum::http::Method;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;
use xelab_kernel::{
    contracts::id,
    task_engine::{claim, commit, prepare_dispatch, AttemptLease},
};

async fn publish(f: &Fixture, budget: Option<u32>, dependent: bool) -> Uuid {
    let mut spec = f.spec("explicit retry");
    spec["steps"][0]["timeout_ms"] = json!(30_000);
    if let Some(budget) = budget {
        spec["steps"][0]["maximum_attempts"] = json!(budget);
    }
    if dependent {
        let mut child = spec["steps"][0].clone();
        child["id"] = json!("child");
        child["depends_on"] = json!(["echo"]);
        spec["steps"].as_array_mut().unwrap().push(child);
    }
    id(
        &f.publish(&f.token, &spec, "retry-workflow")
            .await
            .expect(201)
            .body,
        "id",
    )
    .unwrap()
}

async fn retry(f: &Fixture, token: &str, task: Uuid, etag: &str, key: &str) -> Reply {
    retry_at(f, token, &task.to_string(), etag, key).await
}

async fn retry_at(f: &Fixture, token: &str, task: &str, etag: &str, key: &str) -> Reply {
    request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/tasks/{task}/retry"),
        Some(token),
        None,
        &[("idempotency-key", key), ("if-match", etag)],
    )
    .await
}

async fn task_tag(f: &Fixture, token: &str, task: Uuid) -> String {
    f.read(token, &format!("/api/platform/v1/tasks/{task}"))
        .await
        .expect(200)
        .etag
        .unwrap()
}

async fn non_execution(f: &Fixture, step: &Value, key: &str) {
    let etag = format!("\"rev-{}\"", step["revision"]);
    let response = resolve(f,&f.token,step,key,Some(&etag),&json!({
        "resolution":"confirmed_not_executed","evidence":"Fixture provider journal reports no execution"
    })).await.expect(200);
    assert_eq!(response.body["state"], "failed");
}

#[tokio::test]
async fn explicit_retry_preserves_original_key_and_fences_old_worker() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(2), true).await, "retry-task").await;
    let worker = Uuid::new_v4();
    let (prior, step) = unknown(&f, worker).await;
    let prior_id = id(&prior, "id").unwrap();
    let tag = task_tag(&f, &f.token, task).await;
    retry(&f, &f.token, task, &tag, "unresolved")
        .await
        .expect(409);
    non_execution(&f, &step, "verify-no-execution").await;
    let failed = snapshot(&db, task).await;
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    let tag = task_tag(&f, &f.token, task).await;
    retry(&f, &f.token, task, "\"rev-999\"", "stale-retry")
        .await
        .expect(412);
    let cancel = request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/tasks/{task}/cancel"),
        Some(&f.token),
        Some(&json!({})),
        &[("idempotency-key", "terminal-noop"), ("if-match", &tag)],
    )
    .await
    .expect(200);
    assert_eq!(cancel.body["state"], "failed");
    assert_eq!(cancel.etag.as_deref(), Some(tag.as_str()));
    let alternate = task.to_string().to_uppercase();
    let (a, b) = tokio::join!(
        retry_at(&f, &f.token, &alternate, &tag, "retry-once"),
        retry_at(&f, &f.token, &alternate, &tag, "retry-once")
    );
    let queued = a.expect(200).body;
    assert_eq!(b.expect(200).body, queued);
    assert_eq!(queued["state"], "queued");
    assert_eq!(queued["delegation_id"], failed["task"]["delegation_id"]);
    let requeued = snapshot(&db, task).await;
    assert_eq!(requeued["attempts"], failed["attempts"]);
    assert_eq!(requeued["receipts"], failed["receipts"]);
    assert_eq!(requeued["steps"]["echo"]["state"], "ready");
    assert_eq!(requeued["steps"]["child"]["state"], "pending");
    assert_eq!(
        requeued["outbox"].as_i64().unwrap(),
        failed["outbox"].as_i64().unwrap() + 1
    );

    // Simulate a corrupted marker in an otherwise valid ready projection.
    // Claim must verify the exact linked request before issuing a fence.
    let event: Value = db
        .query_one("SELECT payload FROM outbox WHERE id=$1", &[&prior_id])
        .await
        .unwrap()
        .get(0);
    assert_eq!(event["attempt_id"], json!(prior_id));
    db.execute("UPDATE outbox SET payload=jsonb_set(payload,'{request_id}',to_jsonb($2::uuid)) WHERE id=$1",
        &[&prior_id,&Uuid::new_v4()]).await.unwrap();
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    db.execute(
        "UPDATE outbox SET payload=$2 WHERE id=$1",
        &[&prior_id, &event],
    )
    .await
    .unwrap();

    let another = Uuid::new_v4();
    let (a, b) = tokio::join!(
        claim(&f.db, &f.auth, worker),
        claim(&f.db, &f.auth, another)
    );
    let (claimant, second) = match (a.unwrap(), b.unwrap()) {
        (Some(attempt), None) => (worker, attempt),
        (None, Some(attempt)) => (another, attempt),
        other => panic!("one successor only: {other:?}"),
    };
    assert_eq!(second["step_id"], prior["step_id"]);
    assert_eq!(second["number"], 2);
    assert_eq!(second["idempotency_key"], prior["idempotency_key"]);
    assert_eq!(
        second["fence"].as_i64().unwrap(),
        prior["fence"].as_i64().unwrap() + 1
    );
    assert_ne!(second["execution_id"], prior["execution_id"]);
    assert_ne!(second["id"], prior["id"]);
    let lease = AttemptLease {
        worker_id: claimant,
        attempt_id: id(&second, "id").unwrap(),
        execution_id: id(&second, "execution_id").unwrap(),
        fence: second["fence"].as_i64().unwrap(),
    };
    let dispatch = prepare_dispatch(&f.db, &f.auth, &lease).await.unwrap();
    assert_eq!(
        dispatch.request["idempotency_key"],
        prior["idempotency_key"]
    );
    assert_eq!(
        commit(&f.db, &f.auth, worker, &success(&prior, "late first"))
            .await
            .unwrap_err()
            .code,
        "STATE_CONFLICT"
    );
    commit(&f.db, &f.auth, claimant, &success(&second, "second result"))
        .await
        .unwrap();
    let child = leased(&f, worker).await;
    assert_eq!(child["step_id"], requeued["steps"]["child"]["id"]);
    assert_eq!(child["number"], 1);
    commit(&f.db, &f.auth, worker, &success(&child, "child result"))
        .await
        .unwrap();
    let done = snapshot(&db, task).await;
    assert_eq!(done["task"]["state"], "succeeded");
    assert_eq!(
        done["attempts"][prior_id.to_string()],
        failed["attempts"][prior_id.to_string()]
    );
    assert_eq!(
        done["receipts"][prior_id.to_string()],
        failed["receipts"][prior_id.to_string()]
    );
}

#[tokio::test]
async fn default_and_exhausted_attempt_budgets_refuse_a_further_claim() {
    for budget in [None, Some(2), Some(5)] {
        let Some(f) = Fixture::new().await else {
            return;
        };
        let db = f.db.connect().await.unwrap();
        let task = submit(&f, publish(&f, budget, false).await, "budget-task").await;
        let worker = Uuid::new_v4();
        for count in 1..=budget.unwrap_or(1) {
            let (attempt, step) = unknown(&f, worker).await;
            assert_eq!(attempt["number"], count);
            non_execution(&f, &step, &format!("resolve-attempt-{count}")).await;
            let tag = task_tag(&f, &f.token, task).await;
            let before = snapshot(&db, task).await;
            let response = retry(&f, &f.token, task, &tag, &format!("queue-attempt-{count}")).await;
            if count == budget.unwrap_or(1) {
                response.expect(409);
                assert_eq!(snapshot(&db, task).await, before);
                assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
            } else {
                response.expect(200);
            }
        }
    }
}

#[tokio::test]
async fn prior_execution_evidence_blocks_requeue_and_claim_even_with_a_newer_proof() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(3), false).await, "contested-task").await;
    let worker = Uuid::new_v4();
    let (first, step) = unknown(&f, worker).await;
    non_execution(&f, &step, "confirm-first").await;
    let tag = task_tag(&f, &f.token, task).await;
    retry(&f, &f.token, task, &tag, "queue-second")
        .await
        .expect(200);
    let (_, step) = unknown(&f, worker).await;
    non_execution(&f, &step, "confirm-second").await;
    let tag = task_tag(&f, &f.token, task).await;
    let first = id(&first, "id").unwrap();
    const DECISION: &str = "UPDATE task_step_resolutions SET resolution=$2 WHERE attempt_id=$1";
    // Model imported history with an earlier proven execution and a newer
    // non-execution proof for the same business key.
    db.execute(DECISION, &[&first, &"confirmed_succeeded"])
        .await
        .unwrap();
    retry(&f, &f.token, task, &tag, "queue-third")
        .await
        .expect(409);
    db.execute(DECISION, &[&first, &"confirmed_not_executed"])
        .await
        .unwrap();
    retry(&f, &f.token, task, &tag, "queue-third")
        .await
        .expect(200);
    db.execute(DECISION, &[&first, &"confirmed_succeeded"])
        .await
        .unwrap();
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    let stored = snapshot(&db, task).await;
    assert_eq!(stored["steps"]["echo"]["attempt_count"], 2);
    assert_eq!(stored["attempts"].as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn retry_reconciles_ready_work_on_an_independent_branch() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let mut spec = f.spec("independent DAG branches");
    let template = spec["steps"][0].clone();
    spec["steps"] = [
        ("left", Vec::<&str>::new()),
        ("right", vec![]),
        ("left_sink", vec!["left"]),
        ("right_sink", vec!["right"]),
    ]
    .into_iter()
    .map(|(id, dependencies)| {
        let mut node = template.clone();
        node["id"] = json!(id);
        node["depends_on"] = json!(dependencies);
        node["maximum_attempts"] = json!(2);
        node["timeout_ms"] = json!(30_000);
        node
    })
    .collect();
    let workflow = id(
        &f.publish(&f.token, &spec, "independent-workflow")
            .await
            .expect(201)
            .body,
        "id",
    )
    .unwrap();
    let task = submit(&f, workflow, "independent-task").await;
    let worker = Uuid::new_v4();
    let first = leased(&f, worker).await;
    let second = leased(&f, worker).await;
    let uncertain = mark_unknown(&f, worker, &first).await;
    let succeeded = commit(
        &f.db,
        &f.auth,
        worker,
        &success(&second, "parallel root completed"),
    )
    .await
    .unwrap();
    let ready_sink = format!("{}_sink", succeeded["step_key"].as_str().unwrap());
    let blocked_sink = format!("{}_sink", uncertain["step_key"].as_str().unwrap());
    let suppressed = snapshot(&db, task).await;
    assert_eq!(suppressed["steps"][&ready_sink]["state"], "pending");
    non_execution(&f, &uncertain, "verify-independent-failure").await;
    let tag = task_tag(&f, &f.token, task).await;
    retry(&f, &f.token, task, &tag, "retry-independent-task")
        .await
        .expect(200);
    let requeued = snapshot(&db, task).await;
    assert_eq!(requeued["steps"][&ready_sink]["state"], "ready");
    assert_eq!(requeued["steps"][&blocked_sink]["state"], "pending");
    for _ in 0..3 {
        let attempt = leased(&f, worker).await;
        commit(
            &f.db,
            &f.auth,
            worker,
            &success(&attempt, "branch completion"),
        )
        .await
        .unwrap();
    }
    assert_eq!(snapshot(&db, task).await["task"]["state"], "succeeded");
}

#[tokio::test]
async fn cancellation_can_stop_an_unclaimed_retry_without_erasing_proof() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(2), true).await, "cancel-retry-task").await;
    let (_, step) = unknown(&f, Uuid::new_v4()).await;
    non_execution(&f, &step, "confirm-before-cancel").await;
    let tag = task_tag(&f, &f.token, task).await;
    let queued = retry(&f, &f.token, task, &tag, "queue-before-cancel")
        .await
        .expect(200);
    let before = snapshot(&db, task).await;
    let cancel = request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/tasks/{task}/cancel"),
        Some(&f.token),
        Some(&json!({})),
        &[
            ("idempotency-key", "cancel-queued-retry"),
            ("if-match", queued.etag.as_deref().unwrap()),
        ],
    )
    .await
    .expect(200);
    assert_eq!(cancel.body["state"], "canceled");
    let stopped = snapshot(&db, task).await;
    assert_eq!(stopped["attempts"], before["attempts"]);
    assert_eq!(stopped["receipts"], before["receipts"]);
    assert_eq!(stopped["steps"]["echo"]["state"], "canceled");
    assert_eq!(stopped["steps"]["child"]["state"], "canceled");
    retry(
        &f,
        &f.token,
        task,
        cancel.etag.as_deref().unwrap(),
        "failed-revival",
    )
    .await
    .expect(409);
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
}

#[tokio::test]
async fn revoked_original_authority_refuses_retry_under_a_new_operator_session() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(2), false).await, "revoked-retry-task").await;
    let (_, step) = unknown(&f, Uuid::new_v4()).await;
    non_execution(&f, &step, "verify-revoked-source").await;
    let operator = login(&f.router, f.owner).await;
    let tag = task_tag(&f, &operator, task).await;
    db.execute(
        "UPDATE credentials SET status='revoked' WHERE id=(SELECT e.issuer_credential_id
        FROM tasks t JOIN execution_delegations e ON e.id=t.delegation_id WHERE t.id=$1)",
        &[&task],
    )
    .await
    .unwrap();
    let after_revoke = snapshot(&db, task).await;
    let response = retry(&f, &operator, task, &tag, "cannot-use-new-operator")
        .await
        .expect(401);
    assert_eq!(response.body["error"]["code"], "INVALID_CREDENTIAL");
    assert_eq!(snapshot(&db, task).await, after_revoke);
}

#[tokio::test]
async fn a_requeued_attempt_revalidates_the_original_authority_before_claim() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(
        &f,
        publish(&f, Some(2), false).await,
        "revoke-after-queue-task",
    )
    .await;
    let worker = Uuid::new_v4();
    let (_, step) = unknown(&f, worker).await;
    non_execution(&f, &step, "verify-before-revoke").await;
    let tag = task_tag(&f, &f.token, task).await;
    retry(&f, &f.token, task, &tag, "revoke-after-queue")
        .await
        .expect(200);
    let before = snapshot(&db, task).await;
    db.execute(
        "UPDATE credentials SET status='revoked' WHERE id=(SELECT e.issuer_credential_id
         FROM tasks t JOIN execution_delegations e ON e.id=t.delegation_id WHERE t.id=$1)",
        &[&task],
    )
    .await
    .unwrap();
    assert_eq!(claim(&f.db, &f.auth, worker).await.unwrap(), None);
    let after = snapshot(&db, task).await;
    assert_eq!(after["task"]["state"], "paused_authorization");
    assert_eq!(
        after["task"]["delegation_id"],
        before["task"]["delegation_id"]
    );
    assert_eq!(after["steps"], before["steps"]);
    assert_eq!(after["attempts"], before["attempts"]);
    assert_eq!(after["receipts"], before["receipts"]);
}

#[tokio::test]
async fn retry_rolls_back_when_waiting_for_success_audit_outlives_the_deadline() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut db = f.db.connect().await.unwrap();
    let monitor = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(2), false).await, "deadline-task").await;
    let (_, step) = unknown(&f, Uuid::new_v4()).await;
    non_execution(&f, &step, "verify-before-deadline").await;
    let tag = task_tag(&f, &f.token, task).await;
    let failed = snapshot(&db, task).await;
    let tx = db.transaction().await.unwrap();
    tx.batch_execute("LOCK TABLE audit_events IN ACCESS EXCLUSIVE MODE")
        .await
        .unwrap();
    let deadline: chrono::DateTime<chrono::Utc> = monitor.query_one(
        "UPDATE tasks SET deadline_at=clock_timestamp()+interval '6 seconds' WHERE id=$1 RETURNING deadline_at", &[&task]
    ).await.unwrap().get(0);
    let failed_after_deadline_change = snapshot(&monitor, task).await;
    assert_eq!(
        failed_after_deadline_change["task"]["revision"],
        failed["task"]["revision"]
    );
    let call = tokio::spawn(async move {
        let response = retry(&f, &f.token, task, &tag, "wait-for-audit").await;
        (f, response)
    });
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let waiting: bool = monitor.query_one("SELECT EXISTS (SELECT FROM pg_locks
                WHERE relation='audit_events'::regclass AND mode='RowExclusiveLock' AND NOT granted)", &[]).await.unwrap().get(0);
            if waiting { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("waiting on the success audit write after preparing the retry");
    monitor.query_one("SELECT pg_sleep(greatest(extract(epoch FROM $1::timestamptz-clock_timestamp()),0)+0.05)", &[&deadline]).await.unwrap();
    tx.commit().await.unwrap();
    let (f, response) = call.await.unwrap();
    response.expect(409);
    assert_eq!(snapshot(&db, task).await, failed_after_deadline_change);
    assert_eq!(claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap(), None);
}

#[tokio::test]
async fn failed_queue_event_rolls_back_retry_and_allows_same_request() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let task = submit(&f, publish(&f, Some(2), false).await, "retry-rollback-task").await;
    let (_, step) = unknown(&f, Uuid::new_v4()).await;
    non_execution(&f, &step, "verify-before-rollback").await;
    let tag = task_tag(&f, &f.token, task).await;
    let failed = snapshot(&db, task).await;
    db.batch_execute(
        "ALTER TABLE outbox ADD CONSTRAINT retry_test CHECK (topic <> 'task.step.retry_requested')",
    )
    .await
    .unwrap();
    retry(&f, &f.token, task, &tag, "retry-after-storage-failure")
        .await
        .expect(503);
    assert_eq!(snapshot(&db, task).await, failed);
    db.batch_execute("ALTER TABLE outbox DROP CONSTRAINT retry_test")
        .await
        .unwrap();
    retry(&f, &f.token, task, &tag, "retry-after-storage-failure")
        .await
        .expect(200);
    assert_eq!(leased(&f, Uuid::new_v4()).await["number"], 2);
}
