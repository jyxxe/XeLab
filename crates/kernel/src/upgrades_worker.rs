//! The daemon, not an HTTP waiter, owns every runtime mutation and stop proof.
use super::*;
use crate::{
    admission, controller,
    rpc::{self, plugin_lifecycle_client::PluginLifecycleClient},
    tls,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
    time::Duration,
};

async fn job(app: &App, operation: Uuid) -> Result<Row> {
    app.db.connect().await?.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM deployment_upgrade_jobs j WHERE operation_id=$1",&[&operation]).await.map_err(Into::into)
}
async fn check(app: &App, operation: Uuid) -> Result<Row> {
    let row = job(app, operation).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, &row).await?;
    tx.commit().await?;
    Ok(row)
}
fn context(row: &Row, phase: &str) -> rpc::RpcContext {
    let mut ctx = controller::context(Some(row.get("subject_id")));
    ctx.idempotency_key = format!("{}:{phase}", row.get::<_, Uuid>("operation_id"));
    ctx.deadline_unix_ms = ctx.deadline_unix_ms.min(
        row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            .timestamp_millis(),
    );
    ctx
}
async fn old_instances(app: &App, row: &Row) -> Result<Vec<(Uuid, i64)>> {
    let mut ids = BTreeMap::new();
    for item in row
        .get::<_, Value>("instances")
        .as_array()
        .ok_or_else(|| Error::conflict("Invalid upgrade instance journal"))?
    {
        let generation = item["generation"]
            .as_i64()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::conflict("Invalid upgrade generation"))?;
        ids.insert(id(item, "id")?, generation);
    }
    let c = app.db.connect().await?;
    for instance in c.query("SELECT i.id,i.generation FROM instances i WHERE i.deployment_id=$1 AND i.id<>$2 AND (i.phase<>'stopped' OR EXISTS(SELECT 1 FROM volume_attachments a WHERE a.instance_id=i.id)) LIMIT 33",&[&row.get::<_,Uuid>("deployment_id"),&row.get::<_,Uuid>("candidate_id")]).await? {let id:Uuid=instance.get(0);let generation:i64=instance.get(1);if ids.insert(id,generation).is_some_and(|old|old!=generation){return Err(Error::conflict("Upgrade instance generation changed"));}}
    if ids.len() > 32 {
        return Err(Error::exhausted());
    }
    Ok(ids.into_iter().collect())
}
async fn freeze(app: &App, row: &Row) -> Result<()> {
    let operation: Uuid = row.get("operation_id");
    // A controller tick can have passed its hold check just before admission.
    // We now own its deployment fence: withdraw again before draining, after
    // every old tick has finished. Its phase-only revision is not a new intent.
    let mut database = app.db.connect().await?;
    let tx = database.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    tx.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&row.get::<_,Uuid>("deployment_id")]).await?;
    tx.execute("UPDATE deployments SET phase='draining',revision=revision+CASE WHEN phase<>'draining' THEN 1 ELSE 0 END,updated_at=now() WHERE id=$1",&[&row.get::<_,Uuid>("deployment_id")]).await?;
    tx.commit().await?;
    let instances = old_instances(app, row).await?;
    let c = app.db.connect().await?;
    c.execute("UPDATE deployment_upgrade_jobs SET instances=$2,updated_at=now() WHERE operation_id=$1 AND state='freezing'",&[&operation,&json!(instances.iter().map(|(id,generation)|json!({"id":id,"generation":generation})).collect::<Vec<_>>())]).await?;
    let mut outcome = Ok(());
    for (instance, generation) in &instances {
        let drained: Result<()> = async {
            if !app.runtime.running(*instance).await? {
                return Ok(());
            }
            let channel = tls::channel(
                &app.config,
                *instance,
                *generation,
                &app.runtime.address(*instance)?,
            )
            .await?;
            let mut client = PluginLifecycleClient::new(channel);
            let mut request = tonic::Request::new(rpc::DrainRequest {
                context: Some(context(row, "drain")),
                grace_ms: 30000,
            });
            request.set_timeout(Duration::from_secs(30));
            let result = client.drain(request).await?.into_inner();
            if result.accepting_requests || result.active_calls != 0 || !result.drained {
                return Err(Error::conflict("Upgrade source did not drain"));
            }
            Ok(())
        }
        .await;
        if drained.is_err() {
            outcome = drained;
        }
    }
    for (instance, _) in &instances {
        if let Err(error) = app.runtime.stop(*instance).await {
            outcome = Err(error);
        }
    }
    outcome?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    for (instance, _) in &instances {
        tx.execute("UPDATE instances SET phase='stopped',health='unknown',runtime_id=NULL,revision=revision+1,updated_at=now() WHERE id=$1 AND deployment_id=$2",&[instance,&row.get::<_,Uuid>("deployment_id")]).await?;
        admission::release_stopped(&tx, *instance).await?;
        crate::volumes::release_stopped(&tx, Some(*instance)).await?;
    }
    volume_group::freeze(&tx, operation).await?;
    if row.get::<_, Value>("target_spec")["enabled"] == true {
        // Transfer the released source budget while the same budget lock is
        // held. Another deployment cannot consume it between stop and reserve.
        admission::reserve(&tx, row.get("candidate_id")).await?;
    }
    tx.execute("UPDATE deployment_upgrade_jobs SET state='stopped',updated_at=now() WHERE operation_id=$1 AND state='freezing'",&[&operation]).await?;
    tx.execute("UPDATE operations SET state='running',revision=revision+1,updated_at=now() WHERE id=$1 AND state='pending'",&[&operation]).await?;
    tx.commit().await?;
    Ok(())
}
async fn artifact(app: &App, row: &Row) -> Result<Artifact> {
    let target: Value = row.get("target_metadata");
    let digest = target["image_digest"]
        .as_str()
        .ok_or_else(|| Error::conflict("Missing upgrade image digest"))?;
    let hash = digest
        .strip_prefix("sha256:")
        .filter(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .ok_or_else(|| Error::conflict("Invalid upgrade image digest"))?;
    let image = crate::config::read_private(
        &app.config
            .root()
            .join("images")
            .join(hash)
            .with_extension("reference"),
        4096,
    )?;
    let image = std::str::from_utf8(&image)
        .map_err(|_| Error::conflict("Invalid upgrade image reference"))?;
    if artifact::image_ref(image)? != digest {
        return Err(Error::conflict("Upgrade image reference differs"));
    }
    // Installation already verified the publisher signature. Recheck exactly
    // that admitted immutable content; do not invent a new signature/approval.
    app.runtime.verify_image(image, digest).await?;
    let root = app.runtime.mount_image(image, digest).await?;
    let raw = artifact::read_image_file(&root, "xelab/plugin.json", 1048576)?;
    let manifest = crate::contracts::strict_json(&raw, 1048576)?;
    let manifest_digest = crate::contracts::digest(&raw);
    if manifest_digest != target["manifest_digest"].as_str().unwrap_or("")
        || manifest != target["manifest"]
    {
        return Err(Error::conflict(
            "Actual upgrade artifact differs from admitted target",
        ));
    }
    artifact::validate_profile(&manifest)?;
    Ok(Artifact {
        image: image.to_owned(),
        image_digest: digest.to_owned(),
        manifest_digest,
        manifest,
        raw,
    })
}
async fn probe(app: &App, row: &Row, artifact: &Artifact) -> Result<Option<rpc::HealthResponse>> {
    if row.get::<_, Value>("target_spec")["enabled"] != true {
        return Ok(None);
    }
    let operation: Uuid = row.get("operation_id");
    let candidate: Uuid = row.get("candidate_id");
    let deployment: Uuid = row.get("deployment_id");
    let generation: i64 = row.get("candidate_generation");
    let spec: Value = row.get("target_spec");
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    let approval_revision: i64 = tx
        .query_one(
            "SELECT revision+1 FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await?
        .get(0);
    admission::reserve(&tx, candidate).await?;
    tx.execute("UPDATE deployment_upgrade_jobs SET state='probing',updated_at=now() WHERE operation_id=$1 AND state IN ('stopped','probing')",&[&operation]).await?;
    tx.execute(
        "UPDATE instances SET phase='starting',revision=revision+1,updated_at=now() WHERE id=$1",
        &[&candidate],
    )
    .await?;
    tx.commit().await?;
    let mounts = volume_group::mounts(app, row).await?;
    let observed = app
        .runtime
        .ensure(
            candidate,
            deployment,
            generation,
            &artifact.manifest,
            &artifact.image,
            &artifact.image_digest,
            &spec["limits"],
            &mounts,
        )
        .await?;
    if !observed.running {
        return Err(Error::unavailable("Upgrade candidate is not running"));
    }
    let end = tokio::time::Instant::now()
        + Duration::from_millis(
            artifact.manifest["health"]["startup_timeout_ms"]
                .as_u64()
                .unwrap(),
        );
    loop {
        check(app, operation).await?;
        let attempt = async {
            let channel =
                tls::channel(&app.config, candidate, generation, &observed.address).await?;
            let mut client = PluginLifecycleClient::new(channel);
            let description = client
                .describe(rpc::DescribeRequest {
                    context: Some(context(row, "describe")),
                })
                .await?
                .into_inner();
            if description.manifest_digest != artifact.manifest_digest
                || description.manifest_json != artifact.raw
            {
                return Err(Error::conflict(
                    "Upgrade Describe differs from signed bytes",
                ));
            }
            let initialized = client
                .initialize(rpc::InitializeRequest {
                    context: Some(context(row, "initialize")),
                    deployment_id: deployment.to_string(),
                    instance_id: candidate.to_string(),
                    generation: generation as u64,
                    config_json: serde_json::to_vec(&spec["config"])?,
                    config_digest: crate::contracts::digest(&serde_json::to_vec(&spec["config"])?),
                    approval_revision: approval_revision as u64,
                })
                .await?
                .into_inner();
            if !initialized.initialized || initialized.observed_generation != generation as u64 {
                return Err(Error::conflict("Upgrade Initialize identity differs"));
            }
            let mut request = tonic::Request::new(rpc::HealthRequest {
                context: Some(context(row, "health")),
            });
            request.set_timeout(Duration::from_millis(
                artifact.manifest["health"]["timeout_ms"].as_u64().unwrap(),
            ));
            let health = client.get_health(request).await?.into_inner();
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
            for h in &health.capabilities {
                let Some(contract) = &h.contract else {
                    return Err(Error::conflict("Upgrade health contract is missing"));
                };
                if !h.ready
                    || !actual.insert((
                        contract.name.clone(),
                        contract.version.clone(),
                        contract.schema_digest.clone(),
                    ))
                {
                    return Err(Error::unavailable("Upgrade capability is not ready"));
                }
            }
            if !health.alive
                || health.observed_generation != generation as u64
                || actual != expected
            {
                return Err(Error::unavailable(
                    "Upgrade health does not match the target",
                ));
            }
            Ok(health)
        };
        let startup_left = end
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| Error::unavailable("Upgrade startup deadline expired"))?;
        let job_left = (row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            - chrono::Utc::now())
        .to_std()
        .map_err(|_| Error::conflict("Upgrade deadline expired"))?;
        let attempt: Result<rpc::HealthResponse> =
            tokio::time::timeout(startup_left.min(job_left), attempt)
                .await
                .map_err(|_| Error::unavailable("Upgrade startup deadline expired"))?;
        match attempt {
            Ok(health) => return Ok(Some(health)),
            Err(error) => {
                if tokio::time::Instant::now() >= end {
                    return Err(error);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn publish(app: &App, row: &Row, health: Option<rpc::HealthResponse>) -> Result<()> {
    let enabled = row.get::<_, Value>("target_spec")["enabled"] == true;
    let candidate: Uuid = row.get("candidate_id");
    if enabled && (health.is_none() || !app.runtime.running(candidate).await?) {
        return Err(Error::unavailable("Upgrade candidate is no longer healthy"));
    }
    if !enabled {
        app.runtime.stop(candidate).await?;
    }
    let operation: Uuid = row.get("operation_id");
    let deployment: Uuid = row.get("deployment_id");
    let generation: i64 = row.get("candidate_generation");
    let publication = volume_group::prepare_publication(app, row).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let ctx = revalidate(&tx, app, row).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    tx.query_one(
        "SELECT id FROM deployments WHERE id=$1 FOR UPDATE",
        &[&deployment],
    )
    .await?;
    let current: String = tx
        .query_one(
            "SELECT state FROM deployment_upgrade_jobs WHERE operation_id=$1 FOR UPDATE",
            &[&operation],
        )
        .await?
        .get(0);
    if !matches!(current.as_str(), "stopped" | "probing") {
        return Err(Error::conflict("Upgrade publication phase changed"));
    }
    volume_group::publish(&tx, &ctx, row, publication).await?;
    if enabled {
        if tx.query_opt("SELECT instance_id FROM reservations WHERE instance_id=$1 AND status IN ('reserved','active')",&[&candidate]).await?.is_none(){return Err(Error::conflict("Upgrade candidate lacks its resource reservation"));}
        let manifest = row.get::<_, Value>("target_metadata")["manifest"].clone();
        for cap in manifest["capabilities"].as_array().unwrap() {
            let contract = &cap["contract"];
            let name = contract["name"].as_str().unwrap();
            let version = contract["version"].as_str().unwrap();
            tx.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',$4,now()) ON CONFLICT(instance_id,name,version) DO UPDATE SET availability='ready',observed_generation=excluded.observed_generation,observed_at=now()",&[&candidate,&name,&version,&generation]).await?;
        }
    }
    // A version change never upgrades historical workload proxy/Secret approval.
    // Reducing an already enabled deployment to empty rules grants no authority;
    // a disabled target remains unapproved until an explicit later review.
    let approval = if enabled {
        json!({"rules":[],"egress":[]})
    } else {
        json!({})
    };
    tx.execute(
        "DELETE FROM host_proxy_approvals WHERE deployment_id=$1",
        &[&deployment],
    )
    .await?;
    tx.execute("UPDATE secret_leases SET expires_at=least(expires_at,now()) WHERE binding_id IN(SELECT id FROM secret_bindings WHERE deployment_id=$1)",&[&deployment]).await?;
    tx.execute("UPDATE secret_bindings SET status='revoked',revision=revision+1,updated_at=now() WHERE deployment_id=$1 AND status='active'",&[&deployment]).await?;
    let phase = if enabled { "ready" } else { "disabled" };
    let instance_phase = if enabled { "ready" } else { "stopped" };
    let runtime = enabled.then(|| candidate.to_string());
    tx.execute("UPDATE instances SET phase=$2,runtime_id=$3,health=$4,last_heartbeat_at=now(),revision=revision+1,updated_at=now() WHERE id=$1",&[&candidate,&instance_phase,&runtime,&if enabled{"alive"}else{"unknown"}]).await?;
    tx.execute("UPDATE deployments SET installation_id=$2,spec=$3,approval=$4,observed_generation=$5,phase=$6,conditions='[]',revision=revision+1,updated_at=now() WHERE id=$1",&[&deployment,&row.get::<_,Uuid>("target_installation_id"),&row.get::<_,Value>("target_spec"),&approval,&generation,&phase]).await?;
    tx.execute("UPDATE deployment_upgrade_jobs SET state='succeeded',updated_at=now() WHERE operation_id=$1",&[&operation]).await?;
    tx.execute(
        "DELETE FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",
        &[&deployment, &operation],
    )
    .await?;
    let result = api::read_object(&tx, "deployments", deployment, "Deployment").await?;
    db::complete(&tx, operation, Ok(result)).await?;
    db::audit(
        &tx,
        ctx.subject,
        Some(ctx.credential),
        operation,
        &row.get::<_, String>("action"),
        &Resource::new("deployment", deployment, Some(ctx.subject)).json(),
        "succeeded",
        "UPGRADE_VERIFIED",
    )
    .await?;
    if app.leader.is_closed() {
        return Err(Error::unavailable("Upgrade leadership is lost"));
    }
    tx.commit().await?;
    Ok(())
}
async fn process(app: &App, operation: Uuid, fence: &db::DeploymentFence) -> Result<()> {
    let row = job(app, operation).await?;
    if !fence.protects(row.get("deployment_id")) {
        return Err(Error::conflict("Upgrade runtime fence differs"));
    }
    if row.get::<_, String>("state") == "succeeded" {
        return Ok(());
    }
    let row = check(app, operation).await?;
    if row.get::<_, String>("state") == "freezing" {
        freeze(app, &row).await?;
    }
    let row = check(app, operation).await?;
    if !matches!(
        row.get::<_, String>("state").as_str(),
        "stopped" | "probing"
    ) {
        return Err(Error::conflict("Unsupported upgrade execution phase"));
    }
    let artifact = artifact(app, &row).await?;
    volume_group::clone_group(app, &row).await?;
    check(app, operation).await?;
    let health = probe(app, &row, &artifact).await?;
    if !fence.protects(row.get("deployment_id")) {
        return Err(Error::conflict("Upgrade runtime fence was lost"));
    }
    let row = check(app, operation).await?;
    publish(app, &row, health).await
}
pub(super) enum SourceRecovery {
    NotRequired,
    Created(Uuid),
    Blocked,
}

/// Metadata only. The caller owns the deployment fence, authorization/budget
/// transaction, and positive stop proofs for every old/candidate process.
pub(super) async fn recover_source(
    c: &(impl GenericClient + Sync),
    app: &App,
    row: &Row,
) -> Result<SourceRecovery> {
    let deployment: Uuid = row.get("deployment_id");
    let original: Value = row.get("original_spec");
    let current=c.query_one("SELECT owner_id,spec,installation_id,archived_at FROM deployments WHERE id=$1 FOR UPDATE",&[&deployment]).await?;
    if current.get::<_, Value>("spec") != original
        || current.get::<_, Uuid>("installation_id") != row.get::<_, Uuid>("source_installation_id")
        || current.get::<_, Uuid>("owner_id") != row.get::<_, Uuid>("subject_id")
        || current
            .get::<_, Option<chrono::DateTime<chrono::Utc>>>("archived_at")
            .is_some()
    {
        return Ok(SourceRecovery::NotRequired);
    }
    if original["enabled"] != true {
        c.execute("UPDATE deployments SET phase='disabled',revision=revision+1,updated_at=now() WHERE id=$1",&[&deployment]).await?;
        return Ok(SourceRecovery::NotRequired);
    }
    let permitted: Result<Context> = async {
        let credential: Uuid = row.get("credential_id");
        let peer = row
            .get::<_, String>("approved_peer")
            .parse()
            .map_err(|_| Error::denied())?;
        let ctx = app
            .auth
            .resume(c, credential, peer, row.get("operation_id"))
            .await?;
        if ctx.subject != row.get::<_, Uuid>("subject_id") {
            return Err(Error::denied());
        }
        let source = installation(c, row.get("source_installation_id")).await?;
        if source != row.get::<_, Value>("source_metadata") {
            return Err(Error::conflict("Original installation changed"));
        }
        let saved: Vec<policy::Path> =
            serde_json::from_value(row.get("authority")).map_err(|_| Error::denied())?;
        let available = policy::rules(&Policy::new(c, peer).revalidate(&saved, credential).await?);
        for action in ["plugin.read", "plugin.enable"] {
            if !auth::rules_allow(
                &available,
                action,
                &Resource::new("deployment", deployment, Some(ctx.subject)),
                peer,
            ) {
                return Err(Error::denied());
            }
        }
        if !auth::rules_allow(
            &available,
            "plugin.read",
            &Resource::new("plugin", source["plugin_id"].as_str().unwrap(), None),
            peer,
        ) {
            return Err(Error::denied());
        }
        for volume in original["volume_ids"]
            .as_array()
            .ok_or_else(|| Error::conflict("Invalid original volume group"))?
        {
            let volume: Uuid = volume
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| Error::conflict("Invalid original volume identity"))?;
            if !auth::rules_allow(
                &available,
                "volume.read",
                &Resource::new("volume", volume, Some(ctx.subject)),
                peer,
            ) {
                return Err(Error::denied());
            }
        }
        crate::volumes::validate_selection(c, &ctx, &original, &source["manifest"]).await?;
        Ok(ctx)
    }
    .await;
    if let Ok(ctx) = permitted {
        let last: i64 = c
            .query_one(
                "SELECT coalesce(max(generation),0) FROM instances WHERE deployment_id=$1",
                &[&deployment],
            )
            .await?
            .get(0);
        if let Some(generation) = last
            .checked_add(1)
            .filter(|g| (1..=9_007_199_254_740_991).contains(g))
        {
            c.batch_execute("SAVEPOINT upgrade_source_recovery").await?;
            let instance = Uuid::new_v4();
            let admitted:Result<()>=async{
                c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,$3,0,'pending','unknown',$4,now(),now())",&[&instance,&deployment,&generation,&original["limits"]]).await?;
                admission::reserve(c,instance).await?;
                crate::volumes::approve_mounts(c, &ctx, deployment, generation, &original, &row.get::<_,Value>("source_metadata")["manifest"]).await?;
                Ok(())
            }.await;
            if admitted.is_ok() {
                c.batch_execute("RELEASE SAVEPOINT upgrade_source_recovery")
                    .await?;
                c.execute("UPDATE deployments SET phase='reconciling',revision=revision+1,updated_at=now() WHERE id=$1",&[&deployment]).await?;
                return Ok(SourceRecovery::Created(instance));
            }
            c.batch_execute("ROLLBACK TO SAVEPOINT upgrade_source_recovery; RELEASE SAVEPOINT upgrade_source_recovery").await?;
        }
    }
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','false'),phase='disabled',revision=revision+1,updated_at=now() WHERE id=$1",&[&deployment]).await?;
    Ok(SourceRecovery::Blocked)
}

async fn fail_stopped(
    app: &App,
    operation: Uuid,
    fence: &db::DeploymentFence,
    error: Error,
) -> Result<()> {
    let row = job(app, operation).await?;
    if row.get::<_, String>("state") == "succeeded" {
        return Ok(());
    }
    let deployment: Uuid = row.get("deployment_id");
    if !fence.protects(deployment) {
        return Err(Error::conflict("Upgrade cleanup fence differs"));
    }
    if app.db.connect().await?.query_opt("SELECT deployment_id FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",&[&deployment,&operation]).await?.is_none(){return Ok(());}
    let mut instances = old_instances(app, &row).await?;
    instances.push((row.get("candidate_id"), row.get("candidate_generation")));
    let mut stopped = Ok(());
    for (instance, _) in &instances {
        if let Err(error) = app.runtime.stop(*instance).await {
            stopped = Err(error);
        }
    }
    stopped?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    tx.query_one("SELECT id FROM node_budget FOR UPDATE", &[])
        .await?;
    let state: String = tx
        .query_one(
            "SELECT state FROM deployment_upgrade_jobs WHERE operation_id=$1 FOR UPDATE",
            &[&operation],
        )
        .await?
        .get(0);
    if state == "succeeded" {
        return Ok(());
    }
    for (instance, _) in &instances {
        tx.execute("UPDATE instances SET phase='stopped',runtime_id=NULL,health='unknown',revision=revision+1,updated_at=now() WHERE id=$1 AND deployment_id=$2",&[instance,&deployment]).await?;
        admission::release_stopped(&tx, *instance).await?;
        crate::volumes::release_stopped(&tx, Some(*instance)).await?;
    }
    volume_group::failed(&tx, operation).await?;
    let recovery = recover_source(&tx, app, &row).await?;
    let (recovery_state, instance) = match recovery {
        SourceRecovery::NotRequired => ("not_required", None),
        SourceRecovery::Created(id) => ("intent_created", Some(id)),
        SourceRecovery::Blocked => ("blocked", None),
    };
    tx.execute("UPDATE deployment_upgrade_jobs SET state='failed',failure=coalesce(failure,$2),source_recovery=$3,recovery_instance_id=$4,updated_at=now() WHERE operation_id=$1",&[&operation,&error.detail(),&recovery_state,&instance]).await?;
    let report = if recovery_state == "blocked" {
        Error::conflict("Upgrade failed; the original deployment remains disabled")
            .with_resource(deployment)
    } else {
        error
    };
    db::complete(&tx, operation, Err(report)).await?;
    tx.execute(
        "DELETE FROM deployment_storage_holds WHERE deployment_id=$1 AND operation_id=$2",
        &[&deployment, &operation],
    )
    .await?;
    if app.leader.is_closed() {
        return Err(Error::unavailable("Upgrade cleanup leadership is lost"));
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
                if let Ok(rows)=c.query("SELECT operation_id,deployment_id FROM deployment_upgrade_jobs j WHERE state NOT IN ('succeeded','failed') OR EXISTS(SELECT 1 FROM deployment_storage_holds h WHERE h.operation_id=j.operation_id) ORDER BY updated_at,operation_id LIMIT 4",&[]).await{
            for row in rows {let operation:Uuid=row.get(0);if active.len()>=2||active.contains_key(&operation){continue;}let deployment:Uuid=row.get(1);let (sender,receiver)=tokio::sync::oneshot::channel();active.insert(operation,receiver);let owner=app.clone();
                tokio::spawn(async move{if let Ok(Some(fence))=db::deployment_fence(&owner.db,deployment).await{if let Err(error)=process(&owner,operation,&fence).await{if fail_stopped(&owner,operation,&fence,error).await.is_err(){tracing::error!(reason_code="UPGRADE_CLEANUP_PENDING");}}}let _=sender.send(());});
            }
        }
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
