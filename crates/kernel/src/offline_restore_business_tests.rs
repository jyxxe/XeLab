//! A restored format is data history, not revived authority. Explicitly log in,
//! re-trust the publisher and approve the disabled deployment before starting.
#[path = "../tests/support/http_host.rs"]
mod http_host;
use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;

async fn request(
    config: &Config,
    router: &Router,
    token: Option<&str>,
    action: &str,
    id: Option<Uuid>,
    body: Value,
    revision: Option<i64>,
) -> Result<(u16, Value)> {
    let (mut path, method) = crate::contracts::openapi()["paths"]
        .as_object()
        .unwrap()
        .iter()
        .find_map(|(path, methods)| {
            methods
                .as_object()
                .unwrap()
                .iter()
                .find_map(|(method, spec)| {
                    (spec["operationId"] == action).then(|| (path.clone(), method.to_uppercase()))
                })
        })
        .unwrap();
    if let Some(id) = id {
        for name in ["deployment_id", "operation_id"] {
            path = path.replace(&format!("{{{name}}}"), &id.to_string());
        }
    }
    let mut req = Request::builder()
        .header(
            "host",
            http_host::authority(Some(config.text("server", "public_origin"))),
        )
        .method(method.as_str())
        .uri(path)
        .header("idempotency-key", Uuid::new_v4().to_string());
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    if let Some(revision) = revision {
        req = req.header("if-match", format!("\"rev-{revision}\""));
    }
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        req = req.header("content-type", "application/json");
        serde_json::to_vec(&body)?
    };
    let mut req = req.body(Body::from(bytes)).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
    ));
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 16777216).await.unwrap();
    Ok((
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        },
    ))
}
async fn finish(config: &Config, router: &Router, token: &str, id: Uuid) -> Result<()> {
    for _ in 0..120 {
        let response = request(
            config,
            router,
            Some(token),
            "getOperation",
            Some(id),
            Value::Null,
            None,
        )
        .await?;
        assert_eq!(response.0, 200, "{}", response.1);
        assert_ne!(response.1["state"], "failed", "{}", response.1);
        if response.1["state"] == "succeeded" {
            crate::contracts::validate("Deployment", &response.1["result"])?;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(Error::unavailable(
        "Restored versioned deployment did not settle",
    ))
}

async fn exercise_source(
    app: &Arc<crate::api::App>,
    router: &Router,
    token: &str,
    c: &tokio_postgres::Client,
    source: &Value,
) -> Result<()> {
    let volume: Uuid = source["volume_id"].as_str().unwrap().parse().unwrap();
    let deployment: Uuid = source["deployment_id"].as_str().unwrap().parse().unwrap();
    let schema = source["schema"].as_str().unwrap();
    let publisher=c.query_one("SELECT p.id,p.name,p.key_id,p.public_key_pem,p.status,i.manifest FROM deployments d JOIN installations i ON i.id=d.installation_id JOIN publishers p ON p.id=i.publisher_id WHERE d.id=$1", &[&deployment]).await?;
    let manifest: Value = publisher.get("manifest");
    assert_eq!(manifest["data_schema_version"], schema);
    let current = request(
        &app.config,
        router,
        Some(token),
        "getDeployment",
        Some(deployment),
        Value::Null,
        None,
    )
    .await?;
    assert_eq!(current.0, 200, "{}", current.1);
    assert_eq!(current.1["spec"]["enabled"], false);
    if publisher.get::<_, String>("status") == "revoked" {
        let denied = request(
            &app.config,
            router,
            Some(token),
            "setDeploymentEnabled",
            Some(deployment),
            json!({"enabled":true}),
            current.1["revision"].as_i64(),
        )
        .await?;
        assert_ne!(
            denied.0, 202,
            "restored format does not restore publisher authority"
        );
        let trusted=request(&app.config,router,Some(token),"trustPublisher",None,json!({"name":publisher.get::<_,String>("name"),"key_id":publisher.get::<_,String>("key_id"),"public_key_pem":publisher.get::<_,String>("public_key_pem")}),None).await?;
        assert_eq!(trusted.0, 200, "{}", trusted.1);
    } else {
        assert_eq!(
            publisher.get::<_, String>("status"),
            "trusted",
            "only explicit current Owner trust from the earlier source can be reused"
        );
    }
    let current = request(
        &app.config,
        router,
        Some(token),
        "getDeployment",
        Some(deployment),
        Value::Null,
        None,
    )
    .await?;
    let approved = request(
        &app.config,
        router,
        Some(token),
        "approvePlugin",
        Some(deployment),
        json!({"rules":[],"egress":[]}),
        current.1["revision"].as_i64(),
    )
    .await?;
    assert_eq!(approved.0, 200, "{}", approved.1);
    let current = request(
        &app.config,
        router,
        Some(token),
        "getDeployment",
        Some(deployment),
        Value::Null,
        None,
    )
    .await?;
    let enabled = request(
        &app.config,
        router,
        Some(token),
        "setDeploymentEnabled",
        Some(deployment),
        json!({"enabled":true}),
        current.1["revision"].as_i64(),
    )
    .await?;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    finish(
        &app.config,
        router,
        token,
        enabled.1["id"].as_str().unwrap().parse().unwrap(),
    )
    .await?;
    let row = c
        .query_one(
            "SELECT root_id,project_id FROM volume_allocations WHERE volume_id=$1",
            &[&volume],
        )
        .await?;
    let mut store = crate::storage::xfs::ProjectStore::open(&app.config.root().join("volumes"))?;
    let filesystem = store.status()?.filesystem_uuid;
    store.namespace_identity(filesystem, row.get("root_id"), false)?;
    let data = store.data_path(volume, row.get::<_, i32>("project_id") as u32)?;
    drop(store);
    let before: Value = serde_json::from_slice(&fs::read(data.join("store.json"))?)?;
    let called=request(&app.config,router,Some(token),"invokeCapability",None,json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":format!("business write after whole-platform schema{schema} recovery")},"provider_deployment_id":deployment,"timeout_ms":30000}),None).await?;
    assert_eq!(called.0, 200, "{}", called.1);
    let after: Value = serde_json::from_slice(&fs::read(data.join("store.json"))?)?;
    assert_eq!(after["schema"], schema);
    match schema {
        "1" => {
            assert_eq!(after["items"].as_array().unwrap().len(), 2);
            assert_eq!(after["items"][0], before["items"][0]);
            assert_eq!(after["items"][1]["id"], called.1["call_id"]);
        }
        "2" => {
            let records = after["records"].as_object().unwrap();
            assert_eq!(records.len(), 2);
            for (id, value) in before["records"].as_object().unwrap() {
                assert_eq!(&records[id], value);
            }
            assert!(records.contains_key(called.1["call_id"].as_str().unwrap()));
        }
        _ => panic!("unsupported reference format"),
    }
    let current = request(
        &app.config,
        router,
        Some(token),
        "getDeployment",
        Some(deployment),
        Value::Null,
        None,
    )
    .await?;
    let disabled = request(
        &app.config,
        router,
        Some(token),
        "setDeploymentEnabled",
        Some(deployment),
        json!({"enabled":false}),
        current.1["revision"].as_i64(),
    )
    .await?;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    finish(
        &app.config,
        router,
        token,
        disabled.1["id"].as_str().unwrap().parse().unwrap(),
    )
    .await?;
    assert!(app.runtime.inventory().await?.is_empty());
    let budget = c
        .query_one(
            "SELECT reserved_cpu_millis,reserved_memory_bytes FROM node_budget",
            &[],
        )
        .await?;
    assert_eq!(budget.get::<_, i64>(0), 0);
    assert_eq!(budget.get::<_, i64>(1), 0);
    println!("Whole schema{schema} recovery: original data retained and fresh current-Owner-approved OCI/Broker disk write passed; no format Apply or old authority revival");
    Ok(())
}

pub(super) async fn resume(config: &Config, db: &Db, info: &Value) -> Result<()> {
    let owner: Uuid = info["owner_id"].as_str().unwrap().parse().unwrap();
    let leader = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match db.leader().await {
                Ok(leader) => return Ok(leader),
                Err(error)
                    if error.code == "STATE_CONFLICT"
                        && error.message == "A kernel controller already owns this database" =>
                {
                    tokio::time::sleep(Duration::from_millis(25)).await
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .map_err(|_| Error::unavailable("Completed recovery leader did not close"))??;
    let mut c = db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, true).await?;
    // Synthetic component pool only, never production RUN03 evidence.
    crate::admission::initialize(&tx, &config.value["resources"], 8000, 34359738368).await?;
    tx.commit().await?;
    let (_, key) = config.secrets()?;
    let app = Arc::new(crate::api::App {
        config: config.clone(),
        runtime: Runtime::new(config.clone()),
        db: db.clone(),
        auth: crate::auth::Auth::new(key)?,
        leader: Arc::new(leader),
        ready: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        blob_uploads: tokio::sync::Semaphore::new(4),
    });
    crate::tls::issue(config, None, &config.root().join("kernel-identity"))?;
    app.runtime.prepare_host_service_network().await?;
    let router = crate::api::router(app.clone());
    let controller = tokio::spawn(crate::controller::run(app.clone()));
    let result = async {
        for _ in 0..120 {
            if app.ready.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert!(app.ready.load(Ordering::SeqCst));
        assert_eq!(
            c.query_one(
                "SELECT count(*) FROM publishers WHERE status='trusted'",
                &[]
            )
            .await?
            .get::<_, i64>(0),
            0
        );
        let login = request(
            config,
            &router,
            None,
            "login",
            None,
            json!({"account_id":owner,"password":PASSWORD}),
            None,
        )
        .await?;
        assert_eq!(login.0, 200, "{}", login.1);
        let token = login.1["access_token"].as_str().unwrap();
        let sources = info["versioned_sources"].as_array().unwrap();
        assert_eq!(sources.len(), 2);
        for source in sources {
            exercise_source(&app, &router, token, &c, source).await?;
        }
        Ok(())
    }
    .await;
    app.shutdown.store(true, Ordering::SeqCst);
    controller
        .await
        .map_err(|_| Error::unavailable("Recovery business controller stopped"))?;
    result
}
