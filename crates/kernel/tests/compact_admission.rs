//! PostgreSQL accounting for an explicit compact fixture, independent of the
//! development machine's much larger CPU and memory capacity.
use serde_json::json;
use tokio_postgres::Client;
use uuid::Uuid;
use xelab_kernel::{admission, auth, db::Db};

const MIB: i64 = 1024 * 1024;

async fn instance(c: &Client, owner: Uuid, installation: Uuid, memory: i64) -> Uuid {
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    // Two instances fit the CPU pool; memory must decide admission.
    let limits = json!({"cpu_millis":125,"memory_bytes":memory,"ephemeral_bytes":MIB,"pids":16});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'reconciling',now(),now())", &[&deployment,&owner,&installation,&json!({"enabled":true,"limits":limits})]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'pending','unknown',$3,now(),now())", &[&instance,&deployment,&limits]).await.unwrap();
    instance
}

#[tokio::test]
async fn compact_admission_keeps_exact_memory_bounds_and_stopped_release_proof() {
    let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("SKIPPED: compact admission requires XELAB_TEST_DATABASE_URL");
        return;
    };
    // Use the same isolated-database setup as tests/postgres.rs. No runtime,
    // cgroup privileges or physical host capacity are needed for SQL admission.
    let admin = Db::new(dsn.clone()).unwrap().connect().await.unwrap();
    let name = format!("compact_admission_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{dsn} dbname={name}")).unwrap();
    db.migrate().await.unwrap();
    let mut c = db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let owner = auth::bootstrap(
        &tx,
        "compact admission owner",
        "compact admission fixture password",
        &json!({"maximum_depth":4,"maximum_descendants":20,"maximum_api_keys":50,"maximum_concurrent_calls":16,"daily_calls":10000}),
    ).await.unwrap();
    tx.commit().await.unwrap();
    let publisher = Uuid::new_v4();
    let installation = Uuid::new_v4();
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'compact fixture','fixture','not used by admission','trusted',now(),now())", &[&publisher]).await.unwrap();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.compact-fixture','1.0.0',$2,'fixture','fixture','{}','installed',now(),now())", &[&installation,&publisher]).await.unwrap();
    let config = json!({
        "profile":"compact",
        "kernel_cpu_millis":500,"kernel_memory_bytes":192*MIB,
        "system_cpu_millis":250,"system_memory_bytes":256*MIB,
        "shutdown_grace_ms":30000
    });
    // Cover nominal 512 MiB and a 32 MiB boot reservation without pretending
    // that the observed host still has all 512 MiB available.
    for (host_memory, pool_memory) in [(512 * MIB, 64 * MIB), (480 * MIB, 32 * MIB)] {
        let tx = c.transaction().await.unwrap();
        admission::initialize(&tx, &config, 1000, host_memory)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let before = admission::status(&c).await.unwrap();
        let standard = json!({"kernel_cpu_millis":4000,"kernel_memory_bytes":17179869184u64,
            "system_cpu_millis":1000,"system_memory_bytes":1073741824,"shutdown_grace_ms":30000});
        let tx = c.transaction().await.unwrap();
        assert_eq!(
            admission::initialize(&tx, &standard, 8000, 34359738368)
                .await
                .unwrap_err()
                .code,
            "STATE_CONFLICT"
        );
        // Even a caller committing after this semantic rejection cannot alter
        // capacity/revision/admission under an incompatible runtime profile.
        tx.commit().await.unwrap();
        assert_eq!(admission::status(&c).await.unwrap(), before);
        c.execute("UPDATE node_budget SET admission_enabled=true", &[])
            .await
            .unwrap();
        let status = admission::status(&c).await.unwrap();
        assert_eq!(status["host_cpu_millis"], 1000);
        assert_eq!(status["host_memory_bytes"], host_memory);
        assert_eq!(status["plugin_cpu_pool_millis"], 250);
        assert_eq!(status["plugin_memory_pool_bytes"], pool_memory);

        let too_large = instance(&c, owner, installation, pool_memory + 1).await;
        let tx = c.transaction().await.unwrap();
        assert_eq!(
            admission::reserve(&tx, too_large).await.unwrap_err().code,
            "RESOURCE_EXHAUSTED"
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM reservations WHERE instance_id=$1",
                &[&too_large]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
            0
        );

        let a = instance(&c, owner, installation, pool_memory).await;
        let b = instance(&c, owner, installation, pool_memory).await;
        let mut tasks = Vec::new();
        for candidate in [a, b] {
            let db = db.clone();
            tasks.push(tokio::spawn(async move {
                let mut c = db.connect().await.unwrap();
                let tx = c.transaction().await.unwrap();
                match admission::reserve(&tx, candidate).await {
                    Ok(()) => {
                        tx.commit().await.unwrap();
                        Some(candidate)
                    }
                    Err(error) => {
                        assert_eq!(error.code, "RESOURCE_EXHAUSTED");
                        tx.rollback().await.unwrap();
                        None
                    }
                }
            }));
        }
        let mut winners = Vec::new();
        for task in tasks {
            if let Some(candidate) = task.await.unwrap() {
                winners.push(candidate);
            }
        }
        assert_eq!(
            winners.len(),
            1,
            "concurrent compact reservations cannot oversell memory"
        );
        let winner = winners[0];
        let tx = c.transaction().await.unwrap();
        admission::reserve(&tx, winner).await.unwrap();
        tx.commit().await.unwrap();
        let status = admission::status(&c).await.unwrap();
        assert_eq!(status["reserved_cpu_millis"], 125);
        assert_eq!(status["reserved_memory_bytes"], pool_memory);

        let tx = c.transaction().await.unwrap();
        assert_eq!(
            admission::release_stopped(&tx, winner)
                .await
                .unwrap_err()
                .code,
            "STATE_CONFLICT"
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            admission::status(&c).await.unwrap()["reserved_memory_bytes"],
            pool_memory
        );
        c.execute(
            "UPDATE instances SET phase='stopped' WHERE id=$1",
            &[&winner],
        )
        .await
        .unwrap();
        let tx = c.transaction().await.unwrap();
        admission::release_stopped(&tx, winner).await.unwrap();
        admission::release_stopped(&tx, winner).await.unwrap();
        tx.commit().await.unwrap();
        let status = admission::status(&c).await.unwrap();
        assert_eq!(status["reserved_cpu_millis"], 0);
        assert_eq!(status["reserved_memory_bytes"], 0);
        let tx = c.transaction().await.unwrap();
        assert_eq!(
            admission::reserve(&tx, winner).await.unwrap_err().code,
            "STATE_CONFLICT"
        );
        tx.rollback().await.unwrap();
    }
}
