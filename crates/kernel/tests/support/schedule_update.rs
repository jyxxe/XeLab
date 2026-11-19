use super::{request, schedule_catalog::scoped_key, Fixture, Reply};
use axum::http::Method;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{contracts, schedule_cron::CronSchedule};

const SCHEDULES: &str = "/api/platform/v1/schedules";

async fn publish(f: &Fixture, title: &str, key: &str) -> Uuid {
    let reply = f.publish(&f.token, &f.spec(title), key).await.expect(201);
    contracts::id(&reply.body, "id").unwrap()
}

fn spec(workflow: Uuid) -> Value {
    json!({"name":"Original plan","workflow_id":workflow,"cron":"15 6 * * *","input":{"old":true}})
}

async fn create(f: &Fixture, body: &Value) -> Value {
    request(
        &f.router,
        Method::POST,
        SCHEDULES,
        Some(&f.token),
        Some(body),
        &[("idempotency-key", "create-schedule-to-update")],
    )
    .await
    .expect(201)
    .body
}

async fn update(
    f: &Fixture,
    token: &str,
    schedule: Uuid,
    spec: &Value,
    tag: Option<&str>,
    key: &str,
) -> Reply {
    let mut headers = vec![("idempotency-key", key)];
    if let Some(tag) = tag {
        headers.push(("if-match", tag));
    }
    request(
        &f.router,
        Method::PUT,
        &format!("{SCHEDULES}/{schedule}"),
        Some(token),
        Some(spec),
        &headers,
    )
    .await
}

async fn data(db: &Client, schedule: Uuid) -> (Value, Value) {
    let row = db
        .query_one(
            "SELECT s.admission,to_jsonb(e) FROM schedules s JOIN execution_delegations e
           ON e.id=s.delegation_id WHERE s.id=$1",
            &[&schedule],
        )
        .await
        .unwrap();
    (row.get(0), row.get(1))
}

async fn pending(db: &Client, schedule: Uuid) {
    db.execute(
        "INSERT INTO schedule_fires(schedule_id,scheduled_at,outcome)
        VALUES($1,'2024-01-01T00:00:00Z','pending'),($1,'2024-01-01T00:01:00Z','paused')",
        &[&schedule],
    )
    .await
    .unwrap();
}

async fn fire_states(db: &Client, schedule: Uuid) -> Vec<String> {
    db.query(
        "SELECT outcome FROM schedule_fires WHERE schedule_id=$1 ORDER BY scheduled_at",
        &[&schedule],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| row.get(0))
    .collect()
}

#[tokio::test]
async fn rename_and_idempotent_update_keep_origin_proof_cursor_and_pending_fires() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, "workflow for rename", "published-for-rename").await;
    let initial = create(&f, &spec(workflow)).await;
    let schedule = contracts::id(&initial, "id").unwrap();
    pending(&db, schedule).await;
    let old = data(&db, schedule).await;
    // Authority to edit the Schedule does not imply Workflow or Task authority.
    let editor = scoped_key(
        &f,
        "rename-schedule",
        json!([{
            "action":"schedule.update","resource":{"kind":"schedule","match":"ids","ids":[schedule]}
        }]),
    )
    .await;
    let reader = scoped_key(
        &f,
        "read-schedule-only",
        json!([{
            "action":"schedule.read","resource":{"kind":"schedule","match":"ids","ids":[schedule]}
        }]),
    )
    .await;
    let mut renamed = spec(workflow);
    renamed["name"] = json!("Renamed plan");
    let first = Some("\"rev-1\"");
    update(&f, &editor, schedule, &renamed, None, "no-precondition")
        .await
        .expect(428);
    update(&f, &reader, schedule, &renamed, first, "read-only")
        .await
        .expect(403);
    update(&f, &editor, Uuid::new_v4(), &renamed, first, "hidden")
        .await
        .expect(404);
    let accepted = update(&f, &editor, schedule, &renamed, first, "rename")
        .await
        .expect(200);
    assert_eq!(accepted.etag.as_deref(), Some("\"rev-2\""));
    assert_eq!(accepted.body["spec"]["name"], "Renamed plan");
    assert_eq!(accepted.body["next_fire_at"], initial["next_fire_at"]);
    assert_eq!(accepted.body["delegation_id"], initial["delegation_id"]);
    assert_eq!(data(&db, schedule).await, old);
    assert_eq!(fire_states(&db, schedule).await, ["pending", "paused"]);

    assert_eq!(
        update(&f, &editor, schedule, &renamed, first, "rename")
            .await
            .expect(200)
            .body,
        accepted.body
    );
    update(&f, &editor, schedule, &renamed, first, "stale-edit")
        .await
        .expect(412);
    let current = Some("\"rev-2\"");
    assert_eq!(
        update(&f, &editor, schedule, &renamed, current, "noop")
            .await
            .expect(200)
            .body,
        accepted.body
    );
    renamed["name"] = json!("A different edit");
    update(&f, &editor, schedule, &renamed, first, "rename")
        .await
        .expect(409);
    renamed["enabled"] = json!(false);
    let disabled = update(&f, &editor, schedule, &renamed, current, "disable-update")
        .await
        .expect(200);
    assert_eq!(disabled.body["next_fire_at"], Value::Null);
    assert_eq!(data(&db, schedule).await, old);
    assert_eq!(fire_states(&db, schedule).await, ["skipped", "skipped"]);
}

