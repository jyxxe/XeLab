//! The owning bootstrap supervisor. No HTTP waiter owns a runtime mutation.
//! A persisted initializing checkpoint is ambiguous until actual Verify succeeds.
use super::*;
use crate::{
    admission,
    artifact::Artifact,
    controller, db,
    migration_rpc::{Binding, Peer, Plan},
    rpc::{self, plugin_lifecycle_client::PluginLifecycleClient},
    storage::xfs::ProjectStore,
    tls,
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

const SCRATCH: &str = "/xelab-migration";
const SCRATCH_BYTES: u64 = 67_108_864;
const MAX_FENCE: i64 = 9_007_199_254_740_991;

#[cfg(test)]
#[path = "schema_jobs_tests.rs"]
mod tests;

async fn job(app: &App, operation: Uuid) -> Result<Row> {
    Ok(app.db.connect().await?.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM volume_schema_jobs j WHERE operation_id=$1", &[&operation]).await?)
}

async fn check(app: &App, operation: Uuid, fence: &db::DeploymentFence) -> Result<Row> {
    let row = job(app, operation).await?;
    if !fence.protects(row.get("deployment_id")) || app.shutdown.load(Ordering::SeqCst) {
        return Err(Error::unavailable("Schema bootstrap runtime fence is lost"));
    }
    if matches!(
        row.get::<_, String>("state").as_str(),
        "verified" | "failed"
    ) {
        return Err(Error::conflict("Schema bootstrap is already terminal"));
    }
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, &row).await?;
    tx.commit().await?;
    Ok(row)
}

fn ids(row: &Row) -> Result<Vec<Uuid>> {
    row.get::<_, Value>("volume_ids")
        .as_array()
        .filter(|a| !a.is_empty() && a.len() <= 16)
        .ok_or_else(|| Error::conflict("Incomplete bootstrap volume group"))?
        .iter()
        .map(|v| {
            v.as_str()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| Error::conflict("Invalid bootstrap volume"))
        })
        .collect()
}

async fn instances(app: &App, row: &Row) -> Result<Vec<Uuid>> {
    let mut result: BTreeSet<Uuid> = app.db.connect().await?.query("SELECT i.id FROM instances i WHERE i.deployment_id=$1 AND (i.phase<>'stopped' OR EXISTS(SELECT 1 FROM volume_attachments a WHERE a.instance_id=i.id)) LIMIT 33", &[&row.get::<_, Uuid>("deployment_id")]).await?.iter().map(|r|r.get(0)).collect();
    result.insert(row.get("instance_id"));
    if result.len() > 32 {
        return Err(Error::exhausted());
    }
    Ok(result.into_iter().collect())
}

/// Attempt every stop, including a stopped SQL row with a lingering attachment.
/// An unconfirmed stop retains all holds and allocations for a later owner.
async fn stop_all(app: &App, row: &Row) -> Result<Vec<Uuid>> {
    let instances = instances(app, row).await?;
    let mut outcome = Ok(());
    for instance in &instances {
        if let Err(error) = app.runtime.stop(*instance).await {
            outcome = Err(error);
        }
    }
    outcome?;
    Ok(instances)
}

async fn freeze(app: &App, row: &Row, fence: &db::DeploymentFence) -> Result<()> {
    let operation: Uuid = row.get("operation_id");
    let deployment: Uuid = row.get("deployment_id");
    check(app, operation, fence).await?;
    // An old controller tick may have passed its hold check before admission.
    // We now own its fence and withdraw anything that tick published.
    app.db.connect().await?.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)", &[&deployment]).await?;
    let stopped = stop_all(app, row).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    revalidate(&tx, app, row).await?;
    for instance in stopped {
        volumes::release_stopped(&tx, Some(instance)).await?;
        if instance != row.get::<_, Uuid>("instance_id") {
            tx.execute("UPDATE instances SET phase='stopped',health='unknown',runtime_id=NULL,revision=revision+1,updated_at=now() WHERE id=$1", &[&instance]).await?;
            admission::release_stopped(&tx, instance).await?;
        }
    }
    admission::reserve(&tx, row.get("instance_id")).await?;
    tx.execute("UPDATE operations SET state='running',revision=revision+1,updated_at=now() WHERE id=$1 AND state='pending'", &[&operation]).await?;
    tx.commit().await?;
    Ok(())
}

