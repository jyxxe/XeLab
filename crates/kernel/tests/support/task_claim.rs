//! First-attempt claims through the trusted worker entry point and the workflow API.
use super::{login, request, Fixture, Reply, PASSWORD};
use axum::http::Method;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::{Client, Row};
use uuid::Uuid;
use xelab_kernel::{contracts, task_engine::claim};

async fn publish(f: &Fixture, spec: &Value) -> Uuid {
    let workflow = f
        .publish(&f.token, spec, "claim-workflow")
        .await
        .expect(201);
    contracts::id(&workflow.body, "id").unwrap()
}

async fn submit(f: &Fixture, workflow: Uuid, token: &str, key: &str) -> Uuid {
    let task = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(token),
        Some(&json!({"workflow_id": workflow, "input": {}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201);
    contracts::id(&task.body, "id").unwrap()
}

async fn next(f: &Fixture) -> Option<Value> {
    claim(&f.db, &f.auth, Uuid::new_v4()).await.unwrap()
}

async fn step(db: &Client, task: Uuid, key: &str) -> Row {
    db.query_one(
        "SELECT t.state AS task_state, s.id, s.state, s.attempt_count, s.fence,
                (SELECT count(*) FROM attempts WHERE step_id=s.id) AS attempts
         FROM tasks t JOIN task_steps s ON t.id=s.task_id WHERE t.id=$1 AND s.step_key=$2",
        &[&task, &key],
    )
    .await
    .unwrap()
}

fn assert_unclaimed(row: &Row) {
    assert_ne!(row.get::<_, String>("state"), "running");
    assert_eq!(row.get::<_, i64>("attempts"), 0);
    assert_eq!(row.get::<_, i32>("attempt_count"), 0);
    assert_eq!(row.get::<_, i64>("fence"), 0);
}

async fn assert_paused(f: &Fixture, db: &Client, task: Uuid) {
    assert_eq!(next(f).await, None);
    let root = step(db, task, "echo").await;
    assert_eq!(root.get::<_, String>("task_state"), "paused_authorization");
    assert_unclaimed(&root);
}

#[tokio::test]
async fn contending_workers_claim_a_single_ready_step_once() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let spec = f.spec("contended");
    let timeout_ms = spec["steps"][0]["timeout_ms"].as_i64().unwrap();
    let workflow = publish(&f, &spec).await;
    let task = submit(&f, workflow, &f.token, "contended").await;
    let mut db = f.db.connect().await.unwrap();

    // A separate backend holds the only candidate. The worker must finish
    // while the lock is still held, leaving the row available after release.
    let lock = db.transaction().await.unwrap();
    let root: Uuid = lock
        .query_one(
            "SELECT id FROM task_steps WHERE task_id=$1 FOR UPDATE",
            &[&task],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), next(&f))
            .await
            .expect("claim blocked on the locked step"),
        None
    );
    lock.rollback().await.unwrap();

    let (left, right) = tokio::join!(next(&f), next(&f));
    let attempt = match (left, right) {
        (Some(value), None) | (None, Some(value)) => value,
        results => panic!("expected one successful claim, got {results:?}"),
    };
    contracts::validate("Attempt", &attempt).unwrap();
    assert_eq!(attempt["step_id"], json!(root));
    assert_eq!(attempt["number"], 1);
    assert_eq!(attempt["state"], "leased");
    assert_eq!(attempt["revision"], 1);
    assert_eq!(attempt["fence"], 1);
    assert_eq!(attempt["checkpoint_blob_id"], Value::Null);
    let issued: DateTime<Utc> = attempt["created_at"].as_str().unwrap().parse().unwrap();
    let expires: DateTime<Utc> = attempt["lease_expires_at"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!(expires > issued);
    assert!(expires - issued <= chrono::Duration::milliseconds(timeout_ms));

    let row = step(&db, task, "echo").await;
    assert_eq!(row.get::<_, String>("task_state"), "running");
    assert_eq!(row.get::<_, String>("state"), "running");
    assert_eq!(row.get::<_, i32>("attempt_count"), 1);
    assert_eq!(row.get::<_, i64>("fence"), 1);
    assert_eq!(row.get::<_, i64>("attempts"), 1);
    let history = f
        .read(
            &f.token,
            &format!("/api/platform/v1/task-steps/{root}/attempts"),
        )
        .await
        .expect(200);
    assert_eq!(history.body["items"], json!([attempt]));
}

