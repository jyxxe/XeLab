//! Trusted worker operations on attempts created by HTTP admission and claim.
#[path = "task_worker_lease_dispatch.rs"]
mod dispatch;

use super::{request, Fixture};
use axum::http::Method;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{
    contracts::{id, validate},
    task_engine::{claim, commit, prepare_dispatch, renew, AttemptLease},
};

fn spec(f: &Fixture, timeout_ms: u32) -> Value {
    let mut spec = f.spec("worker lease");
    spec["steps"][0]["timeout_ms"] = json!(timeout_ms);
    spec
}

async fn submit(f: &Fixture, spec: &Value, input: Value, key: &str) -> Uuid {
    let workflow = f.publish(&f.token, spec, key).await.expect(201).body;
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(&f.token),
        Some(&json!({"workflow_id": workflow["id"], "input": input})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201);
    id(&task.body, "id").unwrap()
}

async fn leased(f: &Fixture) -> (AttemptLease, Value) {
    let worker_id = Uuid::new_v4();
    let attempt = claim(&f.db, &f.auth, worker_id)
        .await
        .unwrap()
        .expect("ready step");
    let lease = AttemptLease {
        worker_id,
        attempt_id: id(&attempt, "id").unwrap(),
        execution_id: id(&attempt, "execution_id").unwrap(),
        fence: attempt["fence"].as_i64().unwrap(),
    };
    (lease, attempt)
}

async fn succeed(f: &Fixture, lease: &AttemptLease, message: &str) -> Value {
    let result = json!({"attempt_id":lease.attempt_id,"execution_id":lease.execution_id,
        "fence":lease.fence,"state":"succeeded","output":{"message":message},"error":null});
    commit(&f.db, &f.auth, lease.worker_id, &result)
        .await
        .unwrap()
}

fn time(value: &Value, field: &str) -> DateTime<Utc> {
    value[field].as_str().unwrap().parse().unwrap()
}

async fn db_time(db: &Client) -> DateTime<Utc> {
    db.query_one("SELECT clock_timestamp()", &[])
        .await
        .unwrap()
        .get(0)
}

async fn dispatch_request(
    f: &Fixture,
    lease: &AttemptLease,
    deadline: DateTime<Utc>,
    timeout_ms: u32,
) -> Value {
    let prepared = prepare_dispatch(&f.db, &f.auth, lease).await.unwrap();
    assert_eq!(prepared.deadline_at, deadline);
    assert_eq!(prepared.request["timeout_ms"], timeout_ms);
    prepared.request
}