/// Inspect actual XFS directories only after all writers have positively stopped.
async fn bind_empty(app: &App, row: &Row, fence: &db::DeploymentFence) -> Result<()> {
    let operation: Uuid = row.get("operation_id");
    let selected = ids(row)?;
    let sources = volumes::backup_sources(app, operation, &selected).await?;
    let c = app.db.connect().await?;
    let mut unknown = Vec::new();
    for source in &sources {
        let state: String = c
            .query_one(
                "SELECT schema_state FROM volume_allocations WHERE volume_id=$1",
                &[&source.id],
            )
            .await?
            .get(0);
        if state == "unversioned" {
            unknown.push((source.id, source.project_id));
        }
    }
    let path = app.config.root().join("volumes");
    // This task is always joined; the owning job is not cancelled on timeout.
    tokio::task::spawn_blocking(move || -> Result<()> {
        let store = ProjectStore::open(&path)?;
        for (volume, project) in unknown {
            if !store.data_is_empty(volume, project)? {
                return Err(Error::conflict("Unversioned business data is not empty"));
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::unavailable("Bootstrap filesystem inspection stopped"))??;
    check(app, operation, fence).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    tx.query_one("SELECT id FROM volume_storage_root FOR UPDATE", &[])
        .await?;
    tx.query_one(
        "SELECT id FROM deployments WHERE id=$1 FOR UPDATE",
        &[&row.get::<_, Uuid>("deployment_id")],
    )
    .await?;
    revalidate(&tx, app, row).await?;
    let current = tx
        .query_one(
            "SELECT state,volume_fence FROM volume_schema_jobs WHERE operation_id=$1 FOR UPDATE",
            &[&operation],
        )
        .await?;
    if current.get::<_, String>("state") != "planning" {
        return Err(Error::conflict("Bootstrap planning checkpoint changed"));
    }
    let mut high = 0;
    for volume in &selected {
        let v = tx
            .query_one(
                "SELECT write_fence,writer_instance_id FROM volumes WHERE id=$1 FOR UPDATE",
                &[&volume],
            )
            .await?;
        if v.get::<_, Option<Uuid>>("writer_instance_id").is_some()
            || tx
                .query_opt(
                    "SELECT instance_id FROM volume_attachments WHERE volume_id=$1 LIMIT 1",
                    &[&volume],
                )
                .await?
                .is_some()
        {
            return Err(Error::conflict("Bootstrap source still has an attachment"));
        }
        high = high.max(v.get::<_, i64>("write_fence"));
    }
    let token = match current.get::<_, Option<i64>>("volume_fence") {
        Some(token) if token == high => token,
        Some(_) => return Err(Error::conflict("Bootstrap volume fence changed")),
        None => high
            .checked_add(1)
            .filter(|f| *f <= MAX_FENCE)
            .ok_or_else(Error::exhausted)?,
    };
    let metadata: Value = row.get("installation_metadata");
    let plugin = metadata["plugin_id"].as_str().unwrap();
    let publisher = id(&metadata, "publisher_id")?;
    let manifest_digest = metadata["manifest_digest"].as_str().unwrap();
    let target: String = row.get("target_schema");
    let deployment: Uuid = row.get("deployment_id");
    for volume in &selected {
        let a = tx.query_one("SELECT schema_state,data_schema_version,schema_deployment_id,schema_plugin_id,schema_publisher_id,schema_manifest_digest FROM volume_allocations WHERE volume_id=$1 FOR UPDATE", &[&volume]).await?;
        let state: String = a.get("schema_state");
        let bound = a.get::<_, Option<String>>("data_schema_version").as_deref() == Some(&target)
            && a.get::<_, Option<String>>("schema_plugin_id").as_deref() == Some(plugin)
            && a.get::<_, Option<Uuid>>("schema_publisher_id") == Some(publisher);
        if (state == "unversioned"
            && !matches!(
                a.get::<_, Option<String>>("data_schema_version").as_deref(),
                None | Some("none")
            ))
            || (state != "unversioned" && !bound)
            || (state == "initializing"
                && (a.get::<_, Option<Uuid>>("schema_deployment_id") != Some(deployment)
                    || a.get::<_, Option<String>>("schema_manifest_digest")
                        .as_deref()
                        != Some(manifest_digest)))
        {
            return Err(Error::conflict("Bootstrap schema binding changed"));
        }
        // Already verified members stay verified until the atomic final proof.
        if state != "verified" {
            tx.execute("UPDATE volume_allocations SET data_schema_version=$2,schema_state='initializing',schema_deployment_id=$3,schema_plugin_id=$4,schema_publisher_id=$5,schema_manifest_digest=$6,schema_operation_id=$7,schema_proof=NULL,updated_at=now() WHERE volume_id=$1", &[&volume,&target,&deployment,&plugin,&publisher,&manifest_digest,&operation]).await?;
        }
        tx.execute(
            "UPDATE volumes SET write_fence=$2,revision=revision+1,updated_at=now() WHERE id=$1",
            &[&volume, &token],
        )
        .await?;
    }
    tx.execute(
        "UPDATE volume_schema_jobs SET volume_fence=$2,updated_at=now() WHERE operation_id=$1",
        &[&operation, &token],
    )
    .await?;
    if !fence.protects(deployment) {
        return Err(Error::unavailable("Bootstrap fence was lost"));
    }
    tx.commit().await?;
    Ok(())
}

async fn mounts(
    app: &App,
    row: &Row,
    writable: bool,
    scratch: bool,
) -> Result<Vec<volumes::Mount>> {
    let token = row
        .get::<_, Option<i64>>("volume_fence")
        .filter(|v| (1..=MAX_FENCE).contains(v))
        .ok_or_else(|| Error::conflict("Missing bootstrap fence"))?;
    let metadata: Value = row.get("installation_metadata");
    let manifest = &metadata["manifest"];
    let mut mounts = Vec::new();
    for source in volumes::backup_sources(app, row.get("operation_id"), &ids(row)?).await? {
        let request = manifest["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == source.spec["name"])
            .ok_or_else(|| Error::conflict("Bootstrap alias changed"))?;
        let destination = request["mount_path"].as_str().unwrap();
        if destination == SCRATCH
            || destination.starts_with(&format!("{SCRATCH}/"))
            || SCRATCH.starts_with(&format!("{destination}/"))
        {
            return Err(Error::conflict("Business mount overlaps migration scratch"));
        }
        mounts.push(volumes::Mount {
            volume: source.id,
            source: source.path,
            destination: destination.to_owned(),
            access: if writable { "rw" } else { "ro" }.to_owned(),
            fence: token,
        });
    }
    if scratch {
        let source = volumes::private_restore_path(
            app,
            row.get("operation_id"),
            row.get("scratch_volume_id"),
            true,
        )
        .await?;
        if source.capacity_bytes != SCRATCH_BYTES {
            return Err(Error::conflict("Migration scratch quota changed"));
        }
        mounts.push(volumes::Mount {
            volume: source.id,
            source: source.path,
            destination: SCRATCH.to_owned(),
            access: "rw".to_owned(),
            fence: token,
        });
    }
    Ok(mounts)
}

async fn start(
    app: &App,
    row: &Row,
    artifact: &Artifact,
    fence: &db::DeploymentFence,
    writable: bool,
    scratch: bool,
) -> Result<()> {
    check(app, row.get("operation_id"), fence).await?;
    let mounts = mounts(app, row, writable, scratch).await?;
    check(app, row.get("operation_id"), fence).await?;
    let observed = app
        .runtime
        .ensure(
            row.get("instance_id"),
            row.get("deployment_id"),
            row.get("generation"),
            &artifact.manifest,
            &artifact.image,
            &artifact.image_digest,
            &row.get::<_, Value>("deployment_spec")["limits"],
            &mounts,
        )
        .await?;
    if !observed.running {
        return Err(Error::unavailable("Bootstrap instance is not running"));
    }
    Ok(())
}

fn remaining(row: &Row, cap: Duration) -> Result<Duration> {
    Ok(
        (row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at") - chrono::Utc::now())
            .to_std()
            .map_err(|_| Error::conflict("Bootstrap deadline expired"))?
            .min(cap),
    )
}

async fn peer(
    app: &App,
    row: &Row,
    artifact: &Artifact,
    fence: &db::DeploymentFence,
) -> Result<Peer> {
    let end = tokio::time::Instant::now()
        + remaining(
            row,
            Duration::from_millis(
                artifact.manifest["health"]["startup_timeout_ms"]
                    .as_u64()
                    .unwrap(),
            ),
        )?;
    loop {
        check(app, row.get("operation_id"), fence).await?;
        let binding = Binding {
            operation: row.get("operation_id"),
            deployment: row.get("deployment_id"),
            subject: row.get("subject_id"),
            instance: row.get("instance_id"),
            generation: row.get("generation"),
            expires_unix_ms: row
                .get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
                .timestamp_millis(),
            source_schema: row.get("source_schema"),
            target_schema: row.get("target_schema"),
        };
        let left = end
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| Error::unavailable("Bootstrap startup deadline expired"))?;
        let attempt = tokio::time::timeout(
            left,
            Peer::connect(
                &app.config,
                binding,
                &app.runtime.address(row.get("instance_id"))?,
                &artifact.manifest,
                &artifact.manifest_digest,
            ),
        )
        .await
        .map_err(|_| Error::unavailable("Bootstrap startup deadline expired"))?;
        match attempt {
            Ok(peer) => return Ok(peer),
            Err(error) => {
                if tokio::time::Instant::now() >= end {
                    return Err(error);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn context(row: &Row, phase: &str) -> rpc::RpcContext {
    let mut context = controller::context(Some(row.get("subject_id")));
    context.idempotency_key = format!("{}:{phase}", row.get::<_, Uuid>("operation_id"));
    context.deadline_unix_ms = context.deadline_unix_ms.min(
        row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            .timestamp_millis(),
    );
    context
}

/// A fresh initialization is sent once after its durable checkpoint. Following
/// Verify, this same stable request may activate an already confirmed format.
async fn initialize(app: &App, row: &Row) -> Result<()> {
    let channel = tls::channel(
        &app.config,
        row.get("instance_id"),
        row.get("generation"),
        &app.runtime.address(row.get("instance_id"))?,
    )
    .await?;
    let spec: Value = row.get("deployment_spec");
    let config = serde_json::to_vec(&spec["config"])?;
    let mut request = tonic::Request::new(rpc::InitializeRequest {
        context: Some(context(row, "initialize")),
        deployment_id: row.get::<_, Uuid>("deployment_id").to_string(),
        instance_id: row.get::<_, Uuid>("instance_id").to_string(),
        generation: row.get::<_, i64>("generation") as u64,
        config_digest: crate::contracts::digest(&config),
        config_json: config,
        approval_revision: row.get::<_, i64>("generation") as u64,
    });
    let limit = remaining(row, Duration::from_secs(30))?;
    request.set_timeout(limit);
    let response = tokio::time::timeout(
        limit,
        PluginLifecycleClient::new(channel).initialize(request),
    )
    .await
    .map_err(|_| Error::unavailable("Bootstrap initialization outcome is unknown"))??
    .into_inner();
    if !response.initialized
        || response.observed_generation != row.get::<_, i64>("generation") as u64
    {
        return Err(Error::conflict("Bootstrap initialization identity differs"));
    }
    Ok(())
}

async fn health(app: &App, row: &Row, artifact: &Artifact) -> Result<rpc::HealthResponse> {
    let channel = tls::channel(
        &app.config,
        row.get("instance_id"),
        row.get("generation"),
        &app.runtime.address(row.get("instance_id"))?,
    )
    .await?;
    let limit = remaining(
        row,
        Duration::from_millis(artifact.manifest["health"]["timeout_ms"].as_u64().unwrap()),
    )?;
    let mut request = tonic::Request::new(rpc::HealthRequest {
        context: Some(context(row, "health")),
    });
    request.set_timeout(limit);
    let response = tokio::time::timeout(
        limit,
        PluginLifecycleClient::new(channel).get_health(request),
    )
    .await
    .map_err(|_| Error::unavailable("Bootstrap health deadline expired"))??
    .into_inner();
    let expected: BTreeSet<_> = artifact.manifest["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["contract"]["name"].as_str().unwrap().to_owned(),
                v["contract"]["version"].as_str().unwrap().to_owned(),
                v["contract"]["schema_digest"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut actual = BTreeSet::new();
    for capability in &response.capabilities {
        let contract = capability
            .contract
            .as_ref()
            .ok_or_else(|| Error::conflict("Bootstrap health contract is missing"))?;
        if !capability.ready
            || !actual.insert((
                contract.name.clone(),
                contract.version.clone(),
                contract.schema_digest.clone(),
            ))
        {
            return Err(Error::unavailable("Bootstrap capability is not ready"));
        }
    }
    if !response.alive
        || response.observed_generation != row.get::<_, i64>("generation") as u64
        || actual != expected
    {
        return Err(Error::unavailable(
            "Bootstrap health differs from admitted manifest",
        ));
    }
    Ok(response)
}

fn plan(row: &Row) -> Result<Plan> {
    serde_json::from_value(
        row.get::<_, Option<Value>>("plan")
            .ok_or_else(|| Error::conflict("Missing bootstrap plan"))?,
    )
    .map_err(|_| Error::conflict("Invalid bootstrap plan"))
}

async fn wait_health(
    app: &App,
    row: &Row,
    artifact: &Artifact,
    fence: &db::DeploymentFence,
) -> Result<()> {
    let end = tokio::time::Instant::now()
        + remaining(
            row,
            Duration::from_millis(
                artifact.manifest["health"]["startup_timeout_ms"]
                    .as_u64()
                    .unwrap(),
            ),
        )?;
    loop {
        check(app, row.get("operation_id"), fence).await?;
        let left = end
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| Error::unavailable("Bootstrap health startup deadline expired"))?;
        let result = tokio::time::timeout(left, health(app, row, artifact))
            .await
            .map_err(|_| Error::unavailable("Bootstrap health startup deadline expired"))?;
        match result {
            Ok(_) => return Ok(()),
            Err(error) => {
                if tokio::time::Instant::now() >= end {
                    return Err(error);
                }
            }
        }
        // Health can settle asynchronously. Do not repeat Initialize to poll it.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn checkpoint(
    app: &App,
    row: &Row,
    fence: &db::DeploymentFence,
    from: &str,
    to: &str,
    plan: Option<&Plan>,
) -> Result<()> {
    check(app, row.get("operation_id"), fence).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    let plan = plan.map(serde_json::to_value).transpose()?;
    if tx.execute("UPDATE volume_schema_jobs SET state=$3,plan=coalesce($4,plan),updated_at=now() WHERE operation_id=$1 AND state=$2", &[&row.get::<_, Uuid>("operation_id"),&from,&to,&plan]).await? != 1 { return Err(Error::conflict("Bootstrap phase changed")); }
    if !fence.protects(row.get("deployment_id")) {
        return Err(Error::unavailable("Bootstrap fence was lost"));
    }
    tx.commit().await?;
    Ok(())
}

async fn publish(
    app: &App,
    row: &Row,
    artifact: &Artifact,
    fence: &db::DeploymentFence,
) -> Result<()> {
    let operation: Uuid = row.get("operation_id");
    let deployment: Uuid = row.get("deployment_id");
    let instance: Uuid = row.get("instance_id");
    let generation: i64 = row.get("generation");
    if !app.runtime.running(instance).await? {
        return Err(Error::unavailable(
            "Bootstrap instance stopped before publication",
        ));
    }
    // Fresh namespace/quota proof, outside the short final metadata transaction.
    let sources = volumes::backup_sources(app, operation, &ids(row)?).await?;
    let plan = plan(row)?;
    let token = row
        .get::<_, Option<i64>>("volume_fence")
        .ok_or_else(|| Error::conflict("Missing bootstrap fence"))?;
    let proof = json!({"operation_id":operation,"plan_id":plan.id,"plan_digest":plan.digest,"volume_fence":token,"observed_schema":row.get::<_, String>("target_schema"),"instance_id":instance,"generation":generation,"manifest_digest":artifact.manifest_digest});
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    tx.query_one("SELECT id FROM volume_storage_root FOR UPDATE", &[])
        .await?;
    tx.query_one(
        "SELECT id FROM deployments WHERE id=$1 FOR UPDATE",
        &[&deployment],
    )
    .await?;
    let state: String = tx
        .query_one(
            "SELECT state FROM volume_schema_jobs WHERE operation_id=$1 FOR UPDATE",
            &[&operation],
        )
        .await?
        .get(0);
    if state != "verifying" {
        return Err(Error::conflict("Bootstrap publication phase changed"));
    }
    let ctx = revalidate(&tx, app, row).await?;
    if tx.query_opt("SELECT instance_id FROM reservations WHERE instance_id=$1 AND status IN ('reserved','active')", &[&instance]).await?.is_none() { return Err(Error::conflict("Bootstrap resource reservation was lost")); }
    let metadata: Value = row.get("installation_metadata");
    let publisher = id(&metadata, "publisher_id")?;
    for source in sources {
        let volume = tx
            .query_one(
                "SELECT write_fence,writer_instance_id FROM volumes WHERE id=$1 FOR UPDATE",
                &[&source.id],
            )
            .await?;
        if volume.get::<_, i64>("write_fence") != token
            || volume
                .get::<_, Option<Uuid>>("writer_instance_id")
                .is_some()
            || tx
                .query_opt(
                    "SELECT instance_id FROM volume_attachments WHERE volume_id=$1 LIMIT 1",
                    &[&source.id],
                )
                .await?
                .is_some()
        {
            return Err(Error::conflict("Bootstrap attachment or fence changed"));
        }
        let request = artifact.manifest["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == source.spec["name"])
            .ok_or_else(|| Error::conflict("Bootstrap alias changed"))?;
        tx.execute("UPDATE volume_allocations SET data_schema_version=$2,schema_state='verified',schema_deployment_id=$3,schema_plugin_id=$4,schema_publisher_id=$5,schema_manifest_digest=$6,schema_operation_id=$7,schema_proof=$8,updated_at=now() WHERE volume_id=$1", &[&source.id,&row.get::<_, String>("target_schema"),&deployment,&metadata["plugin_id"].as_str().unwrap(),&publisher,&artifact.manifest_digest,&operation,&proof]).await?;
        tx.execute("INSERT INTO volume_attachments(volume_id,instance_id,access,fence,generation,mount_path) VALUES($1,$2,'rw',$3,$4,$5)", &[&source.id,&instance,&token,&generation,&request["mount_path"].as_str().unwrap()]).await?;
        tx.execute("UPDATE volumes SET writer_instance_id=$2,state=CASE WHEN state='full' THEN state ELSE 'attached' END,revision=revision+1,updated_at=now() WHERE id=$1", &[&source.id,&instance]).await?;
    }
    volumes::approve_mounts(
        &tx,
        &ctx,
        deployment,
        generation,
        &row.get("deployment_spec"),
        &artifact.manifest,
    )
    .await?;
    for cap in artifact.manifest["capabilities"].as_array().unwrap() {
        let contract = &cap["contract"];
        tx.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',$4,now()) ON CONFLICT(instance_id,name,version) DO UPDATE SET availability='ready',observed_generation=excluded.observed_generation,observed_at=now()", &[&instance,&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap(),&generation]).await?;
    }
    tx.execute("UPDATE instances SET phase='ready',health='alive',runtime_id=$2,last_heartbeat_at=now(),revision=revision+1,updated_at=now() WHERE id=$1", &[&instance,&instance.to_string()]).await?;
    tx.execute("UPDATE deployments SET phase='ready',observed_generation=$2,conditions='[]',revision=revision+1,updated_at=now() WHERE id=$1", &[&deployment,&generation]).await?;
    // The supervisor positively stopped the scratch-mounted container and
    // recreated this instance without scratch before reaching publication.
    tx.execute("UPDATE volume_allocations SET state='quarantined',updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring'", &[&row.get::<_, Uuid>("scratch_volume_id"),&operation]).await?;
    tx.execute(
        "UPDATE volume_schema_jobs SET state='verified',updated_at=now() WHERE operation_id=$1",
        &[&operation],
    )
    .await?;
    tx.execute(
        "DELETE FROM volume_holds WHERE operation_id=$1 AND kind='migration'",
        &[&operation],
    )
    .await?;
    tx.execute(
        "DELETE FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",
        &[&deployment, &operation],
    )
    .await?;
    let result = crate::api::read_object(&tx, "deployments", deployment, "Deployment").await?;
    db::complete(&tx, operation, Ok(result)).await?;
    db::audit(
        &tx,
        ctx.subject,
        Some(ctx.credential),
        operation,
        "plugin.enable",
        &Resource::new("deployment", deployment, Some(ctx.subject)).json(),
        "succeeded",
        "SCHEMA_VERIFIED",
    )
    .await?;
    if !fence.protects(deployment) || app.leader.is_closed() {
        return Err(Error::unavailable("Bootstrap publication fence was lost"));
    }
    tx.commit().await?;
    Ok(())
}

async fn process(app: &App, operation: Uuid, fence: &db::DeploymentFence) -> Result<()> {
    let row = job(app, operation).await?;
    if row.get::<_, String>("state") == "verified" {
        return Ok(());
    }
    let row = check(app, operation, fence).await?;
    let artifact = upgrades::artifact(app, &row.get("installation_metadata")).await?;
    freeze(app, &row, fence).await?;
    let mut row = check(app, operation, fence).await?;
    if row.get::<_, String>("state") == "planning" {
        bind_empty(app, &row, fence).await?;
        row = check(app, operation, fence).await?;
        start(app, &row, &artifact, fence, false, true).await?;
        let peer = peer(app, &row, &artifact, fence).await?;
        check(app, operation, fence).await?;
        let planned = peer.plan().await?;
        if !planned.requires_exclusive_write || planned.temporary_bytes_required > SCRATCH_BYTES {
            return Err(Error::exhausted());
        }
        checkpoint(app, &row, fence, "planning", "planned", Some(&planned)).await?;
        row = check(app, operation, fence).await?;
    }
    let fresh = row.get::<_, String>("state") == "planned"
        && row
            .get::<_, Value>("volume_plan")
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["schema_state"] != "initializing");
    app.runtime.stop(row.get("instance_id")).await?;
    if row.get::<_, String>("state") == "planned" {
        checkpoint(app, &row, fence, "planned", "initializing", None).await?;
        row = check(app, operation, fence).await?;
    }
    if !matches!(
        row.get::<_, String>("state").as_str(),
        "initializing" | "verifying"
    ) {
        return Err(Error::conflict("Unsupported bootstrap phase"));
    }
    if fresh {
        // This is the first writable start, after the durable checkpoint.
        start(app, &row, &artifact, fence, true, true).await?;
        let _ = peer(app, &row, &artifact, fence).await?;
        check(app, operation, fence).await?;
        // Even an RPC error can follow a committed write. Only Verify decides.
        let _ = initialize(app, &row).await;
        app.runtime.stop(row.get("instance_id")).await?;
    }
    // An ambiguous/restarted initialization NEVER receives another writable
    // start before Verify. Enforce Verify's no-new-writes contract in OCI too.
    start(app, &row, &artifact, fence, false, true).await?;
    let verifier = peer(app, &row, &artifact, fence).await?;
    check(app, operation, fence).await?;
    verifier
        .verify(
            &plan(&row)?,
            row.get::<_, Option<i64>>("volume_fence").unwrap() as u64,
        )
        .await?;
    if row.get::<_, String>("state") == "initializing" {
        checkpoint(app, &row, fence, "initializing", "verifying", None).await?;
        row = check(app, operation, fence).await?;
    }
    // Scratch cannot be reclaimed while any running mount namespace holds it.
    app.runtime.stop(row.get("instance_id")).await?;
    start(app, &row, &artifact, fence, true, false).await?;
    let _ = peer(app, &row, &artifact, fence).await?;
    check(app, operation, fence).await?;
    initialize(app, &row).await?;
    wait_health(app, &row, &artifact, fence).await?;
    check(app, operation, fence).await?;
    publish(app, &row, &artifact, fence).await
}

async fn fail_stopped(
    app: &App,
    operation: Uuid,
    fence: &db::DeploymentFence,
    error: Error,
) -> Result<()> {
    let row = job(app, operation).await?;
    if row.get::<_, String>("state") == "verified" {
        return Ok(());
    }
    let deployment: Uuid = row.get("deployment_id");
    if !fence.protects(deployment) {
        return Err(Error::unavailable("Bootstrap cleanup fence was lost"));
    }
    if app.db.connect().await?.query_opt("SELECT deployment_id FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2", &[&deployment,&operation]).await?.is_none() { return Ok(()); }
    let stopped = stop_all(app, &row).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    let current = tx.query_one("SELECT d.spec,h.operation_id FROM deployments d JOIN deployment_storage_holds h ON h.deployment_id=d.id WHERE d.id=$1 FOR UPDATE OF d,h", &[&deployment]).await?;
    if current.get::<_, Uuid>("operation_id") != operation {
        return Ok(());
    }
    let state: String = tx
        .query_one(
            "SELECT state FROM volume_schema_jobs WHERE operation_id=$1 FOR UPDATE",
            &[&operation],
        )
        .await?
        .get(0);
    if state == "verified" {
        return Ok(());
    }
    for instance in stopped {
        tx.execute("UPDATE instances SET phase='stopped',health='unknown',runtime_id=NULL,revision=revision+1,updated_at=now() WHERE id=$1", &[&instance]).await?;
        admission::release_stopped(&tx, instance).await?;
        volumes::release_stopped(&tx, Some(instance)).await?;
    }
    tx.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)", &[&deployment]).await?;
    if current.get::<_, Value>("spec") == row.get::<_, Value>("deployment_spec") {
        tx.execute("UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','false'),phase='disabled',revision=revision+1,updated_at=now() WHERE id=$1", &[&deployment]).await?;
    }
    // Partial business data and its initializing binding are retained. Only
    // the private scratch allocation is quarantined after all stop proofs.
    tx.execute("UPDATE volume_allocations SET state='quarantined',updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring' AND NOT EXISTS(SELECT 1 FROM volumes v WHERE v.id=volume_allocations.volume_id)", &[&row.get::<_, Uuid>("scratch_volume_id"),&operation]).await?;
    tx.execute("UPDATE volume_schema_jobs SET state='failed',failure=coalesce(failure,$2),updated_at=now() WHERE operation_id=$1", &[&operation,&error.detail()]).await?;
    db::complete(&tx, operation, Err(error)).await?;
    tx.execute(
        "DELETE FROM volume_holds WHERE operation_id=$1 AND kind='migration'",
        &[&operation],
    )
    .await?;
    tx.execute(
        "DELETE FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",
        &[&deployment, &operation],
    )
    .await?;
    if !fence.protects(deployment) || app.leader.is_closed() {
        return Err(Error::unavailable("Bootstrap cleanup leadership was lost"));
    }
    tx.commit().await?;
    Ok(())
}

pub async fn run(app: Arc<App>) {
    let mut active: HashMap<Uuid, tokio::sync::oneshot::Receiver<()>> = HashMap::new();
    loop {
        if app.shutdown.load(Ordering::SeqCst) || app.leader.is_closed() {
            break;
        }
        active.retain(|_, receiver| {
            matches!(
                receiver.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            )
        });
        if app.ready.load(Ordering::SeqCst) {
            if let Ok(c) = app.db.connect().await {
                if let Ok(rows) = c.query("SELECT operation_id,deployment_id FROM volume_schema_jobs j WHERE kind='bootstrap' AND (state NOT IN ('verified','failed') OR EXISTS(SELECT 1 FROM deployment_storage_holds h WHERE h.operation_id=j.operation_id)) ORDER BY updated_at,operation_id LIMIT 4", &[]).await {
                    for row in rows {
                        let operation: Uuid = row.get(0);
                        if active.len() >= 2 || active.contains_key(&operation) { continue; }
                        let deployment: Uuid = row.get(1);
                        let (sender,receiver) = tokio::sync::oneshot::channel();
                        active.insert(operation,receiver);
                        let owner = app.clone();
                        tokio::spawn(async move {
                            if let Ok(Some(fence)) = db::deployment_fence(&owner.db,deployment).await {
                                if let Err(error) = process(&owner,operation,&fence).await {
                                    if fail_stopped(&owner,operation,&fence,error).await.is_err() { tracing::error!(reason_code="SCHEMA_BOOTSTRAP_CLEANUP_PENDING"); }
                                }
                            }
                            let _ = sender.send(());
                        });
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