#[tokio::test]
async fn ready_step_requires_its_predecessor_to_have_succeeded() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut db = f.db.connect().await.unwrap();
    let mut spec = f.spec("dependency");
    let mut downstream = spec["steps"][0].clone();
    downstream["id"] = json!("downstream");
    downstream["depends_on"] = json!(["echo"]);
    spec["steps"].as_array_mut().unwrap().push(downstream);
    let workflow = publish(&f, &spec).await;
    let task = submit(&f, workflow, &f.token, "dependency").await;
    let root = next(&f).await.unwrap();
    let upstream = contracts::id(&root, "step_id").unwrap();
    assert_eq!(upstream, step(&db, task, "echo").await.get::<_, Uuid>("id"));
    let child: Uuid = db
        .query_one(
            "UPDATE task_steps SET state='ready' WHERE task_id=$1 AND step_key='downstream'
         AND state='pending' RETURNING id",
            &[&task],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(next(&f).await, None);
    assert_unclaimed(&step(&db, task, "downstream").await);

    // Complete the predecessor in the fixture. The same downstream row can now be claimed.
    let tx = db.transaction().await.unwrap();
    tx.execute(
        "UPDATE attempts SET state='succeeded' WHERE step_id=$1",
        &[&upstream],
    )
    .await
    .unwrap();
    tx.execute(
        "UPDATE task_steps SET state='succeeded', output=$2 WHERE id=$1",
        &[&upstream, &json!({"message": "dependency"})],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(next(&f).await.unwrap()["step_id"], json!(child));
}

#[tokio::test]
async fn ineligible_tasks_are_skipped_on_first_claim() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &f.spec("claim filters")).await;
    let db = f.db.connect().await.unwrap();
    for (key, change) in [
        (
            "deadline",
            "UPDATE tasks SET deadline_at=now()-interval '1 second' WHERE id=$1",
        ),
        (
            "cancel-requested",
            "UPDATE tasks SET state='cancel_requested' WHERE id=$1",
        ),
        ("canceled", "UPDATE tasks SET state='canceled' WHERE id=$1"),
        ("legacy", "UPDATE tasks SET admission=NULL WHERE id=$1"),
    ] {
        let task = submit(&f, workflow, &f.token, key).await;
        assert_eq!(db.execute(change, &[&task]).await.unwrap(), 1, "{key}");
        let before = task_and_step(&db, task).await;
        assert_eq!(next(&f).await, None, "{key}");
        assert_eq!(task_and_step(&db, task).await, before, "{key}");
        assert_unclaimed(&step(&db, task, "echo").await);
    }
}

#[tokio::test]
async fn expired_or_revoked_execution_authority_pauses_first_claim() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &f.spec("authority")).await;
    let db = f.db.connect().await.unwrap();
    // Only the credential cases consume their issuing session.
    let expiring = login(&f.router, f.owner).await;
    let revocable = login(&f.router, f.owner).await;
    for (key, token, change) in [
        (
            "delegation-expired",
            &f.token,
            "UPDATE execution_delegations SET expires_at=now()-interval '1 second'
         WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        ),
        (
            "delegation-revoked",
            &f.token,
            "UPDATE execution_delegations SET status='revoked'
         WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        ),
        (
            "credential-expired",
            &expiring,
            "UPDATE credentials SET expires_at=now() WHERE id=(
            SELECT d.issuer_credential_id FROM tasks t
            JOIN execution_delegations d ON d.id=t.delegation_id WHERE t.id=$1)",
        ),
        (
            "credential-revoked",
            &revocable,
            "UPDATE credentials SET status='revoked' WHERE id=(
            SELECT d.issuer_credential_id FROM tasks t
            JOIN execution_delegations d ON d.id=t.delegation_id WHERE t.id=$1)",
        ),
        (
            "grant-expired",
            &f.token,
            "UPDATE grants SET expires_at=now() WHERE scope->>'kind'='capability'
         AND subject_id=(SELECT owner_id FROM tasks WHERE id=$1)",
        ),
    ] {
        let task = submit(&f, workflow, token, key).await;
        assert_eq!(db.execute(change, &[&task]).await.unwrap(), 1, "{change}");
        assert_paused(&f, &db, task).await;
    }
}

