//! Private-target Plan/Apply/Verify, owned by the recreate supervisor and its
//! deployment fence. No HTTP waiter or plugin result constructs UpgradeReady.
use super::*;

pub(crate) struct UpgradeReady {
    operation: Uuid,
    instance: Uuid,
    generation: i64,
    scratch: Uuid,
    proof: Value,
    health: rpc::HealthResponse,
}
impl UpgradeReady {
    pub(crate) fn health(&self) -> rpc::HealthResponse {
        self.health.clone()
    }
}

async fn backup_point(
    app: &App,
    row: &Row,
    fence: &db::DeploymentFence,
) -> Result<crate::storage::migration_backup::Point> {
    check(app, row.get("operation_id"), fence).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let ctx = revalidate(&tx, app, row).await?;
    let paths: Vec<policy::Path> = serde_json::from_value(row.get("authority"))?;
    tx.commit().await?;
    // No transaction spans object I/O. Every following phase rechecks current
    // original authority, including after the owned filesystem work finishes.
    upgrades::schema_registration(row)?
        .read(&c, app, &ctx, &paths)
        .await
}

async fn begin_write(
    app: &App,
    row: &Row,
    fence: &db::DeploymentFence,
    phase: &str,
    physical: &Value,
) -> Result<()> {
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    if !fence.protects(row.get("deployment_id")) {
        return Err(Error::unavailable("Format write fence is lost"));
    }
    let operation: Uuid = row.get("operation_id");
    let metadata: Value = row.get("installation_metadata");
    // The checkpoint precedes EVERY writable start, even if Apply is never
    // reached. A restarted applying job must Verify, never resend Apply.
    if tx.execute("UPDATE volume_schema_jobs SET state=$2,volume_plan=$3,updated_at=now() WHERE operation_id=$1 AND state='planned'", &[&operation,&phase,&physical]).await? != 1 {return Err(Error::conflict("Format write checkpoint changed"));}
    for volume in ids(row)? {
        if tx.execute("UPDATE volume_allocations SET schema_state='initializing',data_schema_version=$3,schema_deployment_id=$4,schema_plugin_id=$5,schema_publisher_id=$6,schema_manifest_digest=$7,schema_operation_id=$2,schema_proof=NULL,updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restored' AND NOT EXISTS(SELECT 1 FROM volumes WHERE id=$1)", &[&volume,&operation,&row.get::<_,String>("target_schema"),&row.get::<_,Uuid>("deployment_id"),&metadata["plugin_id"].as_str().unwrap(),&id(&metadata,"publisher_id")?,&metadata["manifest_digest"].as_str().unwrap()]).await? != 1 { return Err(Error::conflict("Format write target is not private")); }
    }
    tx.commit().await?;
    Ok(())
}

async fn confirmed(
    app: &App,
    row: &Row,
    artifact: &Artifact,
    fence: &db::DeploymentFence,
) -> Result<Value> {
    let operation: Uuid = row.get("operation_id");
    let planned = plan(row)?;
    let proof = json!({"kind":"data_migration_verify","operation_id":operation,"plan_id":planned.id,"plan_digest":planned.digest,"backup_id":row.get::<_,Option<Uuid>>("backup_id"),"backup_manifest_digest":upgrades::schema_registration(row)?.manifest_digest(),"volume_fence":row.get::<_,Option<i64>>("volume_fence"),"observed_schema":row.get::<_,String>("target_schema"),"instance_id":row.get::<_,Uuid>("instance_id"),"generation":row.get::<_,i64>("generation"),"manifest_digest":artifact.manifest_digest});
    if serde_json::to_vec(&proof)?.len() > 65536 {
        return Err(Error::exhausted());
    }
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    revalidate(&tx, app, row).await?;
    if !fence.protects(row.get("deployment_id")) {
        return Err(Error::unavailable("Format verification fence is lost"));
    }
    for volume in ids(row)? {
        if tx.execute("UPDATE volume_allocations SET schema_state='verified',schema_proof=$3,updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restored' AND schema_operation_id=$2 AND data_schema_version=$4 AND schema_manifest_digest=$5 AND NOT EXISTS(SELECT 1 FROM volumes WHERE id=$1)", &[&volume,&operation,&proof,&row.get::<_,String>("target_schema"),&artifact.manifest_digest]).await? != 1 { return Err(Error::conflict("Verified format target changed")); }
    }
    if tx.execute("UPDATE volume_schema_jobs SET state='verifying',updated_at=now() WHERE operation_id=$1 AND state IN ('applying','verifying')", &[&operation]).await? != 1 { return Err(Error::conflict("Format verification checkpoint changed")); }
    tx.commit().await?;
    Ok(proof)
}

