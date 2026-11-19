use super::{request, schedule_catalog::scoped_key, Fixture, Reply, WORKFLOWS};
use axum::http::Method;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;
use xelab_kernel::{contracts, schedule_cron::CronSchedule};

const SCHEDULES: &str = "/api/platform/v1/schedules";

async fn create(f: &Fixture, token: &str, spec: &Value, key: &str) -> Reply {
    request(
        &f.router,
        Method::POST,
        SCHEDULES,
        Some(token),
        Some(spec),
        &[("idempotency-key", key)],
    )
    .await
}

async fn workflow(f: &Fixture) -> Uuid {
    let published = f
        .publish(&f.token, &f.spec("recorded"), "publish-scheduled-workflow")
        .await
        .expect(201);
    contracts::id(&published.body, "id").unwrap()
}

fn spec(workflow: Uuid) -> Value {
    json!({"name":"Pinned workflow schedule","workflow_id":workflow,
           "cron":"30 7 * * *","input":{"configured":true}})
}

fn rules(f: &Fixture, workflow: Uuid) -> Value {
    json!([
        {"action":"schedule.create","resource":{"kind":"schedule","match":"owned_by","owner_id":f.owner}},
        {"action":"task.create","resource":{"kind":"task","match":"owned_by","owner_id":f.owner}},
        {"action":"workflow.read","resource":{"kind":"workflow","match":"ids","ids":[workflow]}},
        {"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":[format!(
            "{}@{}",f.contract["name"].as_str().unwrap(),f.contract["version"].as_str().unwrap())]}}
    ])
}

#[tokio::test]
async fn persists_enabled_schedule_and_exact_original_authority_with_idempotency() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = workflow(&f).await;
    let mut rules = rules(&f, workflow);
    let db = f.db.connect().await.unwrap();
    let expires: DateTime<Utc> = db
        .query_one("SELECT clock_timestamp()+interval '10 minutes'", &[])
        .await
        .unwrap()
        .get(0);
    rules[3]["conditions"] = json!({"expires_at":expires});
    let token = scoped_key(&f, "schedule-creator", rules).await;
    let spec = spec(workflow);
    let created = create(&f, &token, &spec, "create-scheduled-work")
        .await
        .expect(201);
    assert_eq!(created.etag.as_deref(), Some("\"rev-1\""));
    assert_eq!(created.body["owner_id"], json!(f.owner));
    contracts::validate("Schedule", &created.body).unwrap();
    let schedule = contracts::id(&created.body, "id").unwrap();
    assert_eq!(created.body["spec"]["input"], spec["input"]);
    assert_eq!(created.body["spec"]["enabled"], true);
    assert_eq!(created.body["spec"]["timezone"], "Asia/Shanghai");
    assert_eq!(created.body["spec"]["misfire"], "skip");
    assert_eq!(created.body["spec"]["overlap"], "skip");
    let recorded_at: DateTime<Utc> =
        serde_json::from_value(created.body["created_at"].clone()).unwrap();
    let next = CronSchedule::parse("30 7 * * *", "Asia/Shanghai")
        .unwrap()
        .preview(recorded_at, 1)
        .unwrap()
        .fire_at[0];
    assert_eq!(created.body["next_fire_at"], json!(next));

    let persisted = db
        .query_one(
            "SELECT s.admission,d.maximum_rules,d.issuer_credential_id,d.expires_at,d.status
           FROM schedules s JOIN execution_delegations d ON d.id=s.delegation_id
          WHERE s.id=$1",
            &[&schedule],
        )
        .await
        .unwrap();
    let proof: Value = persisted.get(0);
    let maximum: Value = persisted.get(1);
    let issuer: Uuid = persisted.get(2);
    assert_eq!(persisted.get::<_, DateTime<Utc>>(3), expires);
    assert_eq!(persisted.get::<_, String>(4), "active");
    assert_eq!(proof["workflow_id"], json!(workflow));
    assert_eq!(
        proof["task_admission"]["credential_chain"][0],
        json!(issuer)
    );
    assert!(!proof.to_string().contains(&token));
    let task_path = &proof["task_create_paths"][0];
    let workflow_path = &proof["task_admission"]["workflow_paths"][0];
    let target_path = &proof["task_admission"]["steps"][0]["targets"][0]["paths"][0];
    for path in [task_path, workflow_path, target_path] {
        let sources = path["sources"].as_array().unwrap();
        assert!(!sources.is_empty());
        for source in sources {
            let id = source.as_str().unwrap().parse::<Uuid>().unwrap();
            assert!(db
                .query_opt("SELECT id FROM grants WHERE id=$1", &[&id])
                .await
                .unwrap()
                .is_some());
        }
    }
    assert_eq!(
        task_path["rule"]["resource"],
        json!({"kind":"task","match":"ids","ids":["new"]})
    );
    assert_eq!(
        workflow_path["rule"]["resource"],
        json!({"kind":"workflow","match":"ids","ids":[workflow]})
    );
    assert!(maximum
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["action"] != "task.create"));
    assert_eq!(target_path["rule"]["action"], "capability.invoke");

    let published = f
        .publish(&f.token, &f.spec("newer version"), "new-workflow-version")
        .await
        .expect(201);
    assert_ne!(published.body["id"], json!(workflow));
    let loaded = f
        .read(&f.token, &format!("{SCHEDULES}/{schedule}"))
        .await
        .expect(200);
    assert_eq!(loaded.body, created.body);
    assert_eq!(
        create(&f, &token, &spec, "create-scheduled-work")
            .await
            .expect(201)
            .body,
        created.body
    );
    let mut changed = spec;
    changed["input"] = json!({});
    create(&f, &token, &changed, "create-scheduled-work")
        .await
        .expect(409);
    let counts = db.query_one("SELECT (SELECT count(*) FROM schedules),
        (SELECT count(*) FROM execution_delegations),(SELECT count(*) FROM schedule_fires),
        (SELECT count(*) FROM tasks),
        (SELECT count(*) FROM audit_events WHERE action='createSchedule' AND reason_code='COMMITTED'
         AND resource=jsonb_build_object('kind','schedule','match','ids','ids',jsonb_build_array($1::uuid)))",
        &[&schedule]).await.unwrap();
    assert_eq!(
        (0..5).map(|i| counts.get::<_, i64>(i)).collect::<Vec<_>>(),
        [1, 1, 0, 0, 1]
    );
}

