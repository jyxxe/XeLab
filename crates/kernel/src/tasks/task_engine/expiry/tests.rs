use super::*;
use crate::{
    backups::tests::Fixture,
    task_engine::{commit, renew, AttemptLease},
    tasks::snapshot::validate_snapshot,
};
use serde_json::{json, Value};
use tokio_postgres::Client;

async fn task(c: &Client, state: &str, deadline_expired: bool) -> Uuid {
    let id = Uuid::new_v4();
    c.execute(
        "WITH workflow AS (
             INSERT INTO workflows(id,owner_id,name,version,spec,status,created_at,updated_at)
             SELECT gen_random_uuid(),owner_id,$1::uuid::text,1,'{\"steps\":[]}', 'active',now(),now()
               FROM deployments RETURNING id,owner_id
         ), delegation AS (
             INSERT INTO execution_delegations(id,subject_id,maximum_rules,run_as,expires_at,status,created_at)
             SELECT gen_random_uuid(),owner_id,'[]','user',now()+interval '1 hour','active',now()
               FROM workflow RETURNING id
         )
         INSERT INTO tasks(id,owner_id,workflow_id,delegation_id,input,deadline_at,state,created_at,updated_at)
         SELECT $1,workflow.owner_id,workflow.id,delegation.id,'{}',
             now()+CASE WHEN $3 THEN interval '-1 second' ELSE interval '1 hour' END,$2,now(),now()
           FROM workflow CROSS JOIN delegation",
        &[&id, &state, &deadline_expired],
    )
    .await
    .unwrap();
    id
}

async fn attempt(c: &Client, task: Uuid, key: &str, expired: bool) -> AttemptLease {
    let lease = AttemptLease {
        worker_id: Uuid::new_v4(),
        attempt_id: Uuid::new_v4(),
        execution_id: Uuid::new_v4(),
        fence: 1,
    };
    c.execute(
        "WITH step AS (
             INSERT INTO task_steps(id,task_id,step_key,state,fence,attempt_count,created_at,updated_at)
             VALUES(gen_random_uuid(),$1,$2,'running',1,1,now(),now()) RETURNING id
         )
         INSERT INTO attempts(id,step_id,number,execution_id,idempotency_key,fence,
                              lease_expires_at,state,created_at,updated_at,worker_id)
         SELECT $3,id,1,$4,id::text,1,
             now()+CASE WHEN $6 THEN interval '-1 second' ELSE interval '10 minutes' END,
             'leased',now(),now(),$5 FROM step",
        &[&task, &key, &lease.attempt_id, &lease.execution_id, &lease.worker_id, &expired],
    )
    .await
    .unwrap();
    lease
}

async fn outcome(c: &Client, lease: AttemptLease) -> Value {
    c.query_one(
        "SELECT jsonb_build_array(a.state,s.state,t.state,t.error->>'code',
             EXISTS (SELECT FROM attempt_commit_receipts r WHERE r.attempt_id=a.id),
             (SELECT count(*) FROM outbox o WHERE o.topic='task.step.lease_expired'
                AND o.aggregate_id=s.id AND o.payload->>'attempt_id'=a.id::text))
         FROM attempts a JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
         WHERE a.id=$1",
        &[&lease.attempt_id],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
async fn expiry_preserves_the_live_sibling_and_fences_late_workers() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = &f.app.db;
    let c = db.connect().await.unwrap();
    let task = task(&c, "running", false).await;
    let expired = attempt(&c, task, "expired", true).await;
    let live = attempt(&c, task, "live", false).await;
    c.execute(
        "INSERT INTO task_steps(id,task_id,step_key,state,created_at,updated_at)
         VALUES(gen_random_uuid(),$1,'child','pending',now(),now())",
        &[&task],
    )
    .await
    .unwrap();
    let original = c
        .query_one(
            "SELECT to_jsonb(a)-ARRAY['state','updated_at','revision'] FROM attempts a WHERE id=$1",
            &[&expired.attempt_id],
        )
        .await
        .unwrap()
        .get::<_, Value>(0);

    expire_leases(db).await.unwrap();
    assert_eq!(
        outcome(&c, expired).await,
        json!(["expired", "unknown", "running", "OUTCOME_UNKNOWN", false, 1])
    );
    assert_eq!(
        outcome(&c, live).await,
        json!(["leased", "running", "running", "OUTCOME_UNKNOWN", false, 0])
    );
    assert_eq!(
        c.query_one(
            "SELECT to_jsonb(a)-ARRAY['state','updated_at','revision'] FROM attempts a WHERE id=$1",
            &[&expired.attempt_id],
        )
        .await
        .unwrap()
        .get::<_, Value>(0),
        original
    );
    validate_snapshot(&c).await.unwrap();
    assert_eq!(
        renew(db, &f.app.auth, &expired).await.unwrap_err().code,
        "STATE_CONFLICT"
    );
    let result = json!({
        "attempt_id":expired.attempt_id,"execution_id":expired.execution_id,"fence":expired.fence,
        "state":"succeeded","output":{},"error":null
    });
    assert_eq!(
        commit(db, &f.app.auth, expired.worker_id, &result)
            .await
            .unwrap_err()
            .code,
        "STATE_CONFLICT"
    );
    c.execute(
        "UPDATE attempts SET state='running',lease_expires_at=clock_timestamp() WHERE id=$1",
        &[&live.attempt_id],
    )
    .await
    .unwrap();

    expire_leases(db).await.unwrap();
    expire_leases(db).await.unwrap();
    for lease in [expired, live] {
        assert_eq!(
            outcome(&c, lease).await,
            json!(["expired", "unknown", "unknown", "OUTCOME_UNKNOWN", false, 1])
        );
    }
    let child = c.query_one(
        "SELECT state,(SELECT count(*) FROM outbox WHERE topic IN ('task.step.ready','task.step.committed'))
         FROM task_steps WHERE task_id=$1 AND step_key='child'",
        &[&task],
    ).await.unwrap();
    assert_eq!(child.get::<_, &str>(0), "pending");
    assert_eq!(child.get::<_, i64>(1), 0);
    validate_snapshot(&c).await.unwrap();
}

