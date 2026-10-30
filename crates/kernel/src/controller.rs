use crate::{
    admission,
    api::App,
    auth::{authorize, Resource},
    artifact::Artifact,
    contracts::{digest, strict_json},
    db,
    error::{Error, Result},
    rpc::{self, plugin_lifecycle_client::PluginLifecycleClient},
    tls,
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use uuid::Uuid;

pub fn context(subject: Option<Uuid>) -> rpc::RpcContext {
    rpc::RpcContext {
        request_id: Uuid::new_v4().to_string(),
        delegation_token: String::new(),
        deadline_unix_ms: (chrono::Utc::now() + chrono::Duration::seconds(30)).timestamp_millis(),
        idempotency_key: String::new(),
        subject_id: subject.map(|s| s.to_string()).unwrap_or_default(),
        actor_instance_id: String::new(),
    }
}

pub async fn run(app: Arc<App>) {
    let mut first = true;
    let mut rotate = tokio::time::Instant::now();
    loop {
        if app.shutdown.load(Ordering::SeqCst) {
            break;
        }
        if app.leader.is_closed() {
            app.ready.store(false, Ordering::SeqCst);
            tracing::error!(reason_code = "LEADER_CONNECTION_LOST");
            break;
        }
        if rotate.elapsed() > Duration::from_secs(1800)
            && tls::issue(&app.config, None, &app.config.root().join("kernel-identity")).is_ok()
        { rotate = tokio::time::Instant::now(); }
        match tick(&app, first).await {
            Ok(()) => {
                first = false;
                app.ready.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                app.ready.store(false, Ordering::SeqCst);
                tracing::error!(reason_code = e.code, "Controller reconciliation failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    app.ready.store(false, Ordering::SeqCst);
}

async fn tick(app: &App, first: bool) -> Result<()> {
    let mut c = app.db.connect().await?;
    if first {
        c.execute("UPDATE node_budget SET admission_enabled=false", &[]).await?;
        c.execute("UPDATE provider_records SET availability='unavailable'", &[]).await?;
        // A new controller must fence every old process before returning call slots.
        // Stable instance IDs and their resource reservations survive this restart.
        for id in app.runtime.inventory().await? { app.runtime.stop(id).await?; }
        let tx = c.transaction().await?;
        tx.execute("UPDATE calls SET state='unknown',error=$1 WHERE state IN ('accepted','running')", &[&Error::new("OUTCOME_UNKNOWN","Kernel restarted before the result was committed",axum::http::StatusCode::CONFLICT).detail()]).await?;
        tx.execute("UPDATE rpc_delegations SET status='revoked' WHERE status='active'", &[]).await?;
        tx.execute("UPDATE execution_delegations SET status='revoked' WHERE status='active'", &[]).await?;
        tx.execute("UPDATE quotas SET concurrent_calls_used=0", &[]).await?;
        tx.execute("UPDATE instances SET phase='pending',health='unknown',restart_count=0,revision=revision+1,updated_at=now() WHERE phase<>'stopped'", &[]).await?;
        tx.commit().await?;
        c.execute(
            "UPDATE node_budget SET admission_enabled=true,revision=revision+1,updated_at=now()",
            &[],
        )
        .await?;
    }
    process_installations(app).await?;
    let rows=c.query("SELECT id FROM deployments WHERE archived_at IS NULL ORDER BY created_at,id LIMIT 1000",&[]).await?;
    for r in rows {
        let id: Uuid = r.get(0);
        if let Err(e) = reconcile(app, id).await {
            tracing::warn!(deployment_id=%id,reason_code=e.code,"Deployment is not ready");
            c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)", &[&id]).await?;
            let phase = if e.code == "RESOURCE_EXHAUSTED" {
                "waiting_resources"
            } else {
                "degraded"
            };
            c.execute(
                "UPDATE deployments SET phase=$2,conditions=$3,updated_at=now() WHERE id=$1",
                &[&id, &phase, &json!([e.code])],
            )
            .await?;
        }
    }
    Ok(())
}

async fn process_installations(app: &App) -> Result<()> {
    let c = app.db.connect().await?;
    let rows=c.query("SELECT b.id,b.aggregate_id,b.payload FROM outbox b JOIN operations o ON o.id=b.aggregate_id WHERE b.topic='kernel.install' AND b.delivered_at IS NULL AND o.state IN ('pending','running') ORDER BY b.created_at LIMIT 4",&[]).await?;
    for row in rows {
        let event: Uuid = row.get(0);
        let operation: Uuid = row.get(1);
        let payload: Value = row.get(2);
        let request = &payload["request"];
        let result = install(app, request, operation, &payload).await;
        let mut c = app.db.connect().await?;
        let tx = c.transaction().await?;
        db::complete(&tx, operation, result).await?;
        tx.execute(
            "UPDATE outbox SET delivered_at=now(),attempts=attempts+1 WHERE id=$1",
            &[&event],
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

async fn install(app: &App, request: &Value, operation: Uuid, payload: &Value) -> Result<Value> {
    let credential = crate::contracts::id(payload, "credential_id")?;
    let peer = payload["peer"].as_str().and_then(|x|x.parse().ok()).ok_or_else(Error::credential)?;
    if app.leader.is_closed() { return Err(Error::unavailable("Controller leadership is lost")); }
    let c = app.db.connect().await?;
    app.auth.resume(&c, credential, peer, operation).await?;
    let publisher = crate::contracts::id(request, "publisher_id")?;
    let c = app.db.connect().await?;
    let row = c
        .query_opt(
            "SELECT public_key_pem FROM publishers WHERE id=$1 AND status='trusted'",
            &[&publisher],
        )
        .await?
        .ok_or_else(Error::denied)?;
    let artifact = Artifact::inspect(&app.runtime, request, &row.get::<_, String>(0)).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    if tx
        .query_opt(
            "SELECT id FROM publishers WHERE id=$1 AND status='trusted' FOR SHARE",
            &[&publisher],
        )
        .await?
        .is_none()
    {
        return Err(Error::denied());
    }
    let ctx = app.auth.resume(&tx, credential, peer, operation).await?;
    let plugin = artifact.manifest["plugin_id"].as_str().unwrap();
    authorize(&tx, &ctx, "plugin.install", &Resource::new("plugin", plugin, Some(ctx.subject))).await?;
    let version = artifact.manifest["version"].as_str().unwrap();
    let existing=tx.query_opt("SELECT id,image_digest,manifest_digest FROM installations WHERE plugin_id=$1 AND version=$2 FOR UPDATE",&[&plugin,&version]).await?;
    let id = if let Some(r) = existing {
        if r.get::<_, String>("image_digest") != artifact.image_digest
            || r.get::<_, String>("manifest_digest") != artifact.manifest_digest
        {
            return Err(Error::conflict(
                "Installed version already has different immutable content",
            ));
        }
        r.get("id")
    } else {
        let id = Uuid::new_v4();
        tx.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())",&[&id,&plugin,&version,&publisher,&artifact.image_digest,&artifact.manifest_digest,&artifact.manifest]).await?;
        for cap in artifact.manifest["capabilities"].as_array().unwrap() {
            let contract = &cap["contract"];
            let name = contract["name"].as_str().unwrap();
            let v = contract["version"].as_str().unwrap();
            let d = contract["schema_digest"].as_str().unwrap();
            tx.execute("INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4) ON CONFLICT(name,version) DO NOTHING",&[&name,&v,&d,&cap]).await?;
            let current=tx.query_one("SELECT schema_digest FROM capability_contracts WHERE name=$1 AND version=$2 FOR SHARE",&[&name,&v]).await?;
            if current.get::<_, String>(0) != d {
                return Err(Error::conflict(
                    "Capability version has a different schema digest",
                ));
            }
        }
        id
    };
        // The immutable full reference is retained in the operation result and a private, root-owned image index.
        tls::atomic_file(
            &app.config
                .root()
                .join("images")
                .join(&artifact.image_digest[7..])
                .with_extension("reference"),
            artifact.image.as_bytes(),
            0o600,
        )?;
    tx.execute(
        "UPDATE operations SET resource_id=$2 WHERE id=$1",
        &[&operation, &id.to_string()],
    )
    .await?;
    db::outbox(&tx, "installation.installed", id, &json!({})).await?;
    if app.leader.is_closed() { return Err(Error::unavailable("Controller leadership is lost")); }
    tx.commit().await?;
    Ok(json!({"installation_id":id}))
}

async fn reconcile(app: &App, deployment: Uuid) -> Result<()> {
    let c = app.db.connect().await?;
    let d=c.query_one("SELECT d.*,i.manifest,i.manifest_digest,i.image_digest FROM deployments d JOIN installations i ON i.id=d.installation_id WHERE d.id=$1",&[&deployment]).await?;
    let spec: Value = d.get("spec");
    let manifest: Value = d.get("manifest");
    let instances=c.query("SELECT * FROM instances WHERE deployment_id=$1 AND phase<>'stopped' ORDER BY generation",&[&deployment]).await?;
    if spec["enabled"] != true {
        c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&deployment]).await?;
        for i in instances {
            let id: Uuid = i.get("id");
            if let Ok(address) = app.runtime.address(id) {
                if let Ok(channel) =
                    tls::channel(&app.config, id, i.get("generation"), &address).await
                {
                    let mut client = PluginLifecycleClient::new(channel);
                    let mut request = tonic::Request::new(rpc::DrainRequest {
                        context: Some(context(None)),
                        grace_ms: 30000,
                    });
                    request.set_timeout(Duration::from_secs(30));
                    let _ = client.drain(request).await;
                }
            }
            app.runtime.stop(id).await?;
            let mut c = app.db.connect().await?;
            let tx = c.transaction().await?;
            tx.execute("UPDATE instances SET phase='stopped',health='unknown',updated_at=now(),revision=revision+1 WHERE id=$1",&[&id]).await?;
            admission::release_stopped(&tx, id).await?;
            tx.commit().await?;
        }
        c.execute(
            "UPDATE deployments SET phase='disabled',conditions='[]',updated_at=now() WHERE id=$1",
            &[&deployment],
        )
        .await?;
        finish_deployment_operations(&c, deployment, false).await?;
        return Ok(());
    }
    if d.get::<_, Value>("approval") != json!({"rules":[],"egress":[]}) {
        return Err(Error::denied());
    }
    let i = instances
        .last()
        .ok_or_else(|| Error::conflict("Enabled deployment has no admitted instance intent"))?;
    let id: Uuid = i.get("id");
    let generation: i64 = i.get("generation");
    let limits: Value = i.get("limits");
    let running = app.runtime.running(id).await?;
    if !running {
        let count = i.get::<_,i32>("restart_count");
        if count >= 5 { return Err(Error::unavailable("Instance exhausted its restart budget; disable and enable to retry")); }
        let since = (chrono::Utc::now()-i.get::<_,chrono::DateTime<chrono::Utc>>("updated_at")).num_seconds();
        if count > 0 && since < (1i64 << count.min(5)) { return Err(Error::unavailable("Instance restart backoff is active")); }
    }
    let mut dbc = app.db.connect().await?;
    let tx = dbc.transaction().await?;
    admission::reserve(&tx, id).await?;
    tx.commit().await?;
    let sha: String = d.get("image_digest");
    let image = std::fs::read_to_string(
        app.config
            .root()
            .join("images")
            .join(&sha[7..])
            .with_extension("reference"),
    )?;
    if !running {
        c.execute("UPDATE instances SET restart_count=restart_count+1,phase='starting',health='unknown',updated_at=now(),revision=revision+1 WHERE id=$1", &[&id]).await?;
    }
    let obs = app
        .runtime
        .ensure(id, deployment, generation, &manifest, &image, &sha, &limits)
        .await?;
    if !obs.running {
        return Err(Error::unavailable("OCI task is not running"));
    }
    let channel = tls::channel(&app.config, id, generation, &obs.address).await?;
    let mut client = PluginLifecycleClient::new(channel);
    if !running || i.get::<_,String>("phase") != "ready" {
    let description = client
        .describe(rpc::DescribeRequest {
            context: Some(context(None)),
        })
        .await?
        .into_inner();
    if description.manifest_digest != d.get::<_, String>("manifest_digest")
        || digest(&description.manifest_json) != description.manifest_digest
        || strict_json(&description.manifest_json, 1048576)? != manifest
    {
        return Err(Error::conflict(
            "Describe does not match the signed installation",
        ));
    }
    let initialized = client
        .initialize(rpc::InitializeRequest {
            context: Some(context(None)),
            deployment_id: deployment.to_string(),
            instance_id: id.to_string(),
            generation: generation as u64,
            config_json: serde_json::to_vec(&spec["config"])?,
            config_digest: digest(&serde_json::to_vec(&spec["config"])?),
            approval_revision: d.get::<_, i64>("revision") as u64,
        })
        .await?
        .into_inner();
    if !initialized.initialized || initialized.observed_generation != generation as u64 {
        return Err(Error::conflict("Plugin initialization generation differs"));
    }
    }
    let health = client
        .get_health(rpc::HealthRequest {
            context: Some(context(None)),
        })
        .await?
        .into_inner();
    if !health.alive || health.observed_generation != generation as u64 {
        return Err(Error::unavailable("Plugin health generation differs"));
    }
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    let current = tx
        .query_one(
            "SELECT spec,approval FROM deployments WHERE id=$1 FOR UPDATE",
            &[&deployment],
        )
        .await?;
    if current.get::<_, Value>("spec") != spec
        || current.get::<_, Value>("approval") != json!({"rules":[],"egress":[]})
    {
        return Err(Error::conflict(
            "Deployment changed while runtime was starting",
        ));
    }
    let mut all_ready = true;
    for cap in manifest["capabilities"].as_array().unwrap() {
        let name = cap["contract"]["name"].as_str().unwrap();
        let version = cap["contract"]["version"].as_str().unwrap();
        let digest = cap["contract"]["schema_digest"].as_str().unwrap();
        let ready = health.capabilities.iter().any(|h| {
            h.ready
                && h.contract.as_ref().is_some_and(|c| {
                    c.name == name && c.version == version && c.schema_digest == digest
                })
        });
        all_ready &= ready;
        let state = if ready { "ready" } else { "unavailable" };
        tx.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,$4,'[]',$5,now()) ON CONFLICT(instance_id,name,version) DO UPDATE SET availability=excluded.availability,observed_generation=excluded.observed_generation,observed_at=now()",&[&id,&name,&version,&state,&generation]).await?;
    }
    let phase = if all_ready { "ready" } else { "degraded" };
    tx.execute("UPDATE instances SET phase=$2,health='alive',runtime_id=$3,last_heartbeat_at=now(),updated_at=now() WHERE id=$1",&[&id,&phase,&id.to_string()]).await?;
    tx.execute("UPDATE deployments SET phase=$2,observed_generation=$3,conditions='[]',updated_at=now() WHERE id=$1",&[&deployment,&phase,&generation]).await?;
    if app.leader.is_closed() { return Err(Error::unavailable("Controller leadership is lost")); }
    if all_ready {
        finish_deployment_operations(&tx, deployment, true).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn finish_deployment_operations(
    c: &impl tokio_postgres::GenericClient,
    id: Uuid,
    enabled: bool,
) -> Result<()> {
    c.execute("UPDATE operations SET state='succeeded',progress_percent=100,result=$2,updated_at=now(),revision=revision+1 WHERE resource_id=$1 AND kind IN ('createDeployment','setDeploymentEnabled') AND state IN ('pending','running') AND EXISTS(SELECT 1 FROM deployments WHERE id=$3 AND (spec->>'enabled')::boolean=$4)",&[&id.to_string(),&json!({"deployment_id":id}),&id,&enabled]).await?;
    Ok(())
}
