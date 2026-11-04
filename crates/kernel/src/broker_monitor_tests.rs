//! Actual PostgreSQL authorization-fence scheduling. No OCI/provider claim.
use super::*;
use tokio::sync::oneshot;

async fn queued(observer: &tokio_postgres::Client, pid: i32) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = observer
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' AND NOT granted)",
                    &[&pid],
                )
                .await
                .unwrap()
                .get(0);
            if waiting {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the exact backend must really queue on the authorization fence");
}

#[tokio::test]
async fn actual_postgres_queued_revocation_does_not_freeze_the_execution_reader() {
    let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: live-call authorization scheduling requires PostgreSQL");
        return;
    };
    let admin = db::Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("call_monitor_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = db::Db::new(format!("{base} dbname={name}")).unwrap();
    let mut execution_connection = db.connect_scoped(Duration::from_secs(10)).await.unwrap();
    let execution_tx = execution_connection.transaction().await.unwrap();
    db::auth_lock(&execution_tx, false).await.unwrap();

    let mut writer_connection = db.connect_scoped(Duration::from_secs(10)).await.unwrap();
    let writer_pid: i32 = writer_connection
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let mut monitor_connection = db.connect_scoped(Duration::from_secs(10)).await.unwrap();
    let monitor_pid: i32 = monitor_connection
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let observer = db.connect().await.unwrap();
    let writer = tokio::spawn(async move {
        let transaction = writer_connection.transaction().await?;
        db::auth_lock(&transaction, true).await?;
        transaction.commit().await?;
        Ok::<_, Error>(())
    });
    // Establish the writer-before-monitor queue in PostgreSQL, not by sleeps.
    queued(&observer, writer_pid).await;
    let (release_execution, released) = oneshot::channel();
    let observer_task = tokio::spawn(async move {
        queued(&observer, monitor_pid).await;
        release_execution.send(()).unwrap();
    });
    let execution = async {
        released
            .await
            .map_err(|_| Error::unavailable("Execution barrier disappeared"))?;
        execution_tx.commit().await?;
        Ok::<_, Error>(json!({"execution_committed":true}))
    };
    let monitor = async {
        let transaction = monitor_connection.transaction().await?;
        db::auth_lock(&transaction, false).await?;
        transaction.commit().await?;
        std::future::pending::<Result<Value>>().await
    };
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        race_live_call(execution, monitor, std::future::pending()),
    )
    .await
    .expect("a queued guard reader must not stop execution from releasing its read fence")
    .unwrap();
    assert_eq!(result, json!({"execution_committed":true}));
    tokio::time::timeout(Duration::from_secs(3), writer)
        .await
        .expect("revocation writer must finish promptly, before any call deadline")
        .unwrap()
        .unwrap();
    observer_task.await.unwrap();
    println!(
        "Actual PostgreSQL: execution read held; revoke writer queued; guard read queued behind writer; execution committed and revoke completed in {}ms",
        started.elapsed().as_millis()
    );
}
