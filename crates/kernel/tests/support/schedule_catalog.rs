//! Read-only Schedule API coverage using persisted, pre-existing metadata.
use super::{request, Fixture};
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

async fn scoped_key(fixture: &Fixture, name: &str, selector: Value) -> String {
    let reply = request(
        &fixture.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", fixture.owner),
        Some(&fixture.token),
        Some(&json!({
            "name":name,
            "inheritance":{"mode":"selected","rules":[{"action":"schedule.read","resource":selector}]},
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
        json!({"kind":"schedule","match":"ids","ids":[first,last]}),
    )
    .await;
    let owner_reader = scoped_key(
        &fixture,
        "schedule-owner-reader",
        json!({"kind":"schedule","match":"owned_by","owner_id":fixture.owner}),
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