#[tokio::test]
async fn input_and_cadence_require_current_editor_scope_and_keep_pinned_authority_and_child_task() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let workflow = publish(&f, "workflow for input", "published-for-input").await;
    let initial = create(&f, &spec(workflow)).await;
    let schedule = contracts::id(&initial, "id").unwrap();
    let parent = contracts::id(&initial, "delegation_id").unwrap();
    let child = Uuid::new_v4();
    let task = Uuid::new_v4();
    db.execute(
        "INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,
           run_as,expires_at,status,created_at)
         SELECT $2,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,now()
           FROM execution_delegations WHERE id=$1",
        &[&parent, &child],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO tasks(id,owner_id,workflow_id,delegation_id,input,state,deadline_at,created_at,updated_at)
         VALUES($1,$2,$3,$4,'{\"old\":true}','running',now()+interval '1 hour',now(),now())",
        &[&task,&f.owner,&workflow,&child],
    ).await.unwrap();
    db.execute(
        "INSERT INTO schedule_fires(schedule_id,scheduled_at,task_id,outcome)
        VALUES($1,'2023-12-31T23:59:00Z',$2,'created')",
        &[&schedule, &task],
    )
    .await
    .unwrap();
    pending(&db, schedule).await;
    let snapshot = "SELECT jsonb_build_array(to_jsonb(t),to_jsonb(e)) FROM tasks t
                     JOIN execution_delegations e ON t.delegation_id=e.id WHERE t.id=$1";
    let original_task: Value = db.query_one(snapshot, &[&task]).await.unwrap().get(0);
    let original = data(&db, schedule).await;
    let restricted = scoped_key(&f,"schedule-editor-without-capability",json!([
        {"action":"schedule.update","resource":{"kind":"schedule","match":"ids","ids":[schedule]}},
        {"action":"task.create","resource":{"kind":"task","match":"owned_by","owner_id":f.owner}},
        {"action":"workflow.read","resource":{"kind":"workflow","match":"ids","ids":[workflow]}}
    ])).await;
    let mut edited = spec(workflow);
    edited["input"] = json!({"new":"arguments"});
    edited["cron"] = json!("43 23 * * *");
    edited["timezone"] = json!("UTC");
    let first = Some("\"rev-1\"");
    update(
        &f,
        &restricted,
        schedule,
        &edited,
        first,
        "insufficient-for-execution",
    )
    .await
    .expect(403);
    assert_eq!(
        fire_states(&db, schedule).await,
        ["created", "pending", "paused"]
    );
    let updated = update(
        &f,
        &f.token,
        schedule,
        &edited,
        first,
        "edit-input-and-time",
    )
    .await
    .expect(200);
    assert_eq!(updated.body["spec"]["input"], edited["input"]);
    let updated_at: DateTime<Utc> =
        serde_json::from_value(updated.body["updated_at"].clone()).unwrap();
    let next: DateTime<Utc> = serde_json::from_value(updated.body["next_fire_at"].clone()).unwrap();
    assert_eq!(
        next,
        CronSchedule::parse("43 23 * * *", "UTC")
            .unwrap()
            .preview(updated_at, 1)
            .unwrap()
            .fire_at[0]
    );
    assert_eq!(data(&db, schedule).await, original);
    assert_eq!(
        fire_states(&db, schedule).await,
        ["created", "skipped", "skipped"]
    );
    assert_eq!(
        db.query_one(snapshot, &[&task])
            .await
            .unwrap()
            .get::<_, Value>(0),
        original_task
    );
}

#[tokio::test]
async fn explicitly_changing_workflow_replaces_proof_without_reviving_the_old_one() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let old = publish(&f, "old workflow", "publish-old-workflow").await;
    let new = publish(&f, "new workflow", "publish-new-workflow").await;
    let mut body = spec(old);
    body["enabled"] = json!(false);
    let record = create(&f, &body).await;
    let schedule = contracts::id(&record, "id").unwrap();
    let original = contracts::id(&record, "delegation_id").unwrap();
    let (original_proof, _) = data(&db, schedule).await;
    db.execute(
        "UPDATE schedules SET admission=NULL WHERE id=$1",
        &[&schedule],
    )
    .await
    .unwrap();
    body["enabled"] = json!(true);
    let first = Some("\"rev-1\"");
    update(&f, &f.token, schedule, &body, first, "enable-unproven-plan")
        .await
        .expect(403);
    db.execute(
        "UPDATE schedules SET admission=$2 WHERE id=$1",
        &[&schedule, &original_proof],
    )
    .await
    .unwrap();
    db.execute(
        "UPDATE execution_delegations SET status='revoked' WHERE id=$1",
        &[&original],
    )
    .await
    .unwrap();
    let original_row = data(&db, schedule).await;
    update(
        &f,
        &f.token,
        schedule,
        &body,
        first,
        "enable-with-revoked-proof",
    )
    .await
    .expect(403);
    assert_eq!(data(&db, schedule).await, original_row);
    body["workflow_id"] = json!(new);
    let updated = update(&f, &f.token, schedule, &body, first, "replace-workflow")
        .await
        .expect(200);
    let (proof, delegation) = data(&db, schedule).await;
    assert_eq!(proof["workflow_id"], json!(new));
    assert_eq!(delegation["id"], updated.body["delegation_id"]);
    assert_ne!(delegation["id"], json!(original));
    assert_eq!(delegation["status"], "active");
    assert!(updated.body["next_fire_at"].is_string());
    assert_eq!(
        db.query_one(
            "SELECT to_jsonb(d) FROM execution_delegations d WHERE id=$1",
            &[&original]
        )
        .await
        .unwrap()
        .get::<_, Value>(0),
        original_row.1
    );
}

