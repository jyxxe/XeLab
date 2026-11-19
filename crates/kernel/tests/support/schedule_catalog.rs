//! Schedule API coverage using persisted, pre-existing metadata.
use super::{request, Fixture, Reply};
use axum::http::Method;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::contracts;

const SCHEDULES: &str = "/api/platform/v1/schedules";

async fn insert_schedule(
    db: &Client,
    id: Uuid,
    owner: Uuid,
    created: DateTime<Utc>,
    next_fire_at: Option<DateTime<Utc>>,
) -> Value {
    let postgres_time =
        |time: DateTime<Utc>| DateTime::from_timestamp_micros(time.timestamp_micros()).unwrap();
    let created = postgres_time(created);
    let next_fire_at = next_fire_at.map(postgres_time);
    let workflow = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    // Offline restore leaves the schedule data in place and revokes its execution delegation.
    db.execute(
        "INSERT INTO execution_delegations(id,subject_id,maximum_rules,run_as,expires_at,status,created_at)
         VALUES($1,$2,'[]','user',now()+interval '1 day','revoked',$3)",
        &[&delegation, &owner, &created],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO workflows(id,owner_id,name,version,spec,status,created_at,updated_at)
         VALUES($1,$2,$3,1,'{}','archived',$4,$4)",
        &[&workflow, &owner, &workflow.to_string(), &created],
    )
    .await
    .unwrap();
    // Historical records may omit optional settings and carry arbitrary JSON input.
    let spec = json!({
        "name":"Recovered schedule","workflow_id":workflow,"cron":"30 7 * * *",
        "input":{"message":"schedule input"}
    });
    let updated = created + Duration::minutes(5);
    db.execute(
        "INSERT INTO schedules(id,owner_id,spec,delegation_id,next_fire_at,revision,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,8,$6,$7)",
        &[&id, &owner, &spec, &delegation, &next_fire_at, &created, &updated],
    )
    .await
    .unwrap();
    json!({"id":id,"revision":8,"created_at":created,"updated_at":updated,
        "owner_id":owner,"spec":spec,"delegation_id":delegation,"next_fire_at":next_fire_at})
}

async fn scoped_key(fixture: &Fixture, name: &str, rules: Value) -> String {
    let reply = request(
        &fixture.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", fixture.owner),
        Some(&fixture.token),
        Some(&json!({
            "name":name,
            "inheritance":{"mode":"selected","rules":rules},
            "conditions":{"expires_at":Utc::now()+Duration::days(1)}
        })),
        &[("idempotency-key", name)],
    )
    .await
    .expect(201);
    reply.body["secret"].as_str().unwrap().to_owned()
}

async fn page(fixture: &Fixture, token: &str, uri: &str) -> Value {
    let body = fixture.read(token, uri).await.expect(200).body;
    contracts::validate("SchedulePage", &body).unwrap();
    body
}