async fn stored(db: &Client, attempt: Uuid) -> Value {
    db.query_one(
        "SELECT to_jsonb(a) FROM attempts a WHERE id=$1",
        &[&attempt],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
async fn successive_heartbeats_extend_one_attempt_within_its_original_timeout() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    for (key, timeout_ms) in [("sliding", 120_000), ("absolute", 20_000)] {
        submit(&f, &spec(&f, timeout_ms), json!({}), key).await;
        let (lease, first) = leased(&f).await;
        let step = id(&first, "step_id").unwrap();
        let absolute = time(&first, "created_at") + Duration::milliseconds(timeout_ms.into());
        let request = dispatch_request(&f, &lease, absolute, timeout_ms).await;
        let mut previous = first.clone();
        for _ in 0..2 {
            let before = db_time(&db).await;
            let renewed = renew(&f.db, &f.auth, &lease).await.unwrap();
            validate("Attempt", &renewed).unwrap();
            for field in [
                "id",
                "step_id",
                "execution_id",
                "fence",
                "number",
                "idempotency_key",
                "created_at",
                "checkpoint_blob_id",
            ] {
                assert_eq!(renewed[field], first[field], "{field}");
            }
            assert_eq!(
                renewed["revision"],
                previous["revision"].as_i64().unwrap() + 1
            );
            let expires = time(&renewed, "lease_expires_at");
            if key == "absolute" {
                assert_eq!(expires, absolute);
            } else {
                let after = db_time(&db).await;
                assert!(
                    (before + Duration::seconds(30)..=after + Duration::seconds(30))
                        .contains(&expires)
                );
                assert!(expires > time(&previous, "lease_expires_at"));
            }
            previous = renewed;
        }
        // Heartbeats change the lease expiry, while the Broker fingerprints the entire request.
        assert_eq!(
            dispatch_request(&f, &lease, absolute, timeout_ms).await,
            request
        );
        let history = f
            .read(
                &f.token,
                &format!("/api/platform/v1/task-steps/{step}/attempts"),
            )
            .await
            .expect(200);
        validate("AttemptPage", &history.body).unwrap();
        assert_eq!(history.body["items"], json!([previous]));
        succeed(&f, &lease, "after two heartbeats").await;
    }
}

#[tokio::test]
async fn heartbeat_and_dispatch_observe_frozen_and_current_authority_and_task_deadline() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let frozen: DateTime<Utc> = db
        .query_one(
            "UPDATE grants SET expires_at=clock_timestamp()+interval '25 seconds'
         WHERE subject_id=$1 AND scope->>'kind'='capability' RETURNING expires_at",
            &[&f.owner],
        )
        .await
        .unwrap()
        .get(0);
    let task = submit(&f, &spec(&f, 120_000), json!({}), "cutoffs").await;
    let (lease, _) = leased(&f).await;
    db.execute(
        "UPDATE grants SET expires_at=NULL WHERE subject_id=$1 AND scope->>'kind'='capability'",
        &[&f.owner],
    )
    .await
    .unwrap();
    let request = dispatch_request(&f, &lease, frozen, 120_000).await;
    let renewed = renew(&f.db, &f.auth, &lease).await.unwrap();
    assert_eq!(time(&renewed, "lease_expires_at"), frozen);

    // Tighten one independent bound at a time after the worker already holds a lease.
    for (change, key, seconds) in [
        ("UPDATE grants SET expires_at=$2 WHERE subject_id=$1 AND scope->>'kind'='capability'", f.owner, 5),
        ("UPDATE execution_delegations SET expires_at=$2 WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)", task, 10),
        ("UPDATE tasks SET deadline_at=$2 WHERE id=$1", task, 15),
    ] {
        let cutoff = frozen - Duration::seconds(seconds);
        assert_eq!(db.execute(change, &[&key, &cutoff]).await.unwrap(), 1);
        assert_eq!(dispatch_request(&f, &lease, cutoff, 120_000).await, request);
        let renewed = renew(&f.db, &f.auth, &lease).await.unwrap();
        assert_eq!(time(&renewed, "lease_expires_at"), cutoff);
    }

    // A 120s step can still prepare during its last positive execution window.
    // If scheduling consumed that window, the database clock must confirm expiry.
    let cutoff: DateTime<Utc> = db
        .query_one(
            "UPDATE tasks SET deadline_at=clock_timestamp()+interval '99 milliseconds'
         WHERE id=$1 RETURNING deadline_at",
            &[&task],
        )
        .await
        .unwrap()
        .get(0);
    match prepare_dispatch(&f.db, &f.auth, &lease).await {
        Ok(prepared) => {
            assert_eq!(prepared.deadline_at, cutoff);
            assert_eq!(prepared.request, request);
        }
        Err(error) => {
            assert_eq!(error.code, "STATE_CONFLICT", "{error}");
            assert!(
                db_time(&db).await >= cutoff,
                "refused while the hard deadline was still live"
            );
        }
    }
}

async fn reject_stale(f: &Fixture, lease: &AttemptLease) {
    let renewal = renew(&f.db, &f.auth, lease).await.unwrap_err();
    assert_eq!(renewal.code, "STATE_CONFLICT", "{renewal}");
    let dispatch = prepare_dispatch(&f.db, &f.auth, lease)
        .await
        .expect_err("stale dispatch");
    assert_eq!(dispatch.code, "STATE_CONFLICT", "{dispatch}");
}

#[tokio::test]
async fn stale_identity_expired_and_committed_attempts_cannot_resume() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let spec = spec(&f, 120_000);
    submit(&f, &spec, json!({}), "stale").await;
    let (lease, attempt) = leased(&f).await;
    let before = stored(&db, lease.attempt_id).await;
    for wrong in [
        AttemptLease {
            worker_id: Uuid::new_v4(),
            ..lease
        },
        AttemptLease {
            attempt_id: Uuid::new_v4(),
            ..lease
        },
        AttemptLease {
            execution_id: Uuid::new_v4(),
            ..lease
        },
        AttemptLease {
            fence: lease.fence + 1,
            ..lease
        },
    ] {
        reject_stale(&f, &wrong).await;
    }
    let step = id(&attempt, "step_id").unwrap();
    db.execute("UPDATE task_steps SET fence=fence+1 WHERE id=$1", &[&step])
        .await
        .unwrap();
    reject_stale(&f, &lease).await;
    assert_eq!(stored(&db, lease.attempt_id).await, before);

    submit(&f, &spec, json!({}), "expired").await;
    let (expired, _) = leased(&f).await;
    db.execute(
        "UPDATE attempts SET lease_expires_at=clock_timestamp() WHERE id=$1",
        &[&expired.attempt_id],
    )
    .await
    .unwrap();
    let before = stored(&db, expired.attempt_id).await;
    reject_stale(&f, &expired).await;
    assert_eq!(stored(&db, expired.attempt_id).await, before);

    // The task remains running after the first step commits.
    let mut spec = spec;
    let mut next = spec["steps"][0].clone();
    next["id"] = json!("next");
    next["depends_on"] = json!(["echo"]);
    spec["steps"].as_array_mut().unwrap().push(next);
    submit(&f, &spec, json!({}), "terminal").await;
    let (terminal, _) = leased(&f).await;
    succeed(&f, &terminal, "done").await;
    let before = stored(&db, terminal.attempt_id).await;
    reject_stale(&f, &terminal).await;
    assert_eq!(stored(&db, terminal.attempt_id).await, before);
}