#[tokio::test]
async fn new_session_cannot_replace_the_original_issuer() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let spec = f.spec("issuer");
    let workflow = publish(&f, &spec).await;
    let task = submit(&f, workflow, &f.token, "old-session").await;
    let replacement = request(
        &f.router,
        Method::POST,
        "/api/platform/v1/auth/login",
        None,
        Some(&json!({"account_id": f.owner, "password": PASSWORD})),
        &[],
    )
    .await
    .expect(200)
    .body;
    let new_id = contracts::id(&replacement["session"], "id").unwrap();
    let db = f.db.connect().await.unwrap();
    let old_id: Uuid = db
        .query_one(
            "SELECT d.issuer_credential_id FROM execution_delegations d
             JOIN tasks t ON t.delegation_id=d.id WHERE t.id=$1",
            &[&task],
        )
        .await
        .unwrap()
        .get(0);
    assert_ne!(old_id, new_id);
    // Both sessions are live. Only their identities distinguish them.
    db.execute(
        "UPDATE execution_delegations SET issuer_credential_id=$2
         WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        &[&task, &new_id],
    )
    .await
    .unwrap();
    assert_paused(&f, &db, task).await;
    let fresh = submit(
        &f,
        workflow,
        replacement["access_token"].as_str().unwrap(),
        "new-session",
    )
    .await;
    assert_eq!(
        next(&f).await.unwrap()["step_id"],
        json!(step(&db, fresh, "echo").await.get::<_, Uuid>("id"))
    );
}

async fn issue_grant(f: &Fixture, subject: Uuid, role: &str, scope: &Value, key: &str) -> Reply {
    request(
        &f.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{subject}/grants"),
        Some(&f.token),
        Some(&json!({"source": "role", "role_id": role, "scope": scope})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201)
}

#[tokio::test]
async fn equivalent_grant_cannot_replace_a_frozen_source() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let spec = f.spec("grant");
    let workflow = publish(&f, &spec).await;
    let db = f.db.connect().await.unwrap();
    // A standalone submitter has no inherited path to the Owner's capability.
    let subject = Uuid::new_v4();
    db.execute(
        "INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at)
         SELECT $1,'user','active','Submitter',password_hash,now(),now() FROM accounts WHERE id=$2",
        &[&subject, &f.owner],
    )
    .await
    .unwrap();
    db.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&subject])
        .await
        .unwrap();
    db.execute(
        "INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at)
         SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2",
        &[&subject, &f.owner],
    )
    .await
    .unwrap();
    issue_grant(
        &f,
        subject,
        "Owner",
        &json!({"kind":"workflow","match":"ids","ids":[workflow]}),
        "workflow-access",
    )
    .await;
    issue_grant(
        &f,
        subject,
        "Member",
        &json!({"kind":"task","match":"owned_by","owner_id":subject}),
        "task-access",
    )
    .await;
    let capability = json!({"kind":"capability","match":"all"});
    let original = issue_grant(&f, subject, "Guest", &capability, "original-capability").await;
    let token = login(&f.router, subject).await;
    let task = submit(&f, workflow, &token, "old-grant").await;

    let id = contracts::id(&original.body, "id").unwrap();
    f.archive(
        &format!("/api/platform/v1/grants/{id}"),
        "revoke-capability",
        original.etag.as_deref(),
    )
    .await
    .expect(204);
    let replacement =
        issue_grant(&f, subject, "Guest", &capability, "replacement-capability").await;
    assert_ne!(replacement.body["id"], original.body["id"]);
    assert_paused(&f, &db, task).await;
    let fresh = submit(&f, workflow, &token, "new-grant").await;
    assert_eq!(
        next(&f).await.unwrap()["step_id"],
        json!(step(&db, fresh, "echo").await.get::<_, Uuid>("id"))
    );
}