#[tokio::test]
async fn checks_schedule_and_task_permissions_and_validates_fixed_workflow_and_input() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = workflow(&f).await;
    let body = spec(workflow);
    for (index, status) in [(0, 403), (1, 403), (2, 404), (3, 403)] {
        let mut selected = rules(&f, workflow);
        selected.as_array_mut().unwrap().remove(index);
        let token = scoped_key(&f, &format!("missing-schedule-rule-{index}"), selected).await;
        create(&f, &token, &body, &format!("cannot-create-{index}"))
            .await
            .expect(status);
    }
    for (field, value) in [
        ("timezone", json!("Unknown/Zone")),
        ("cron", json!("* * * * * *")),
        ("input", json!("does not satisfy workflow input schema")),
        ("name", json!("embedded\u{0000}nul")),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        create(&f, &f.token, &invalid, &format!("invalid-{field}"))
            .await
            .expect(400);
    }
    let mut impossible = body.clone();
    impossible["cron"] = json!("0 0 30 2 *");
    tokio::time::timeout(
        Duration::from_secs(2),
        create(&f, &f.token, &impossible, "impossible-cron"),
    )
    .await
    .expect("checking the search horizon must be bounded")
    .expect(400);
    request(
        &f.router,
        Method::POST,
        &format!("{SCHEDULES}?force=true"),
        Some(&f.token),
        Some(&body),
        &[("idempotency-key", "undeclared-query")],
    )
    .await
    .expect(400);
    let mut disabled = body.clone();
    disabled["enabled"] = json!(false);
    let value = create(&f, &f.token, &disabled, "disabled")
        .await
        .expect(201)
        .body;
    assert_eq!(value["next_fire_at"], Value::Null);
    assert_eq!(value["spec"]["enabled"], false);
    f.archive(
        &format!("{WORKFLOWS}/{workflow}"),
        "archive-schedule-workflow",
        Some("\"rev-1\""),
    )
    .await
    .expect(204);
    create(&f, &f.token, &body, "archived").await.expect(409);
}

#[tokio::test]
async fn late_audit_wait_expires_frozen_dispatch_authority_and_rolls_back_both_rows() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let workflow = workflow(&f).await;
    let body = spec(workflow);
    let db = f.db.connect().await.unwrap();
    let until: DateTime<Utc> = db
        .query_one("SELECT clock_timestamp()+interval '5 seconds'", &[])
        .await
        .unwrap()
        .get(0);
    let mut selected = rules(&f, workflow);
    selected[3]["conditions"] = json!({"expires_at":until});
    let token = scoped_key(&f, "temporary-schedule", selected).await;
    let mut locker = f.db.connect().await.unwrap();
    let hold = locker.transaction().await.unwrap();
    hold.batch_execute("LOCK TABLE audit_events IN SHARE MODE")
        .await
        .unwrap();
    let handle = tokio::spawn(async move {
        let reply = create(&f, &token, &body, "stale-frozen-authority").await;
        (f, body, reply)
    });
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let waiting: bool = db.query_one("SELECT EXISTS(SELECT FROM pg_locks
                WHERE relation='audit_events'::regclass AND mode='RowExclusiveLock' AND NOT granted)",
                &[]).await.unwrap().get(0);
            if waiting { break; }
            assert!(!handle.is_finished());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("create reaches its success audit with live authority");
    db.query_one(
        "SELECT pg_sleep(greatest(extract(epoch FROM $1::timestamptz-clock_timestamp()),0)+0.05)",
        &[&until],
    )
    .await
    .unwrap();
    hold.rollback().await.unwrap();
    let (f, body, reply) = handle.await.unwrap();
    reply.expect(409);
    let result = db
        .query_one(
            "SELECT (SELECT count(*) FROM schedules),
        (SELECT count(*) FROM execution_delegations),
        (SELECT count(*) FROM idempotency_records WHERE key='stale-frozen-authority'),
        (SELECT count(*) FROM audit_events WHERE action='createSchedule' AND outcome='failed')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        (0..4).map(|i| result.get::<_, i64>(i)).collect::<Vec<_>>(),
        [0, 0, 0, 1]
    );
    create(&f, &f.token, &body, "create-with-live-authority")
        .await
        .expect(201);
}