#[tokio::test]
async fn revoked_capability_blocks_renewal_and_dispatch_on_live_leases() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let spec = spec(&f, 120_000);
    let mut active = Vec::new();
    for operation in ["renew", "dispatch"] {
        let task = submit(&f, &spec, json!({}), operation).await;
        let (lease, _) = leased(&f).await;
        let before = stored(&db, lease.attempt_id).await;
        active.push((operation, task, lease, before));
    }
    assert_eq!(db.execute(
        "UPDATE grants SET status='revoked' WHERE subject_id=$1 AND scope->>'kind'='capability'",
        &[&f.owner],
    ).await.unwrap(), 1);
    for (operation, task, lease, before) in active {
        let error = if operation == "renew" {
            renew(&f.db, &f.auth, &lease).await.unwrap_err()
        } else {
            prepare_dispatch(&f.db, &f.auth, &lease)
                .await
                .expect_err("revoked dispatch")
        };
        assert_eq!(error.code, "PERMISSION_DENIED", "{operation}: {error}");
        assert_eq!(stored(&db, lease.attempt_id).await, before);
        let state: String = db
            .query_one("SELECT state FROM tasks WHERE id=$1", &[&task])
            .await
            .unwrap()
            .get(0);
        assert_eq!(state, "paused_authorization", "{operation}");
    }
}

#[tokio::test]
async fn provider_lock_crossing_expiry_blocks_heartbeat_and_dispatch() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let spec = spec(&f, 120_000);
    submit(&f, &spec, json!({}), "heartbeat-wait").await;
    let (heartbeat, _) = leased(&f).await;
    submit(&f, &spec, json!({}), "dispatch-wait").await;
    let (dispatch, _) = leased(&f).await;
    let ids = vec![heartbeat.attempt_id, dispatch.attempt_id];
    let attempts = db
        .query(
            "UPDATE attempts SET lease_expires_at=statement_timestamp()+interval '4 seconds'
         WHERE id=ANY($1) RETURNING lease_expires_at",
            &[&ids],
        )
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2);
    let cutoff: DateTime<Utc> = attempts[0].get(0);
    let mut blocker = f.db.connect().await.unwrap();
    let hold = blocker.transaction().await.unwrap();
    hold.query_one(
        "SELECT id FROM installations WHERE id=$1 FOR UPDATE",
        &[&f.installation],
    )
    .await
    .unwrap();
    let before = stored(&db, heartbeat.attempt_id).await;
    let (renewed, prepared, ()) = tokio::join!(
        renew(&f.db, &f.auth, &heartbeat),
        prepare_dispatch(&f.db, &f.auth, &dispatch),
        async {
            loop {
                // Each test has its own database. Both workers must be waiting while live.
                let row = db
                    .query_one(
                        "SELECT clock_timestamp() < $1, (SELECT count(*) FROM pg_stat_activity
                     WHERE datname=current_database() AND wait_event_type='Lock')",
                        &[&cutoff],
                    )
                    .await
                    .unwrap();
                assert!(
                    row.get::<_, bool>(0),
                    "workers did not reach the lock before expiry"
                );
                if row.get::<_, i64>(1) == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            db.query_one("SELECT pg_sleep_until($1)", &[&cutoff])
                .await
                .unwrap();
            hold.rollback().await.unwrap();
        },
    );
    assert_eq!(renewed.unwrap_err().code, "STATE_CONFLICT");
    assert_eq!(
        prepared.expect_err("dispatch waited past expiry").code,
        "STATE_CONFLICT"
    );
    assert_eq!(stored(&db, heartbeat.attempt_id).await, before);
}
