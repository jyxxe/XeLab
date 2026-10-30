use serde_json::json;
use uuid::Uuid;
use xelab_kernel::{
    admission,
    auth::{self, Auth, Resource},
    db::Db,
};

#[tokio::test]
async fn real_postgres_enforces_authentication_and_atomic_admission() {
    let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
        eprintln!("Set XELAB_TEST_DATABASE_URL to run PostgreSQL integration tests");
        return;
    };
    let db = Db::new(dsn).unwrap();
    db.migrate().await.unwrap();
    db.migrate().await.unwrap();
    db.check().await.unwrap();
    let mut c = db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let quotas = json!({"maximum_depth":4,"maximum_descendants":20,"maximum_api_keys":50,"maximum_concurrent_calls":16,"daily_calls":10000});
    let owner = auth::bootstrap(
        &tx,
        "integration owner",
        "correct horse test password",
        &quotas,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let auth = Auth::new(vec![42; 32]).unwrap();
    let tx = c.transaction().await.unwrap();
    let issued = auth
        .login(
            &tx,
            owner,
            "correct horse test password".into(),
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let token = issued["access_token"].as_str().unwrap();
    let credential: Uuid = issued["session"]["id"].as_str().unwrap().parse().unwrap();
    let ctx = auth
        .authenticate(&tx, token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    auth::authorize(
        &tx,
        &ctx,
        "plugin.deploy",
        &Resource::new("deployment", "new", Some(owner)),
    )
    .await
    .unwrap();
    assert!(auth::authorize(
        &tx,
        &ctx,
        "unknown.action",
        &Resource::new("deployment", "new", Some(owner))
    )
    .await
    .is_err());
    assert!(auth
        .authenticate(
            &tx,
            "a fabricated bearer token of sufficient length",
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4()
        )
        .await
        .is_err());
    tx.commit().await.unwrap();
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE id=$1",
        &[&credential],
    )
    .await
    .unwrap();
    assert!(auth
        .authenticate(&c, token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .is_err());
    let config = json!({"kernel_cpu_millis":4000,"kernel_memory_bytes":17179869184u64,"system_cpu_millis":1000,"system_memory_bytes":1073741824u64,"shutdown_grace_ms":30000});
    let tx = c.transaction().await.unwrap();
    admission::initialize(&tx, &config, 7000, 34359738368)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    c.execute("UPDATE node_budget SET admission_enabled=true", &[])
        .await
        .unwrap();
    let publisher = Uuid::new_v4();
    let installation = Uuid::new_v4();
    let deployment = Uuid::new_v4();
    let limits =
        json!({"cpu_millis":1500,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":32});
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','fixture','not used in admission test','trusted',now(),now())",&[&publisher]).await.unwrap();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.fixture','1.0.0',$2,'test','test','{}','installed',now(),now())",&[&installation,&publisher]).await.unwrap();
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'reconciling',now(),now())",&[&deployment,&owner,&installation,&json!({"enabled":true,"limits":limits})]).await.unwrap();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    for (i, id) in [a, b].iter().enumerate() {
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,$3,'pending','unknown',$4,now(),now())",&[&id,&deployment,&(i as i32),&limits]).await.unwrap();
    }
    let mut handles = Vec::new();
    for id in [a, b] {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            let mut c = db.connect().await.unwrap();
            let tx = c.transaction().await.unwrap();
            match admission::reserve(&tx, id).await {
                Ok(()) => {
                    tx.commit().await.unwrap();
                    Some(id)
                }
                Err(_) => None,
            }
        }));
    }
    let mut winners = Vec::new();
    for h in handles {
        if let Some(id) = h.await.unwrap() {
            winners.push(id);
        }
    }
    assert_eq!(
        winners.len(),
        1,
        "concurrent reservations must not oversell"
    );
    let tx = c.transaction().await.unwrap();
    admission::reserve(&tx, winners[0]).await.unwrap();
    tx.commit().await.unwrap();
    let r = c
        .query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
        .await
        .unwrap();
    assert_eq!(r.get::<_, i64>(0), 1500, "replayed reserve is idempotent");
    let tx = c.transaction().await.unwrap();
    assert!(admission::release_stopped(&tx, winners[0]).await.is_err());
    tx.rollback().await.unwrap();
    c.execute(
        "UPDATE instances SET phase='stopped' WHERE id=$1",
        &[&winners[0]],
    )
    .await
    .unwrap();
    let tx = c.transaction().await.unwrap();
    admission::release_stopped(&tx, winners[0]).await.unwrap();
    admission::release_stopped(&tx, winners[0]).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        c.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}
