use super::*;
use std::time::Duration;

#[tokio::test]
async fn connection_budget_is_shared_and_both_entry_points_fail_fast() {
    let db = Db::new("host=127.0.0.1 port=1".into())
        .unwrap()
        .with_connection_limit(1);
    let clone = db.clone();
    let permit = db.connection_permit().unwrap().unwrap();
    let error = tokio::time::timeout(Duration::from_millis(100), clone.connect())
        .await
        .expect("nested connections must fail without waiting")
        .expect_err("capacity is already held");
    assert_eq!(error.code, "RESOURCE_EXHAUSTED");
    let error = tokio::time::timeout(
        Duration::from_millis(100),
        clone.connect_scoped(Duration::from_secs(1)),
    )
    .await
    .expect("scoped connections must use the same budget")
    .err()
    .expect("capacity is already held");
    assert_eq!(error.code, "RESOURCE_EXHAUSTED");
    drop(permit);
    assert!(clone.connection_permit().unwrap().is_some());
}

#[tokio::test]
async fn canceled_connect_releases_its_handshake_reservation() {
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let db = Db::new(format!(
        "host=127.0.0.1 port={} sslmode=disable",
        server.local_addr().unwrap().port()
    ))
    .unwrap()
    .with_connection_limit(1);
    let clone = db.clone();
    let attempt = tokio::spawn(async move { clone.connect().await });
    let (_socket, _) = tokio::time::timeout(Duration::from_secs(2), server.accept())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(db.connections.as_ref().unwrap().available_permits(), 0);
    attempt.abort();
    assert!(attempt.await.unwrap_err().is_cancelled());
    assert!(db.connection_permit().unwrap().is_some());
}

#[tokio::test]
async fn actual_postgres_budget_counts_drivers_after_request_cancellation() {
    let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: connection budget lifetime requires PostgreSQL");
        return;
    };
    let admin = Db::new(dsn.clone()).unwrap().connect().await.unwrap();
    let db = Db::new(dsn).unwrap().with_connection_limit(1);
    let client = db.connect().await.unwrap();
    let pid: i32 = client
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let lock = Uuid::new_v4().as_u128() as i64;
    admin
        .query_one("SELECT pg_advisory_lock($1)", &[&lock])
        .await
        .unwrap();
    let waiting = tokio::spawn(async move {
        client
            .query_one("SELECT pg_advisory_lock($1)", &[&lock])
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let blocked: bool = admin
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND NOT granted)",
                    &[&pid],
                )
                .await
                .unwrap()
                .get(0);
            if blocked {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("query must actually wait on the server lock");
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(db.connect().await.err().unwrap().code, "RESOURCE_EXHAUSTED");
    admin
        .query_one("SELECT pg_advisory_unlock($1)", &[&lock])
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while db.connections.as_ref().unwrap().available_permits() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("finished driver must return its reservation");
    let scoped = db.connect_scoped(Duration::from_secs(2)).await.unwrap();
    drop(scoped);
    // Drop initiates cancellation, but only the protocol task owns the permit.
    assert_eq!(db.connections.as_ref().unwrap().available_permits(), 0);
    tokio::time::timeout(Duration::from_secs(3), async {
        while db.connections.as_ref().unwrap().available_permits() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("scoped cancellation must eventually return capacity");
    assert_eq!(
        db.connect()
            .await
            .unwrap()
            .query_one("SELECT 1::integer", &[])
            .await
            .unwrap()
            .get::<_, i32>(0),
        1
    );
}
