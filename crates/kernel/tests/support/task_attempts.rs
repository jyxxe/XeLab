use super::{request, Fixture};
use axum::http::Method;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::contracts;

async fn create_task(fixture: &Fixture, token: &str, key: &str) -> (Uuid, [Uuid; 2]) {
    let mut spec = fixture.spec("attempt history");
    let mut sibling = spec["steps"][0].clone();
    sibling["id"] = json!("sibling");
    spec["steps"].as_array_mut().unwrap().push(sibling);
    let workflow = fixture.publish(token, &spec, key).await.expect(201).body;
    let task = request(
        &fixture.router,
        Method::POST,
        "/api/platform/v1/tasks",
        Some(token),
        Some(&json!({"workflow_id": workflow["id"], "input": {}})),
        &[("idempotency-key", key)],
    )
    .await
    .expect(201)
    .body;
    let task = contracts::id(&task, "id").unwrap();
    let steps = fixture
        .read(token, &format!("/api/platform/v1/tasks/{task}/steps"))
        .await
        .expect(200)
        .body;
    let ids: Vec<_> = steps["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| contracts::id(step, "id").unwrap())
        .collect();
    (task, ids.try_into().unwrap())
}

fn attempts_uri(step: Uuid) -> String {
    format!("/api/platform/v1/task-steps/{step}/attempts")
}

async fn read_page(fixture: &Fixture, token: &str, uri: &str) -> Value {
    let page = fixture.read(token, uri).await.expect(200).body;
    contracts::validate("AttemptPage", &page).unwrap();
    page
}

async fn insert_attempt(
    db: &Client,
    step: Uuid,
    id: Uuid,
    number: i32,
    created_at: DateTime<Utc>,
    checkpoint: Option<Uuid>,
) -> Uuid {
    let execution = Uuid::new_v4();
    let key = format!("retry-{step}");
    let fence = 5_000_000_000_i64 + i64::from(number);
    db.execute(
        "INSERT INTO attempts(id, step_id, number, execution_id, idempotency_key, fence,
                              created_at, updated_at, lease_expires_at, state, revision, checkpoint_blob_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$7::timestamptz + interval '2 seconds',
                 $7::timestamptz + interval '30 seconds','expired',3,$8)",
        &[&id, &step, &number, &execution, &key, &fence, &created_at, &checkpoint],
    )
    .await
    .unwrap();
    execution
}

#[tokio::test]
async fn list_attempts_returns_contract_fields_and_pages_through_tied_timestamps() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let (_, [step, sibling]) = create_task(&fixture, &fixture.token, "history-task").await;
    let uri = attempts_uri(step);
    assert_eq!(
        read_page(&fixture, &fixture.token, &uri).await,
        json!({"items": [], "next_cursor": null})
    );

    let db = fixture.db.connect().await.unwrap();
    let checkpoint = Uuid::new_v4();
    db.execute(
        "INSERT INTO blobs(id,owner_id,content_type,size_bytes,digest,backend_reference,
                           expires_at,created_at,updated_at)
         VALUES ($1,$2,'application/octet-stream',0,$3,$4,now()+interval '1 day',now(),now())",
        &[
            &checkpoint,
            &fixture.owner,
            &contracts::digest(b""),
            &checkpoint.to_string(),
        ],
    )
    .await
    .unwrap();
    let oldest = Uuid::from_u128(30);
    let tied_low = Uuid::from_u128(0x10000000_0000_0000_0000_000000000000);
    let tied_high = Uuid::from_u128(0x2a000000_0000_0000_0000_000000000000);
    let start = DateTime::from_timestamp(1_700_000_000, 123_456_000).unwrap();
    let later = start + Duration::seconds(1);
    let execution = insert_attempt(&db, step, oldest, 1, start, Some(checkpoint)).await;
    insert_attempt(&db, step, tied_high, 2, later, None).await;
    insert_attempt(&db, step, tied_low, 3, later, None).await;
    insert_attempt(&db, sibling, Uuid::new_v4(), 1, start, None).await;

    let first = read_page(&fixture, &fixture.token, &format!("{uri}?limit=2")).await;
    let items = first["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let row = &items[0];
    assert_eq!(row["id"], json!(oldest));
    assert_eq!(row["step_id"], json!(step));
    assert_eq!(row["revision"], 3);
    assert_eq!(row["number"], 1);
    assert_eq!(row["execution_id"], json!(execution));
    assert_eq!(row["idempotency_key"], json!(format!("retry-{step}")));
    assert_eq!(row["fence"], json!(5_000_000_001_i64));
    assert_eq!(row["state"], "expired");
    assert_eq!(row["checkpoint_blob_id"], json!(checkpoint));
    for (field, expected) in [
        ("created_at", start),
        ("updated_at", start + Duration::seconds(2)),
        ("lease_expires_at", start + Duration::seconds(30)),
    ] {
        let raw = row[field].as_str().unwrap();
        assert!(raw.ends_with('Z'), "{field}: {raw}");
        let actual: DateTime<Utc> = raw.parse().unwrap();
        assert_eq!(actual, expected, "{field}");
    }
    assert_eq!(items[1]["id"], json!(tied_low));
    assert_eq!(items[1].get("checkpoint_blob_id"), Some(&Value::Null));
    let cursor = first["next_cursor"].as_str().unwrap();
    let last = read_page(
        &fixture,
        &fixture.token,
        &format!("{uri}?limit=2&cursor={cursor}"),
    )
    .await;
    assert_eq!(last["items"].as_array().unwrap().len(), 1);
    assert_eq!(last["items"][0]["id"], json!(tied_high));
    assert_eq!(last["next_cursor"], Value::Null);
}