#[tokio::test]
async fn update_rechecks_etag_after_waiting_for_the_trigger_lock() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.db.connect().await.unwrap();
    let mut other = f.db.connect().await.unwrap();
    let workflow = publish(&f, "workflow for concurrent update", "publish-concurrent").await;
    let initial = create(&f, &spec(workflow)).await;
    let schedule = contracts::id(&initial, "id").unwrap();
    let pid: i32 = other
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let hold = other.transaction().await.unwrap();
    hold.query_one(
        "SELECT id FROM schedules WHERE id=$1 FOR UPDATE",
        &[&schedule],
    )
    .await
    .unwrap();
    let request = tokio::spawn(async move {
        let mut wanted = spec(workflow);
        wanted["name"] = json!("stale name");
        let reply = update(
            &f,
            &f.token,
            schedule,
            &wanted,
            Some("\"rev-1\""),
            "stale-after-trigger",
        )
        .await;
        (f, reply)
    });
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let blocked: bool = c
                .query_one(
                    "SELECT EXISTS(SELECT FROM pg_stat_activity a
                WHERE a.datname=current_database() AND $1=ANY(pg_blocking_pids(a.pid)))",
                    &[&pid],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            assert!(!request.is_finished());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the HTTP edit must wait on the trigger's Schedule row");
    hold.execute(
        "UPDATE schedules SET revision=revision+1 WHERE id=$1",
        &[&schedule],
    )
    .await
    .unwrap();
    hold.commit().await.unwrap();
    let (f, response) = request.await.unwrap();
    response.expect(412);
    let persisted = f
        .read(&f.token, &format!("{SCHEDULES}/{schedule}"))
        .await
        .expect(200);
    assert_eq!(persisted.body["spec"]["name"], "Original plan");
    assert_eq!(persisted.body["revision"], 2);
}

#[tokio::test]
async fn timed_origin_expiring_at_audit_wait_rolls_back_edit_and_unsubmitted_fires() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.db.connect().await.unwrap();
    let mut other = f.db.connect().await.unwrap();
    let workflow = publish(&f, "workflow for expiration", "publish-expiration").await;
    let initial = create(&f, &spec(workflow)).await;
    let schedule = contracts::id(&initial, "id").unwrap();
    let delegation = contracts::id(&initial, "delegation_id").unwrap();
    pending(&c, schedule).await;
    let until: DateTime<Utc> = c
        .query_one(
            "UPDATE execution_delegations SET expires_at=clock_timestamp()+interval '6 seconds'
          WHERE id=$1 RETURNING expires_at",
            &[&delegation],
        )
        .await
        .unwrap()
        .get(0);
    let old = data(&c, schedule).await;
    let hold = other.transaction().await.unwrap();
    hold.batch_execute("LOCK TABLE audit_events IN SHARE MODE")
        .await
        .unwrap();
    let request = tokio::spawn(async move {
        let mut wanted = spec(workflow);
        wanted["cron"] = json!("16 7 * * *");
        let reply = update(
            &f,
            &f.token,
            schedule,
            &wanted,
            Some("\"rev-1\""),
            "wait-on-update-audit",
        )
        .await;
        (f, reply)
    });
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let blocked: bool = c
                .query_one(
                    "SELECT EXISTS(SELECT FROM pg_locks WHERE relation='audit_events'::regclass
                   AND database=(SELECT oid FROM pg_database WHERE datname=current_database())
                   AND mode='RowExclusiveLock' AND NOT granted)",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            assert!(!request.is_finished());
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the edit must reach its pending success audit write");
    c.query_one(
        "SELECT pg_sleep(greatest(extract(epoch FROM $1::timestamptz-clock_timestamp()),0)+0.05)",
        &[&until],
    )
    .await
    .unwrap();
    hold.rollback().await.unwrap();
    let (f, response) = request.await.unwrap();
    response.expect(409);
    assert_eq!(
        f.read(&f.token, &format!("{SCHEDULES}/{schedule}"))
            .await
            .expect(200)
            .body,
        initial
    );
    assert_eq!(fire_states(&c, schedule).await, ["pending", "paused"]);
    assert_eq!(data(&c, schedule).await, old);
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM idempotency_records WHERE key='wait-on-update-audit'",
            &[],
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
}
