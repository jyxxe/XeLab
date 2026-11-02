use crate::{
    admission,
    api::App,
    artifact::Artifact,
    auth::{authorize, Resource},
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
    let mut probes = std::collections::HashMap::new();
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
            && tls::issue(
                &app.config,
                None,
                &app.config.root().join("kernel-identity"),
            )
            .is_ok()
        {
            rotate = tokio::time::Instant::now();
        }
        match tick(&app, first, &mut probes).await {
            Ok(()) => {
                first = false;
                app.ready.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                app.ready.store(false, Ordering::SeqCst);
                tracing::error!(
                    reason_code = e.code,
                    message = e.message,
                    "Controller reconciliation failed"
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    app.ready.store(false, Ordering::SeqCst);
}

#[derive(Default)]
struct Probe {
    next: Option<tokio::time::Instant>,
    started: Option<tokio::time::Instant>,
    failures: u64,
}
async fn tick(
    app: &App,
    first: bool,
    probes: &mut std::collections::HashMap<Uuid, Probe>,
) -> Result<()> {
    let mut c = app.db.connect().await?;
    if first {
        c.execute("UPDATE node_budget SET admission_enabled=false", &[])
            .await?;
        c.execute(
            "UPDATE provider_records SET availability='unavailable'",
            &[],
        )
        .await?;
        // A new controller must fence every old process before returning call slots.
        // Stable instance IDs and their resource reservations survive this restart.
        for id in app.runtime.inventory().await? {
            app.runtime.stop(id).await?;
        }
        let tx = c.transaction().await?;
        db::call_lock(&tx).await?;
        tx.execute(
            "UPDATE calls SET state='unknown',error=$1 WHERE state IN ('accepted','running')",
            &[&Error::new(
                "OUTCOME_UNKNOWN",
                "Kernel restarted before the result was committed",
                axum::http::StatusCode::CONFLICT,
            )
            .detail()],
        )
        .await?;
        tx.execute(
            "UPDATE rpc_delegations SET status='revoked' WHERE status='active'",
            &[],
        )
        .await?;
        tx.execute(
            "UPDATE execution_delegations SET status='revoked' WHERE status='active'",
            &[],
        )
        .await?;
        tx.execute(
            "UPDATE reservations SET status='released' WHERE kind='call' AND status<>'released'",
            &[],
        )
        .await?;
        tx.execute("UPDATE quotas SET concurrent_calls_used=0", &[])
            .await?;
        tx.execute("UPDATE instances SET phase='pending',health='unknown',restart_count=0,revision=revision+1,updated_at=now() WHERE phase<>'stopped'", &[]).await?;
        tx.commit().await?;
        c.execute(
            "UPDATE node_budget SET admission_enabled=true,revision=revision+1,updated_at=now()",
            &[],
        )
        .await?;
    }
    process_installations(app).await?;
    recover_expired_calls(app).await?;
    // A fixed upper watermark makes each sweep finite even while deployments
    // are created. Keyset pages include disabled rows without starving the tail.
    let end = c.query_opt("SELECT created_at,id FROM deployments WHERE archived_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 1", &[]).await?;
    let Some(end) = end else {
        return Ok(());
    };
    let through = (end.get(0), end.get(1));
    let mut after = None;
    loop {
        if app.shutdown.load(Ordering::SeqCst) || app.leader.is_closed() {
            return Err(Error::unavailable("Controller is stopping"));
        }
        let rows = deployment_page(&c, after, through).await?;
        if rows.is_empty() {
            break;
        }
        for r in rows {
            after = Some((r.get("created_at"), r.get("id")));
            let id: Uuid = r.get("id");
            if let Err(e) = reconcile(app, id, probes.entry(id).or_default()).await {
                tracing::warn!(deployment_id=%id,reason_code=e.code,message=e.message,"Deployment is not ready");
                c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)", &[&id]).await?;
                let failed:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM instances WHERE deployment_id=$1 AND phase='failed')", &[&id]).await?.get(0);
                let phase = if failed {
                    "failed"
                } else if e.code == "RESOURCE_EXHAUSTED" {
                    "waiting_resources"
                } else {
                    "degraded"
                };
                if failed {
                    c.execute("UPDATE operations SET state='failed',error=$2,revision=revision+1,updated_at=now() WHERE resource_id=$1 AND state IN ('pending','running')", &[&id.to_string(),&e.detail()]).await?;
                }
                c.execute(
                "UPDATE deployments SET revision=revision+CASE WHEN phase<>$2 OR conditions<>$3 THEN 1 ELSE 0 END,phase=$2,conditions=$3,updated_at=now() WHERE id=$1",
                &[&id, &phase, &json!([e.code])],
            )
            .await?;
            }
        }
    }
    Ok(())
}

type DeploymentPosition = (chrono::DateTime<chrono::Utc>, Uuid);
async fn deployment_page(
    c: &impl tokio_postgres::GenericClient,
    after: Option<DeploymentPosition>,
    through: DeploymentPosition,
) -> Result<Vec<tokio_postgres::Row>> {
    let (created, id) = after
        .map(|(created, id)| (Some(created), Some(id)))
        .unwrap_or((None, None));
    Ok(c.query("SELECT created_at,id FROM deployments WHERE archived_at IS NULL AND ($1::timestamptz IS NULL OR (created_at,id)>($1,$2)) AND (created_at,id)<=($3,$4) ORDER BY created_at,id LIMIT 1000", &[&created, &id, &through.0, &through.1]).await?)
}

async fn recover_expired_calls(app: &App) -> Result<()> {
    let mut c = app.db.connect().await?;
    let rows=c.query("SELECT DISTINCT provider_instance_id FROM calls c JOIN reservations r ON r.id=c.id AND r.kind='call' AND r.status<>'released' WHERE (c.state='unknown' OR (c.state IN ('accepted','running') AND deadline_at<now()-interval '1 second')) AND provider_instance_id IS NOT NULL", &[]).await?;
    for row in rows {
        let instance: Uuid = row.get(0);
        let tx = c.transaction().await?;
        db::call_lock(&tx).await?;
        // A running provider cannot receive another call after this fence commits.
        tx.query_one(
            "SELECT id FROM instances WHERE id=$1 FOR UPDATE",
            &[&instance],
        )
        .await?;
        tx.execute(
            "UPDATE provider_records SET availability='unavailable' WHERE instance_id=$1",
            &[&instance],
        )
        .await?;
        tx.commit().await?;
        app.runtime.stop(instance).await?;
        // Only confirmed process termination permits slot reuse. Concurrent late
        // responses are fenced by the same call-state conditional update.
        let tx = c.transaction().await?;
        db::call_lock(&tx).await?;
        let error = Error::new(
            "OUTCOME_UNKNOWN",
            "Call lease expired before its result was committed",
            axum::http::StatusCode::CONFLICT,
        )
        .detail();
        let calls=tx.query("UPDATE calls SET state='unknown',error=$2 WHERE provider_instance_id=$1 AND (state IN ('accepted','running') OR (state='unknown' AND EXISTS(SELECT 1 FROM reservations WHERE id=calls.id AND kind='call' AND status<>'released'))) RETURNING id,owner_id", &[&instance,&error]).await?;
        for call in calls {
            let id: Uuid = call.get(0);
            let owner: Uuid = call.get(1);
            let released=tx.execute("UPDATE reservations SET status='released' WHERE id=$1 AND kind='call' AND status<>'released'", &[&id]).await?;
            if released == 1 {
                for ancestor in tx.query("SELECT ancestor_id FROM account_closure WHERE descendant_id=$1 ORDER BY depth DESC", &[&owner]).await? {
                    tx.execute("UPDATE quotas SET concurrent_calls_used=greatest(0,concurrent_calls_used-1) WHERE account_id=$1", &[&ancestor.get::<_,Uuid>(0)]).await?;
                }
            }
            tx.execute(
                "UPDATE rpc_delegations SET status='revoked' WHERE call_id=$1",
                &[&id],
            )
            .await?;
            tx.execute("UPDATE execution_delegations SET status='revoked' WHERE id IN(SELECT delegation_id FROM calls WHERE id=$1)", &[&id]).await?;
            db::audit(
                &tx,
                owner,
                None,
                Uuid::new_v4(),
                "capability.invoke",
                &Resource::new("call", id, Some(owner)).json(),
                "failed",
                "CALL_LEASE_EXPIRED",
            )
            .await?;
        }
        tx.commit().await?;
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
    let peer = payload["peer"]
        .as_str()
        .and_then(|x| x.parse().ok())
        .ok_or_else(Error::credential)?;
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
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
    authorize(
        &tx,
        &ctx,
        "plugin.install",
        &Resource::new("plugin", plugin, Some(ctx.subject)),
    )
    .await?;
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
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
    tx.commit().await?;
    Ok(json!({"installation_id":id}))
}

async fn reconcile(app: &App, deployment: Uuid, probe: &mut Probe) -> Result<()> {
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
            tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
                .await?;
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
    if running
        && probe
            .next
            .is_some_and(|at| tokio::time::Instant::now() < at)
    {
        return Ok(());
    }
    probe.next = Some(
        tokio::time::Instant::now()
            + Duration::from_millis(manifest["health"]["interval_ms"].as_u64().unwrap()),
    );
    if !running {
        let count = i.get::<_, i32>("restart_count");
        if count >= 5 {
            c.execute("UPDATE instances SET phase='failed',health='unhealthy',revision=revision+1,updated_at=now() WHERE id=$1 AND phase<>'failed'", &[&id]).await?;
            return Err(Error::unavailable(
                "Instance exhausted its restart budget; disable and enable to retry",
            ));
        }
        let since = (chrono::Utc::now() - i.get::<_, chrono::DateTime<chrono::Utc>>("updated_at"))
            .num_seconds();
        if count > 0 && since < (1i64 << count.min(5)) {
            return Err(Error::unavailable("Instance restart backoff is active"));
        }
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
        probe.started = Some(tokio::time::Instant::now());
        c.execute("UPDATE instances SET restart_count=restart_count+1,phase='starting',health='unknown',updated_at=now(),revision=revision+1 WHERE id=$1", &[&id]).await?;
    }
    let obs = app
        .runtime
        .ensure(id, deployment, generation, &manifest, &image, &sha, &limits)
        .await?;
    if !obs.running {
        return Err(Error::unavailable("OCI task is not running"));
    }
    let checked: Result<rpc::HealthResponse> = async {
        let channel = tls::channel(&app.config, id, generation, &obs.address).await?;
        let mut client = PluginLifecycleClient::new(channel);
        if !running || i.get::<_, String>("phase") != "ready" {
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
        let mut health_request = tonic::Request::new(rpc::HealthRequest {
            context: Some(context(None)),
        });
        health_request.set_timeout(Duration::from_millis(
            manifest["health"]["timeout_ms"].as_u64().unwrap(),
        ));
        let health = client.get_health(health_request).await?.into_inner();
        if !health.alive || health.observed_generation != generation as u64 {
            return Err(Error::unavailable("Plugin health generation differs"));
        }
        Ok(health)
    }
    .await;
    let health = match checked {
        Ok(health) => {
            probe.failures = 0;
            health
        }
        Err(error) => {
            probe.failures += 1;
            c.execute("UPDATE instances SET health='unhealthy',revision=revision+1,updated_at=now() WHERE id=$1 AND health<>'unhealthy'", &[&id]).await?;
            let failed = if i.get::<_, String>("phase") == "ready" {
                probe.failures >= manifest["health"]["failure_threshold"].as_u64().unwrap()
            } else {
                probe.started.is_some_and(|at| {
                    at.elapsed()
                        >= Duration::from_millis(
                            manifest["health"]["startup_timeout_ms"].as_u64().unwrap(),
                        )
                })
            };
            if failed {
                app.runtime.stop(id).await?;
                probe.failures = 0;
            }
            return Err(error);
        }
    };
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
    tx.execute("UPDATE instances SET phase=$2,health='alive',runtime_id=$3,last_heartbeat_at=now(),updated_at=now(),revision=revision+1 WHERE id=$1",&[&id,&phase,&id.to_string()]).await?;
    tx.execute("UPDATE deployments SET revision=revision+CASE WHEN phase<>$2 OR observed_generation<>$3 OR conditions<>'[]'::jsonb THEN 1 ELSE 0 END,phase=$2,observed_generation=$3,conditions='[]',updated_at=now() WHERE id=$1",&[&deployment,&phase,&generation]).await?;
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
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

#[cfg(test)]
mod pagination_tests {
    use super::*;

    #[tokio::test]
    async fn controller_reconciles_deployment_1001_after_disabled_prefix() {
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            return;
        };
        let admin = db::Db::new(base.clone()).unwrap().connect().await.unwrap();
        let database = format!("controller_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {database}"))
            .await
            .unwrap();
        let db = db::Db::new(format!("{base} dbname={database}")).unwrap();
        db.migrate().await.unwrap();
        let c = db.connect().await.unwrap();
        let owner = Uuid::new_v4();
        let publisher = Uuid::new_v4();
        let installation = Uuid::new_v4();
        c.execute("INSERT INTO accounts(id,kind,status,display_name,created_at,updated_at) VALUES($1,'user','active','fixture',now(),now())", &[&owner]).await.unwrap();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','fixture','unused','trusted',now(),now())", &[&publisher]).await.unwrap();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.fixture','1.0.0',$2,'test','test','{}','installed',now(),now())", &[&installation,&publisher]).await.unwrap();
        for n in 1..=1001u128 {
            c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,approval,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'reconciling',now(),now())", &[&Uuid::from_u128(n),&owner,&installation,&json!({"enabled":n==1001}),&json!({"rules":[],"egress":[]})]).await.unwrap();
        }
        let value: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let config =
            crate::config::Config::from_value(serde_json::to_value(value).unwrap()).unwrap();
        let app = App {
            runtime: crate::runtime::Runtime::new(config.clone()),
            config,
            db: db.clone(),
            auth: crate::auth::Auth::new(vec![42; 32]).unwrap(),
            leader: Arc::new(db.leader().await.unwrap()),
            ready: std::sync::atomic::AtomicBool::new(false),
            shutdown: std::sync::atomic::AtomicBool::new(false),
        };
        tick(&app, false, &mut std::collections::HashMap::new())
            .await
            .unwrap();
        let tail = c
            .query_one(
                "SELECT phase,conditions FROM deployments WHERE id=$1",
                &[&Uuid::from_u128(1001)],
            )
            .await
            .unwrap();
        // No instance intent is provided: reconciliation must actually reach
        // the tail and record that error, without needing an OCI runtime.
        assert_eq!(tail.get::<_, String>(0), "degraded");
        assert_ne!(tail.get::<_, Value>(1), json!([]));
        let disabled: i64 = c
            .query_one(
                "SELECT count(*) FROM deployments WHERE phase='disabled'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(disabled, 1000);
    }

    #[tokio::test]
    async fn deployment_sweep_passes_disabled_prefix_and_handles_deletions() {
        let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            return;
        };
        let db = db::Db::new(dsn).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        // Shadow only this query's relation so the pagination test is isolated
        // from concurrently running database/OCI fixtures.
        tx.batch_execute("CREATE TEMP TABLE deployments(id uuid PRIMARY KEY,created_at timestamptz NOT NULL,archived_at timestamptz,spec jsonb) ON COMMIT DROP").await.unwrap();
        let at = chrono::Utc::now();
        for n in 1..=1002u128 {
            tx.execute(
                "INSERT INTO deployments VALUES($1,$2,NULL,$3)",
                &[&Uuid::from_u128(n), &at, &json!({"enabled":n>1000})],
            )
            .await
            .unwrap();
        }
        let end = (at, Uuid::from_u128(1002));
        let first = deployment_page(&tx, None, end).await.unwrap();
        assert_eq!(first.len(), 1000);
        let last = first.last().unwrap();
        let cursor = Some((last.get(0), last.get(1)));
        tx.execute(
            "DELETE FROM deployments WHERE id=$1",
            &[&Uuid::from_u128(1000)],
        )
        .await
        .unwrap();
        tx.execute(
            "UPDATE deployments SET archived_at=now() WHERE id=$1",
            &[&Uuid::from_u128(1001)],
        )
        .await
        .unwrap();
        tx.execute(
            "INSERT INTO deployments VALUES($1,$2,NULL,'{}')",
            &[&Uuid::from_u128(1003), &at],
        )
        .await
        .unwrap();
        let second = deployment_page(&tx, cursor, end).await.unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].get::<_, Uuid>(1), Uuid::from_u128(1002));
        assert!(deployment_page(&tx, Some(end), end)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            deployment_page(&tx, Some(end), (at, Uuid::from_u128(1003)))
                .await
                .unwrap()
                .len(),
            1
        );
        tx.rollback().await.unwrap();
    }
}