#[tokio::test]
async fn list_attempts_authorizes_parent_task_and_binds_cursor_to_step_and_identity() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let (_, child_token) = fixture.child().await;
    let (task, [step, sibling]) = create_task(&fixture, &child_token, "visible-task").await;
    let (_, [hidden, _]) = create_task(&fixture, &child_token, "hidden-task").await;
    let reader = request(
        &fixture.router,
        Method::POST,
        &format!("/api/platform/v1/accounts/{}/api-keys", fixture.owner),
        Some(&fixture.token),
        Some(&json!({
            "name": "one-task-reader",
            "inheritance": {"mode": "selected", "rules": [{
                "action": "task.read", "resource": {"kind": "task", "match": "ids", "ids": [task]}
            }]},
            "conditions": {"expires_at": Utc::now() + Duration::days(1)}
        })),
        &[("idempotency-key", "one-task-reader")],
    )
    .await
    .expect(201)
    .body;
    let reader = reader["secret"].as_str().unwrap();
    let db = fixture.db.connect().await.unwrap();
    let start = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let first_id = Uuid::from_u128(1);
    let last_id = Uuid::from_u128(2);
    insert_attempt(&db, step, first_id, 1, start, None).await;
    insert_attempt(&db, step, last_id, 2, start, None).await;

    let uri = attempts_uri(step);
    let sibling_uri = attempts_uri(sibling);
    let first = read_page(&fixture, reader, &format!("{uri}?limit=1")).await;
    assert_eq!(first["items"][0]["id"], json!(first_id));
    let cursor = first["next_cursor"].as_str().unwrap();
    let query = format!("?limit=1&cursor={cursor}");
    let last = read_page(&fixture, reader, &format!("{uri}{query}")).await;
    assert_eq!(last["items"][0]["id"], json!(last_id));

    read_page(&fixture, reader, &sibling_uri).await;
    read_page(&fixture, &child_token, &uri).await;
    for (token, target) in [(reader, &sibling_uri), (child_token.as_str(), &uri)] {
        let error = fixture
            .read(token, &format!("{target}{query}"))
            .await
            .expect(400);
        assert_eq!(error.body["error"]["code"], "INVALID_ARGUMENT");
    }

    for (token, unavailable) in [(reader, hidden), (child_token.as_str(), Uuid::new_v4())] {
        let error = fixture
            .read(token, &attempts_uri(unavailable))
            .await
            .expect(404);
        assert_eq!(error.body["error"]["code"], "RESOURCE_NOT_FOUND");
    }
}

#[tokio::test]
async fn list_attempts_rejects_invalid_queries() {
    let Some(fixture) = Fixture::new().await else {
        return;
    };
    let (_, [step, _]) = create_task(&fixture, &fixture.token, "query-task").await;
    let uri = attempts_uri(step);
    for query in [
        "limit=0",
        "limit=201",
        "limit=abc",
        "limit=1&limit=2",
        "state=expired",
        "cursor=",
        "cursor=invalid",
    ] {
        let error = fixture
            .read(&fixture.token, &format!("{uri}?{query}"))
            .await;
        assert_eq!(error.status, 400, "{query}: {}", error.body);
        assert_eq!(error.body["error"]["code"], "INVALID_ARGUMENT", "{query}");
    }
}