async fn task_and_step(db: &Client, task: Uuid) -> Value {
    db.query_one(
        "SELECT jsonb_build_array(to_jsonb(t),to_jsonb(s)) FROM tasks t
         JOIN task_steps s ON s.task_id=t.id WHERE t.id=$1",
        &[&task],
    )
    .await
    .unwrap()
    .get(0)
}

#[tokio::test]
async fn failed_claim_commit_rolls_back_and_can_be_retried() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = publish(&f, &f.spec("rollback")).await;
    let task = submit(&f, workflow, &f.token, "rollback").await;
    let db = f.db.connect().await.unwrap();
    let before = task_and_step(&db, task).await;
    // A deferred constraint trigger fails COMMIT after all claim writes.
    db.batch_execute(
        "CREATE FUNCTION fail_claim_commit() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'injected claim commit failure'; END; $$;
         CREATE CONSTRAINT TRIGGER fail_claim AFTER INSERT ON attempts
         DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION fail_claim_commit()",
    )
    .await
    .unwrap();
    claim(&f.db, &f.auth, Uuid::new_v4())
        .await
        .expect_err("commit must fail");
    assert_eq!(task_and_step(&db, task).await, before);
    let row = step(&db, task, "echo").await;
    assert_unclaimed(&row);

    db.batch_execute("DROP TRIGGER fail_claim ON attempts")
        .await
        .unwrap();
    let retry = next(&f).await.unwrap();
    assert_eq!(retry["step_id"], json!(row.get::<_, Uuid>("id")));
    assert_eq!(retry["number"], 1);
    assert_eq!(retry["fence"], 1);
    assert_eq!(step(&db, task, "echo").await.get::<_, i64>("attempts"), 1);
}

#[tokio::test]
async fn first_claim_does_not_replay_unknown_or_expired_attempts() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut spec = f.spec("attempt history");
    spec["steps"][0]["maximum_attempts"] = json!(2);
    let workflow = publish(&f, &spec).await;
    let db = f.db.connect().await.unwrap();
    for (state, step_state) in [
        ("unknown", "unknown"),
        ("expired", "ready"),
        ("leased", "running"),
    ] {
        let task = submit(&f, workflow, &f.token, state).await;
        let first = next(&f).await.unwrap();
        let id = contracts::id(&first, "id").unwrap();
        let root = step(&db, task, "echo").await.get::<_, Uuid>("id");
        assert_eq!(first["step_id"], json!(root));
        db.execute(
            "UPDATE task_steps SET state=$2 WHERE id=$1",
            &[&root, &step_state],
        )
        .await
        .unwrap();
        let expiry: DateTime<Utc> = db
            .query_one(
                "UPDATE attempts SET state=$2, lease_expires_at=now()
             WHERE id=$1 RETURNING lease_expires_at",
                &[&id, &state],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(next(&f).await, None, "prior attempt state {state}");
        let row = step(&db, task, "echo").await;
        assert_eq!(row.get::<_, i32>("attempt_count"), 1);
        assert_eq!(row.get::<_, i64>("fence"), 1);

        // Even if step metadata gets reset, attempt history precludes a first claim.
        db.execute("UPDATE tasks SET state='running' WHERE id=$1", &[&task])
            .await
            .unwrap();
        db.execute(
            "UPDATE task_steps SET state='ready',attempt_count=0,fence=0 WHERE id=$1",
            &[&root],
        )
        .await
        .unwrap();
        assert_eq!(
            next(&f).await,
            None,
            "reset step with prior {state} attempt"
        );
        assert_eq!(step(&db, task, "echo").await.get::<_, i64>("attempts"), 1);
        let stored = db
            .query_one(
                "SELECT execution_id,number,fence,lease_expires_at,state FROM attempts WHERE id=$1",
                &[&id],
            )
            .await
            .unwrap();
        assert_eq!(
            json!(stored.get::<_, Uuid>("execution_id")),
            first["execution_id"]
        );
        assert_eq!(stored.get::<_, i32>("number"), 1);
        assert_eq!(stored.get::<_, i64>("fence"), 1);
        assert_eq!(stored.get::<_, DateTime<Utc>>("lease_expires_at"), expiry);
        if state == "unknown" {
            assert_eq!(stored.get::<_, String>("state"), "unknown");
        }
    }
}