pub(crate) async fn run_upgrade(
    app: &App,
    operation: Uuid,
    fence: &db::DeploymentFence,
) -> Result<UpgradeReady> {
    let mut row = check(app, operation, fence).await?;
    if !matches!(
        row.get::<_, String>("kind").as_str(),
        "upgrade" | "rollback"
    ) {
        return Err(Error::conflict("Format worker requires an upgrade parent"));
    }
    let artifact = upgrades::artifact(app, &row.get("installation_metadata")).await?;
    // Startup fencing can leave an old private candidate with either RO or RW
    // mounts. Prove it stopped before interpreting any durable checkpoint.
    app.runtime.stop(row.get("instance_id")).await?;
    if row.get::<_, String>("state") == "planning" {
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
    app.runtime.stop(row.get("instance_id")).await?;
    if row.get::<_, String>("state") == "planned" {
        let mut physical: Value = row.get("volume_plan");
        if row.get::<_, String>("kind") == "upgrade" {
            let point = backup_point(app, &row, fence).await?;
            let pool = crate::storage::backup_transfer::Pool::default();
            for item in physical.as_array_mut().unwrap() {
                let source =
                    volumes::private_restore_path(app, operation, id(item, "id")?, false).await?;
                check(app, operation, fence).await?;
                let verified = point
                    .verify_clone(
                        &pool,
                        source.spec["name"].as_str().unwrap(),
                        source.path,
                        crate::backups::master(&app.config)?,
                        remaining(&row, Duration::from_secs(3600))?,
                    )
                    .await?;
                item["backup_freshness"] = json!(verified);
            }
            begin_write(app, &row, fence, "applying", &physical).await?;
            row = check(app, operation, fence).await?;
            // Apply is dispatched ONLY in this fresh planned->applying branch.
            start(app, &row, &artifact, fence, true, true).await?;
            let peer = peer(app, &row, &artifact, fence).await?;
            check(app, operation, fence).await?;
            let _ = peer
                .apply(
                    &plan(&row)?,
                    upgrades::schema_registration(&row)?.backup(),
                    row.get::<_, Option<i64>>("volume_fence").unwrap() as u64,
                )
                .await;
            // Both success and unknown/error outcomes require actual RO Verify.
            app.runtime.stop(row.get("instance_id")).await?;
        } else {
            // restore_group already authenticated/read back the OLD target
            // bytes. Plan/Verify checks their business format; no inverse Apply.
            begin_write(app, &row, fence, "verifying", &physical).await?;
            row = check(app, operation, fence).await?;
        }
    }
    if !matches!(
        row.get::<_, String>("state").as_str(),
        "applying" | "verifying"
    ) {
        return Err(Error::conflict("Unsupported format verification phase"));
    }
    start(app, &row, &artifact, fence, false, true).await?;
    let verifier = peer(app, &row, &artifact, fence).await?;
    check(app, operation, fence).await?;
    verifier
        .verify(
            &plan(&row)?,
            row.get::<_, Option<i64>>("volume_fence").unwrap() as u64,
        )
        .await?;
    let proof = confirmed(app, &row, &artifact, fence).await?;
    row = check(app, operation, fence).await?;
    // Positive stop detaches scratch before the production RW candidate starts.
    app.runtime.stop(row.get("instance_id")).await?;
    start(app, &row, &artifact, fence, true, false).await?;
    let _ = peer(app, &row, &artifact, fence).await?;
    check(app, operation, fence).await?;
    initialize(app, &row).await?;
    let health = wait_health(app, &row, &artifact, fence).await?;
    let scratch =
        volumes::private_restore_path(app, operation, row.get("scratch_volume_id"), false).await?;
    if scratch.capacity_bytes != SCRATCH_BYTES {
        return Err(Error::conflict("Format scratch quota changed"));
    }
    check(app, operation, fence).await?;
    Ok(UpgradeReady {
        operation,
        instance: row.get("instance_id"),
        generation: row.get("generation"),
        scratch: scratch.id,
        proof,
        health,
    })
}

/// Must precede public volume publication, in the SAME authorization/budget/D
/// transaction. UpgradeReady can only come from actual Verify + detached scratch
/// + target activation/health, never a serialized plugin response or DB fixture.
pub(crate) async fn commit_upgrade(
    c: &(impl GenericClient + Sync),
    app: &App,
    parent: &Row,
    ready: Option<&UpgradeReady>,
    fence: &db::DeploymentFence,
) -> Result<()> {
    let operation: Uuid = parent.get("operation_id");
    let child = c.query_opt("SELECT j.*,host(j.peer) AS approved_peer FROM volume_schema_jobs j WHERE operation_id=$1 AND kind<>'bootstrap' FOR UPDATE", &[&operation]).await?;
    let Some(child) = child else {
        return if ready.is_none() {
            Ok(())
        } else {
            Err(Error::conflict("Unexpected format publication witness"))
        };
    };
    let ready =
        ready.ok_or_else(|| Error::conflict("Format publication requires actual verification"))?;
    revalidate(c, app, &child).await?;
    if child.get::<_, String>("state") != "verifying"
        || ready.operation != operation
        || ready.instance != child.get::<_, Uuid>("instance_id")
        || ready.generation != child.get::<_, i64>("generation")
        || ready.scratch != child.get::<_, Uuid>("scratch_volume_id")
        || !fence.protects(child.get("deployment_id"))
    {
        return Err(Error::conflict("Format publication witness differs"));
    }
    for volume in ids(&child)? {
        let allocation = c.query_one("SELECT schema_state,data_schema_version,schema_proof,schema_operation_id,schema_manifest_digest FROM volume_allocations WHERE volume_id=$1 FOR UPDATE", &[&volume]).await?;
        if allocation.get::<_, String>("schema_state") != "verified"
            || allocation.get::<_, Option<String>>("data_schema_version")
                != Some(child.get("target_schema"))
            || allocation.get::<_, Option<Uuid>>("schema_operation_id") != Some(operation)
            || allocation.get::<_, Option<Value>>("schema_proof").as_ref() != Some(&ready.proof)
            || allocation
                .get::<_, Option<String>>("schema_manifest_digest")
                .as_deref()
                != child.get::<_, Value>("installation_metadata")["manifest_digest"].as_str()
        {
            return Err(Error::conflict(
                "Private target lacks the original format proof",
            ));
        }
    }
    if c.execute("UPDATE volume_allocations SET state='quarantined',failure='{}',updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring' AND capacity_bytes=67108864", &[&ready.scratch,&operation]).await? != 1 {return Err(Error::conflict("Detached scratch allocation changed"));}
    c.execute("UPDATE volume_schema_jobs SET state='verified',updated_at=now() WHERE operation_id=$1 AND state='verifying'", &[&operation]).await?;
    Ok(())
}