#[tokio::test]
async fn expiry_honors_recovery_and_deadline_without_overwriting_suspended_tasks() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = &f.app.db;
    let c = db.connect().await.unwrap();
    let mut parked = Vec::new();
    for state in [
        "waiting_dependency",
        "paused_authorization",
        "cancel_requested",
    ] {
        let id = task(&c, state, true).await;
        parked.push((state, attempt(&c, id, "deadline-expired", false).await));
    }
    let running = task(&c, "running", false).await;
    let legacy = attempt(&c, running, "unowned", true).await;
    c.execute(
        "UPDATE attempts SET worker_id=NULL WHERE id=$1",
        &[&legacy.attempt_id],
    )
    .await
    .unwrap();
    let stale = attempt(&c, running, "wrong-fence", true).await;
    c.execute(
        "UPDATE task_steps s SET fence=2 FROM attempts a WHERE a.step_id=s.id AND a.id=$1",
        &[&stale.attempt_id],
    )
    .await
    .unwrap();
    c.execute(
        "INSERT INTO platform_recovery_state(recovery_id,backup_id,phase)
         VALUES(gen_random_uuid(),gen_random_uuid(),'importing')",
        &[],
    )
    .await
    .unwrap();
    expire_leases(db).await.unwrap();
    for &(state, lease) in &parked {
        assert_eq!(
            outcome(&c, lease).await,
            json!(["leased", "running", state, null, false, 0])
        );
    }
    c.execute("UPDATE platform_recovery_state SET phase='ready'", &[])
        .await
        .unwrap();

    expire_leases(db).await.unwrap();
    for (state, lease) in parked {
        assert_eq!(
            outcome(&c, lease).await,
            json!(["expired", "unknown", state, null, false, 1])
        );
    }
    for lease in [legacy, stale] {
        assert_eq!(
            outcome(&c, lease).await,
            json!(["leased", "running", "running", null, false, 0])
        );
    }
}

#[tokio::test]
async fn expiry_skips_contended_parent_and_retries_a_rolled_back_outbox_write() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = &f.app.db;
    let c = db.connect().await.unwrap();
    let id = task(&c, "running", false).await;
    let locked = attempt(&c, id, "parent-locked", true).await;
    let other = attempt(&c, task(&c, "running", false).await, "available", true).await;
    let mut connection = db.connect().await.unwrap();
    let authorized = connection.transaction().await.unwrap();
    crate::db::auth_lock(&authorized, true).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), expire_leases(db))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        outcome(&c, other).await,
        json!(["leased", "running", "running", null, false, 0])
    );
    authorized.rollback().await.unwrap();

    let blocked = connection.transaction().await.unwrap();
    blocked
        .query_one("SELECT id FROM tasks WHERE id=$1 FOR UPDATE", &[&id])
        .await
        .unwrap();

    expire_leases(db).await.unwrap();
    assert_eq!(
        outcome(&c, locked).await,
        json!(["leased", "running", "running", null, false, 0])
    );
    assert_eq!(
        outcome(&c, other).await,
        json!(["expired", "unknown", "unknown", "OUTCOME_UNKNOWN", false, 1])
    );
    blocked.rollback().await.unwrap();
    c.batch_execute(
        "CREATE FUNCTION fail_expiry_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected expiry outbox failure'; END; $$;
         CREATE TRIGGER fail_expiry_outbox BEFORE INSERT ON outbox
         FOR EACH ROW EXECUTE FUNCTION fail_expiry_outbox()",
    )
    .await
    .unwrap();
    expire_leases(db).await.unwrap_err();
    assert_eq!(
        outcome(&c, locked).await,
        json!(["leased", "running", "running", null, false, 0])
    );
    c.batch_execute("DROP TRIGGER fail_expiry_outbox ON outbox")
        .await
        .unwrap();

    let (first, second) = tokio::join!(expire_leases(db), expire_leases(db));
    first.unwrap();
    second.unwrap();
    assert_eq!(
        outcome(&c, locked).await,
        json!(["expired", "unknown", "unknown", "OUTCOME_UNKNOWN", false, 1])
    );
    validate_snapshot(&c).await.unwrap();
}

#[tokio::test]
async fn expiry_reconciles_only_one_batch_per_tick() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = &f.app.db;
    let c = db.connect().await.unwrap();
    let task = task(&c, "running", false).await;
    let mut leases = Vec::new();
    for index in 0..=EXPIRY_BATCH_SIZE {
        leases.push(attempt(&c, task, &format!("step-{index}"), true).await);
    }
    expire_leases(db).await.unwrap();
    let open = c
        .query_one(
            "SELECT t.state, (SELECT count(*) FROM attempts a JOIN task_steps s ON s.id=a.step_id
           WHERE s.task_id=t.id AND a.state='leased') FROM tasks t WHERE t.id=$1",
            &[&task],
        )
        .await
        .unwrap();
    assert_eq!(open.get::<_, &str>(0), "running");
    assert_eq!(open.get::<_, i64>(1), 1);

    expire_leases(db).await.unwrap();
    for lease in leases {
        assert_eq!(
            outcome(&c, lease).await,
            json!(["expired", "unknown", "unknown", "OUTCOME_UNKNOWN", false, 1])
        );
    }
    validate_snapshot(&c).await.unwrap();
}
