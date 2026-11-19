//! Terminal results from overlapping leases and a lease lost during persistence.
use super::{request, Fixture};
use axum::http::Method;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{
    contracts::id,
    task_engine::{claim, commit},
};

async fn create_task(f: &Fixture, key: &str, graph: &[(&str, &[&str])], timeout_ms: u32) -> Uuid {
    let mut spec = f.spec(key);
    let template = &spec["steps"][0];
    spec["steps"] = graph
        .iter()
        .map(|(name, dependencies)| {
            let mut step = template.clone();
            step["id"] = json!(name);
            step["depends_on"] = json!(dependencies);
            step["timeout_ms"] = json!(timeout_ms);
            step
        })
        .collect();
    let workflow = f.publish(&f.token, &spec, key).await.expect(201).body;
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(&f.token),
        Some(&json!({"workflow_id": workflow["id"], "input": {}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201)
    .body;
    id(&task, "id").unwrap()
}

fn result(attempt: &Value, state: &str) -> Value {
    let code = match state {
        "failed" => Some("INVALID_ARGUMENT"),
        "unknown" => Some("OUTCOME_UNKNOWN"),
        "succeeded" => None,
        _ => unreachable!(),
    };
    let error = code.map(|code| {
        json!({"code":code, "message":"Worker result",
        "retryable":false, "violations":[], "retry_after_ms":null})
    });
    let output = (state == "succeeded").then(|| json!({"message":"done"}));
    json!({"attempt_id":attempt["id"], "execution_id":attempt["execution_id"],
           "fence":attempt["fence"], "state":state, "output":output, "error":error})
}

// Check the stored rows, including the live-attempt invariant used by the
// offline snapshot gate. The two workers may have claimed either root first.
async fn parallel_state(
    db: &Client,
    task: Uuid,
    first: &Value,
    sibling: &Value,
    outcome: &str,
) -> (Value, Value) {
    let row = db
        .query_one(
            "SELECT jsonb_build_object('state', t.state, 'error', t.error, 'output', t.output),
                    jsonb_object_agg(coalesce(a.id::text, s.step_key),
                        jsonb_build_array(s.state, a.state, r.state, s.attempt_count, s.output))
         FROM tasks t JOIN task_steps s ON s.task_id=t.id LEFT JOIN attempts a ON a.step_id=s.id
         LEFT JOIN attempt_commit_receipts r ON r.attempt_id=a.id
         WHERE t.id=$1 GROUP BY t.id",
            &[&task],
        )
        .await
        .unwrap();
    let steps: Value = row.get(1);
    assert_eq!(
        steps[first["id"].as_str().unwrap()],
        json!([outcome, outcome, outcome, 1, null])
    );
    for key in ["join", "after-left", "after-right"] {
        assert_eq!(steps[key], json!(["pending", null, null, 0, null]), "{key}");
    }
    (row.get(0), steps[sibling["id"].as_str().unwrap()].clone())
}

#[tokio::test]
async fn sibling_success_preserves_failed_or_unknown_predecessor() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let graph: &[(&str, &[&str])] = &[
        ("left", &[]),
        ("right", &[]),
        ("join", &["left", "right"]),
        ("after-left", &["left"]),
        ("after-right", &["right"]),
    ];
    for outcome in ["failed", "unknown"] {
        let task = create_task(&f, outcome, graph, 30_000).await;
        let first_worker = Uuid::new_v4();
        let sibling_worker = Uuid::new_v4();
        let first = claim(&f.db, &f.auth, first_worker).await.unwrap().unwrap();
        let sibling = claim(&f.db, &f.auth, sibling_worker)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first["step_id"], sibling["step_id"]);

        let first_result = result(&first, outcome);
        let sibling_result = result(&sibling, "succeeded");
        commit(&f.db, &f.auth, first_worker, &first_result)
            .await
            .unwrap();
        let (draining, live) = parallel_state(&db, task, &first, &sibling, outcome).await;
        assert_eq!(draining["state"], "running");
        assert_eq!(live, json!(["running", "leased", null, 1, null]));

        commit(&f.db, &f.auth, sibling_worker, &sibling_result)
            .await
            .unwrap();
        let (finished, settled) = parallel_state(&db, task, &first, &sibling, outcome).await;
        assert_eq!(
            settled,
            json!([
                "succeeded",
                "succeeded",
                "succeeded",
                1,
                sibling_result["output"]
            ])
        );
        assert_eq!(
            finished,
            json!({"state":outcome,"error":first_result["error"],"output":null})
        );
        assert_eq!(claim(&f.db, &f.auth, sibling_worker).await.unwrap(), None);
    }
}