#[tokio::test]
async fn reads_persisted_schedule_fields_as_utc_in_a_non_utc_database() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let db = fixture.db.connect().await.unwrap();
    let set_timezone: String = db
        .query_one(
            "SELECT format('ALTER DATABASE %I SET TimeZone TO %L', current_database(), 'America/Los_Angeles')",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    db.batch_execute(&set_timezone).await.unwrap();
    let later_connection = fixture.db.connect().await.unwrap();
    assert_eq!(
        later_connection
            .query_one("SHOW TimeZone", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "America/Los_Angeles"
    );
    let first = Uuid::from_u128(1);
    let second = Uuid::from_u128(2);
    let created = DateTime::from_timestamp(-1, 123_456_000).unwrap();
    let expected = [
        insert_schedule(
            &db,
            first,
            fixture.owner,
            created,
            Some(created + Duration::days(1)),
        )
        .await,
        insert_schedule(&db, second, fixture.owner, created, None).await,
    ];
    for row in &expected {
        let reply = fixture
            .read(
                &fixture.token,
                &format!("{SCHEDULES}/{}", row["id"].as_str().unwrap()),
            )
            .await
            .expect(200);
        assert_eq!(reply.etag.as_deref(), Some("\"rev-8\""));
        assert_eq!(reply.body, *row);
    }
    let first_page = page(&fixture, &fixture.token, &format!("{SCHEDULES}?limit=1")).await;
    assert_eq!(first_page["items"], json!([expected[0]]));
    let cursor = first_page["next_cursor"].as_str().unwrap();
    let last = page(
        &fixture,
        &fixture.token,
        &format!("{SCHEDULES}?limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(last, json!({"items":[expected[1]],"next_cursor":null}));
    assert_eq!(
        db.query_one("SELECT count(*) FROM schedule_fires", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn filters_scope_and_binds_schedule_cursor_to_credential_and_limit() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let (child, _) = fixture.child().await;
    let db = fixture.db.connect().await.unwrap();
    let created = Utc::now() - Duration::minutes(1);
    let first = Uuid::from_u128(1);
    let hidden = Uuid::from_u128(2);
    let last = Uuid::from_u128(3);
    insert_schedule(&db, last, fixture.owner, created, None).await;
    insert_schedule(&db, hidden, child, created, None).await;
    insert_schedule(&db, first, fixture.owner, created, None).await;
    let reader = scoped_key(
        &fixture,
        "schedule-reader",
        json!([{"action":"schedule.read","resource":{"kind":"schedule","match":"ids","ids":[first,last]}}]),
    )
    .await;
    let owner_reader = scoped_key(
        &fixture,
        "schedule-owner-reader",
        json!([{"action":"schedule.read","resource":{"kind":"schedule","match":"owned_by","owner_id":fixture.owner}}]),
    )
    .await;
    let workflow_reader = fixture
        .workflow_key(fixture.owner, &fixture.token, "workflow.read")
        .await;

    let first_page = page(&fixture, &reader, &format!("{SCHEDULES}?limit=1")).await;
    assert_eq!(first_page["items"][0]["id"], json!(first));
    let cursor = first_page["next_cursor"].as_str().unwrap();
    let last_page = page(
        &fixture,
        &reader,
        &format!("{SCHEDULES}?limit=1&cursor={cursor}"),
    )
    .await;
    assert_eq!(last_page["items"].as_array().unwrap().len(), 1);
    assert_eq!(last_page["items"][0]["id"], json!(last));
    assert_eq!(last_page["next_cursor"], Value::Null);
    let owner_page = page(&fixture, &owner_reader, SCHEDULES).await;
    assert_eq!(
        owner_page["items"],
        json!([first_page["items"][0], last_page["items"][0]])
    );
    assert_eq!(
        page(&fixture, &workflow_reader, SCHEDULES).await,
        json!({"items":[],"next_cursor":null})
    );
    for (token, limit) in [(&owner_reader, 1), (&reader, 2)] {
        fixture
            .read(token, &format!("{SCHEDULES}?limit={limit}&cursor={cursor}"))
            .await
            .expect(400);
    }
    fixture
        .read(&reader, &format!("{SCHEDULES}/{last}"))
        .await
        .expect(200);
    for (token, id) in [
        (&reader, hidden),
        (&owner_reader, hidden),
        (&workflow_reader, first),
        (&fixture.token, Uuid::new_v4()),
    ] {
        let denial = fixture
            .read(token, &format!("{SCHEDULES}/{id}"))
            .await
            .expect(404);
        assert_eq!(denial.body["error"]["code"], "RESOURCE_NOT_FOUND");
    }
}

#[tokio::test]
async fn schedule_reads_reject_undeclared_inputs() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let schedule = Uuid::new_v4();
    let item = format!("{SCHEDULES}/{schedule}");
    for target in [
        format!("{SCHEDULES}?limit=0"),
        format!("{SCHEDULES}?limit=201"),
        format!("{SCHEDULES}?limit=1&limit=2"),
        format!("{SCHEDULES}?state=active"),
        format!("{SCHEDULES}?cursor=invalid"),
        format!("{item}?limit=1"),
        format!("{SCHEDULES}/invalid-uuid"),
    ] {
        fixture.read(&fixture.token, &target).await.expect(400);
    }
    request(
        &fixture.router,
        Method::GET,
        &item,
        Some(&fixture.token),
        Some(&json!({})),
        &[],
    )
    .await
    .expect(400);
}

async fn stop(
    fixture: &Fixture,
    token: &str,
    schedule: Uuid,
    tag: Option<&str>,
    key: &str,
) -> Reply {
    let mut headers = vec![("idempotency-key", key)];
    if let Some(tag) = tag {
        headers.push(("if-match", tag));
    }
    request(
        &fixture.router,
        Method::DELETE,
        &format!("{SCHEDULES}/{schedule}"),
        Some(token),
        None,
        &headers,
    )
    .await
}

async fn fires(db: &Client, schedule: Uuid) -> Vec<(String, Option<Uuid>)> {
    db.query(
        "SELECT outcome,task_id FROM schedule_fires WHERE schedule_id=$1 ORDER BY scheduled_at",
        &[&schedule],
    )
    .await
    .unwrap()
    .iter()
    .map(|row| (row.get(0), row.get(1)))
    .collect()
}

#[tokio::test]
async fn stops_future_fires_and_keeps_submitted_work_and_delegation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let schedule = Uuid::new_v4();
    let created = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let record = insert_schedule(
        &db,
        schedule,
        f.owner,
        created,
        Some(created + Duration::hours(1)),
    )
    .await;
    let task = Uuid::new_v4();
    let delegation = contracts::id(&record, "delegation_id").unwrap();
    let workflow = contracts::id(&record["spec"], "workflow_id").unwrap();
    db.execute(
        "UPDATE execution_delegations SET status='active' WHERE id=$1",
        &[&delegation],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT INTO tasks(id,owner_id,workflow_id,delegation_id,input,deadline_at,state,created_at,updated_at)
         VALUES($1,$2,$3,$4,'{\"original\":true}',now()+interval '1 day','running',now(),now())",
        &[&task, &f.owner, &workflow, &delegation],
    )
    .await
    .unwrap();
    let task_and_authority = "SELECT to_jsonb(t),to_jsonb(d) FROM tasks t
                              JOIN execution_delegations d ON d.id=t.delegation_id WHERE t.id=$1";
    let before = db.query_one(task_and_authority, &[&task]).await.unwrap();
    for (minutes, outcome, task_id) in [
        (0, "created", Some(task)),
        (1, "pending", None),
        (2, "paused", None),
        (3, "skipped", None),
        // A legacy fire with a task link must never retire that work.
        (4, "paused", Some(task)),
    ] {
        db.execute("INSERT INTO schedule_fires(schedule_id,scheduled_at,task_id,outcome) VALUES($1,$2,$3,$4)",
            &[&schedule, &(created + Duration::minutes(minutes)), &task_id, &outcome]).await.unwrap();
    }

    let old_tag = Some("\"rev-8\"");
    let key = "stop-planned-work";
    assert_eq!(
        stop(&f, &f.token, schedule, old_tag, key)
            .await
            .expect(204)
            .body,
        Value::Null
    );
    stop(&f, &f.token, schedule, old_tag, key).await.expect(204);
    stop(&f, &f.token, schedule, Some("\"rev-9\""), key)
        .await
        .expect(409);
    // An already-settled schedule can be stopped again with the current ETag.
    stop(&f, &f.token, schedule, Some("\"rev-9\""), "stop-again")
        .await
        .expect(204);

    let read = f
        .read(&f.token, &format!("{SCHEDULES}/{schedule}"))
        .await
        .expect(200);
    assert_eq!(read.etag.as_deref(), Some("\"rev-9\""));
    assert_eq!(read.body["delegation_id"], record["delegation_id"]);
    assert_eq!(read.body["next_fire_at"], Value::Null);
    let mut spec = record["spec"].clone();
    spec["enabled"] = json!(false);
    assert_eq!(read.body["spec"], spec);
    assert_eq!(
        page(&f, &f.token, SCHEDULES).await["items"],
        json!([read.body])
    );
    assert_eq!(
        fires(&db, schedule).await,
        [
            ("created".to_owned(), Some(task)),
            ("skipped".to_owned(), None),
            ("skipped".to_owned(), None),
            ("skipped".to_owned(), None),
            ("paused".to_owned(), Some(task)),
        ]
    );
    let after = db.query_one(task_and_authority, &[&task]).await.unwrap();
    for column in [0, 1] {
        assert_eq!(
            before.get::<_, Value>(column),
            after.get::<_, Value>(column)
        );
    }
    assert_eq!(db.query_one("SELECT count(*) FROM audit_events WHERE action='deleteSchedule' AND reason_code='COMMITTED'", &[]).await.unwrap().get::<_, i64>(0), 2);
}

#[tokio::test]
async fn stop_checks_scope_etag_and_request_contract_for_restored_plans() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let schedule = Uuid::new_v4();
    let record = insert_schedule(&db, schedule, f.owner, Utc::now(), None).await;
    let hidden = Uuid::new_v4();
    let (child, _) = f.child().await;
    insert_schedule(&db, hidden, child, Utc::now(), None).await;
    let scope = json!({"kind":"schedule","match":"ids","ids":[schedule]});
    let read = scoped_key(
        &f,
        "only-read-schedule",
        json!([{"action":"schedule.read","resource":scope}]),
    )
    .await;
    let delete = scoped_key(
        &f,
        "only-stop-schedule",
        json!([{"action":"schedule.delete","resource":scope}]),
    )
    .await;

    stop(&f, &read, schedule, Some("\"rev-8\""), "read-only")
        .await
        .expect(403);
    stop(&f, &delete, hidden, Some("\"rev-8\""), "hidden")
        .await
        .expect(404);
    stop(&f, &delete, schedule, None, "no-etag")
        .await
        .expect(428);
    stop(&f, &delete, schedule, Some("\"rev-7\""), "old-etag")
        .await
        .expect(412);
    let uri = format!("{SCHEDULES}/{schedule}");
    let headers = [
        ("if-match", "\"rev-8\""),
        ("idempotency-key", "unexpected-input"),
    ];
    request(
        &f.router,
        Method::DELETE,
        &format!("{uri}?force=true"),
        Some(&delete),
        None,
        &headers,
    )
    .await
    .expect(400);
    request(
        &f.router,
        Method::DELETE,
        &uri,
        Some(&delete),
        Some(&json!({})),
        &headers,
    )
    .await
    .expect(400);
    assert_eq!(f.read(&f.token, &uri).await.expect(200).body, record);
    let (first, second) = tokio::join!(
        stop(&f, &delete, schedule, Some("\"rev-8\""), "stop-restored"),
        stop(&f, &delete, schedule, Some("\"rev-8\""), "stop-concurrent"),
    );
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses, [204, 412], "{} / {}", first.body, second.body);
    assert_eq!(
        f.read(&f.token, &uri).await.expect(200).body["spec"]["enabled"],
        false
    );
    let delegation = contracts::id(&record, "delegation_id").unwrap();
    assert_eq!(
        db.query_one(
            "SELECT status FROM execution_delegations WHERE id=$1",
            &[&delegation]
        )
        .await
        .unwrap()
        .get::<_, String>(0),
        "revoked"
    );
}

#[tokio::test]
async fn stop_rolls_back_when_the_audit_wait_outlasts_authority_and_rechecks_replays() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let completed = Uuid::new_v4();
    let blocked = Uuid::new_v4();
    let scheduled_at = Utc::now() + Duration::hours(1);
    insert_schedule(&db, completed, f.owner, Utc::now(), Some(scheduled_at)).await;
    let before = insert_schedule(&db, blocked, f.owner, Utc::now(), Some(scheduled_at)).await;
    db.execute(
        "INSERT INTO schedule_fires(schedule_id,scheduled_at,outcome) VALUES($1,$2,'pending')",
        &[&blocked, &scheduled_at],
    )
    .await
    .unwrap();

    let until: DateTime<Utc> = db
        .query_one("SELECT clock_timestamp()+interval '5 seconds'", &[])
        .await
        .unwrap()
        .get(0);
    let scope = json!({"kind":"schedule","match":"owned_by","owner_id":f.owner});
    let token = scoped_key(
        &f,
        "temporary-stop",
        json!([
            {"action":"schedule.read","resource":scope},
            {"action":"schedule.delete","resource":scope,"conditions":{"expires_at":until}}
        ]),
    )
    .await;
    stop(&f, &token, completed, Some("\"rev-8\""), "completed-stop")
        .await
        .expect(204);

    let mut holder = f.db.connect().await.unwrap();
    let hold = holder.transaction().await.unwrap();
    // SHARE admits reads but blocks the INSERT of the success audit.
    hold.batch_execute("LOCK TABLE audit_events IN SHARE MODE")
        .await
        .unwrap();
    let pending = tokio::spawn(async move {
        let reply = stop(&f, &token, blocked, Some("\"rev-8\""), "blocked-stop").await;
        (f, token, reply)
    });
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let waiting: bool = db
                .query_one(
                    "SELECT EXISTS(SELECT FROM pg_locks WHERE relation='audit_events'::regclass
                   AND mode='RowExclusiveLock' AND NOT granted)",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
            if waiting {
                break;
            }
            assert!(!pending.is_finished());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stop must reach the blocked success audit");
    db.query_one(
        "SELECT pg_sleep(greatest(extract(epoch FROM $1::timestamptz-clock_timestamp()),0)+0.05)",
        &[&until],
    )
    .await
    .unwrap();
    hold.rollback().await.unwrap();
    let (f, token, reply) = pending.await.unwrap();
    reply.expect(403);
    stop(&f, &token, completed, Some("\"rev-8\""), "completed-stop")
        .await
        .expect(403);
    assert_eq!(
        f.read(&f.token, &format!("{SCHEDULES}/{blocked}"))
            .await
            .expect(200)
            .body,
        before
    );
    assert_eq!(fires(&db, blocked).await, [("pending".to_owned(), None)]);
    assert_eq!(
        db.query_one(
            "SELECT count(*) FROM idempotency_records WHERE key='blocked-stop'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn stop_rolls_back_fire_changes_and_allows_same_key_after_storage_failure() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let db = f.db.connect().await.unwrap();
    let schedule = Uuid::new_v4();
    let scheduled_at = Utc::now() + Duration::hours(1);
    let before = insert_schedule(&db, schedule, f.owner, Utc::now(), Some(scheduled_at)).await;
    db.execute(
        "INSERT INTO schedule_fires(schedule_id,scheduled_at,outcome) VALUES($1,$2,'pending')",
        &[&schedule, &scheduled_at],
    )
    .await
    .unwrap();
    db.batch_execute("ALTER TABLE schedules ADD CONSTRAINT reject_stop CHECK (spec->'enabled' IS DISTINCT FROM 'false'::jsonb)")
        .await.unwrap();
    let key = "stop-after-error";
    stop(&f, &f.token, schedule, Some("\"rev-8\""), key)
        .await
        .expect(503);
    assert_eq!(fires(&db, schedule).await, [("pending".to_owned(), None)]);
    assert_eq!(
        f.read(&f.token, &format!("{SCHEDULES}/{schedule}"))
            .await
            .expect(200)
            .body,
        before
    );
    db.batch_execute("ALTER TABLE schedules DROP CONSTRAINT reject_stop")
        .await
        .unwrap();
    stop(&f, &f.token, schedule, Some("\"rev-8\""), key)
        .await
        .expect(204);
    assert_eq!(fires(&db, schedule).await, [("skipped".to_owned(), None)]);
}