async fn stored_result_rows(db: &Client, task: Uuid) -> Value {
    db.query_one(
        "SELECT jsonb_build_object('task', to_jsonb(t),
           'steps', (SELECT jsonb_agg(to_jsonb(s) ORDER BY s.id) FROM task_steps s WHERE s.task_id=t.id),
           'attempts', (SELECT jsonb_agg(to_jsonb(a) ORDER BY a.id) FROM attempts a
                         JOIN task_steps s ON a.step_id=s.id WHERE s.task_id=t.id),
           'receipts', (SELECT jsonb_agg(to_jsonb(r) ORDER BY r.attempt_id) FROM attempt_commit_receipts r
                         JOIN attempts a ON a.id=r.attempt_id JOIN task_steps s ON s.id=a.step_id WHERE s.task_id=t.id),
           'outbox', (SELECT jsonb_agg(to_jsonb(b) ORDER BY b.id) FROM outbox b))
         FROM tasks t WHERE t.id=$1", &[&task],
    ).await.unwrap().get(0)
}

#[tokio::test]
async fn lease_expiry_while_writing_outbox_rolls_back_result_and_audits_rejection() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let mut blocker = f.db.connect().await.unwrap();
    let graph: &[(&str, &[&str])] = &[("root", &[]), ("next", &["root"])];
    let task = create_task(&f, "outbox-wait", graph, 5_000).await;
    let worker = Uuid::new_v4();
    let attempt = claim(&f.db, &f.auth, worker).await.unwrap().unwrap();
    let expires: DateTime<Utc> = attempt["lease_expires_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let execution = id(&attempt, "execution_id").unwrap();
    let before = stored_result_rows(&db, task).await;
    let hold = blocker.transaction().await.unwrap();
    // SHARE blocks the outbox INSERT after the result writes have started.
    hold.batch_execute("LOCK TABLE outbox IN SHARE MODE")
        .await
        .unwrap();
    let pending = tokio::spawn(async move {
        commit(&f.db, &f.auth, worker, &result(&attempt, "succeeded")).await
    });

    // Observe the write lock request while its lease is still valid. Use the
    // database clock for both the observation and the eventual release.
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let row = db.query_one(
                "SELECT clock_timestamp() < $1, EXISTS (
                    SELECT FROM pg_locks WHERE relation='outbox'::regclass
                      AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                      AND mode='RowExclusiveLock' AND NOT granted)", &[&expires],
            ).await.unwrap();
            assert!(row.get::<_, bool>(0), "commit did not reach outbox during its lease");
            if row.get::<_, bool>(1) { break; }
            assert!(!pending.is_finished(), "commit returned before the outbox write");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        db.query_one("SELECT pg_sleep(greatest(0, extract(epoch FROM ($1::timestamptz - clock_timestamp())))::float8)", &[&expires])
            .await.unwrap();
    }).await.expect("outbox wait exceeded lease observation window");
    hold.rollback().await.unwrap();

    let error = tokio::time::timeout(Duration::from_secs(6), pending)
        .await
        .expect("commit must finish after lock release")
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "STATE_CONFLICT");
    assert_eq!(stored_result_rows(&db, task).await, before);
    let audit: Value = db
        .query_one(
            "SELECT jsonb_agg(jsonb_build_array(outcome,reason_code,resource))
             FROM audit_events WHERE request_id=$1 AND action='task.attempt.commit'",
            &[&execution],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        audit,
        json!([["denied", "STALE_ATTEMPT_COMMIT",
        {"kind":"task", "match":"ids", "ids":[task]}]])
    );
}
