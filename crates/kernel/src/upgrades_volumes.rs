//! Admission and private physical mappings for same-schema volume upgrades.
use super::*;
use crate::{storage::volume_clone, volumes};
use std::{collections::BTreeSet, time::Duration};

pub(super) struct Item {
    pub source: Uuid,
    pub target: Uuid,
    pub spec: Value,
    pub schema: Option<String>,
    pub binding: Option<Value>,
}
pub(super) struct Admission {
    pub items: Vec<Item>,
    pub reservation: volumes::RestorePlan,
}

/// Geometry/binding can use a short independent transaction, so this MUST run
/// before the API locks node_budget/deployment rows.
pub(super) async fn plan(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    original: &Value,
    target: &Value,
) -> Result<Option<Admission>> {
    let selected = ids(&original["volume_ids"])?;
    volumes::validate_selection(c, ctx, original, &target["manifest"]).await?;
    if selected.is_empty() {
        return Ok(None);
    }
    if selected.len() > 16 {
        return Err(Error::exhausted());
    }
    auth::authorize(
        c,
        ctx,
        "volume.create",
        &Resource::new("volume", "new", Some(ctx.subject)),
    )
    .await?;
    let mut seen: BTreeSet<_> = selected.iter().copied().collect();
    let mut items = Vec::new();
    let mut reserved = Vec::new();
    for source in selected {
        let row=c.query_one("SELECT v.owner_id,v.spec,v.capacity_bytes,a.spec AS allocation_spec,a.owner_id AS allocation_owner,a.capacity_bytes AS allocation_capacity,a.data_schema_version,a.state AS allocation_state FROM volumes v JOIN volume_allocations a ON a.volume_id=v.id WHERE v.id=$1",&[&source]).await?;
        if row.get::<_, Uuid>("owner_id") != ctx.subject
            || row.get::<_, Uuid>("allocation_owner") != ctx.subject
            || row.get::<_, Value>("allocation_spec") != row.get::<_, Value>("spec")
            || row.get::<_, i64>("allocation_capacity") != row.get::<_, i64>("capacity_bytes")
            || row.get::<_, Value>("spec")["capacity_bytes"].as_i64()
                != Some(row.get("capacity_bytes"))
            || row.get::<_, String>("allocation_state") != "ready"
            || row.get::<_, i64>("capacity_bytes") > 63 * 1024 * 1024 * 1024
        {
            return Err(Error::conflict(
                "Source volume schema, ownership or capacity differs",
            ));
        }
        let fresh = Uuid::new_v4();
        if !seen.insert(fresh) {
            return Err(Error::conflict("Private clone identity collision"));
        }
        auth::authorize(
            c,
            ctx,
            "volume.read",
            &Resource::new("volume", fresh, Some(ctx.subject)),
        )
        .await?;
        let spec: Value = row.get("spec");
        let schema: Option<String> = row.get("data_schema_version");
        let binding = schema_binding(c, source, &target["manifest"]).await?;
        reserved.push((fresh, spec.clone(), schema.clone()));
        items.push(Item {
            source,
            target: fresh,
            spec,
            schema,
            binding,
        });
    }
    Ok(Some(Admission {
        items,
        reservation: volumes::plan_restore(app, reserved).await?,
    }))
}
pub(super) async fn reserve(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    operation: Uuid,
    admission: Admission,
) -> Result<()> {
    volumes::reserve_restore(c, ctx, operation, admission.reservation).await?;
    let source_manifest:Value = c.query_one("SELECT manifest FROM installations WHERE id=(SELECT installation_id FROM deployments WHERE id=$1)", &[&deployment]).await?.get(0);
    for (ordinal, item) in admission.items.into_iter().enumerate() {
        let row=c.query_one("SELECT v.owner_id,v.spec,v.state AS public_state,a.state,a.spec AS allocation_spec,a.owner_id AS allocation_owner,a.data_schema_version FROM volumes v JOIN volume_allocations a ON a.volume_id=v.id WHERE v.id=$1 FOR UPDATE OF v,a",&[&item.source]).await?;
        if row.get::<_, Uuid>("owner_id") != ctx.subject
            || row.get::<_, Uuid>("allocation_owner") != ctx.subject
            || row.get::<_, Value>("spec") != item.spec
            || row.get::<_, Value>("allocation_spec") != item.spec
            || matches!(
                row.get::<_, String>("public_state").as_str(),
                "deleted" | "deleting"
            )
            || row.get::<_, String>("state") != "ready"
            || row.get::<_, Option<String>>("data_schema_version") != item.schema
            || schema_binding(c, item.source, &source_manifest).await? != item.binding
        {
            return Err(Error::conflict(
                "Source volume changed during upgrade admission",
            ));
        }
        if c.query_opt("SELECT a.instance_id FROM volume_attachments a JOIN instances i ON i.id=a.instance_id WHERE a.volume_id=$1 AND i.deployment_id<>$2 LIMIT 1",&[&item.source,&deployment]).await?.is_some(){return Err(Error::conflict("Source volume has attachments outside this deployment"));}
        c.execute("INSERT INTO volume_holds(volume_id,operation_id,kind) VALUES($1,$2,'migration') ON CONFLICT DO NOTHING",&[&item.source,&operation]).await?;
        if c.query_one(
            "SELECT operation_id FROM volume_holds WHERE volume_id=$1",
            &[&item.source],
        )
        .await?
        .get::<_, Uuid>(0)
            != operation
        {
            return Err(Error::conflict(
                "Source volume is held by another operation",
            ));
        }
        c.execute("INSERT INTO deployment_upgrade_volumes(operation_id,ordinal,source_volume_id,target_volume_id,source_spec,data_schema_version,state) VALUES($1,$2,$3,$4,$5,$6,'reserved')",&[&operation,&(ordinal as i32),&item.source,&item.target,&item.spec,&item.schema]).await?;
    }
    Ok(())
}
pub(super) async fn authorize_group(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    selected: &Value,
) -> Result<()> {
    let ids = ids(selected)?;
    if !ids.is_empty() {
        auth::authorize(
            c,
            ctx,
            "volume.create",
            &Resource::new("volume", "new", Some(ctx.subject)),
        )
        .await?;
    }
    for volume in ids {
        let owner: Uuid = c
            .query_opt("SELECT owner_id FROM volumes WHERE id=$1", &[&volume])
            .await?
            .ok_or_else(Error::not_found)?
            .get(0);
        if owner != ctx.subject {
            return Err(Error::denied());
        }
        auth::authorize(
            c,
            ctx,
            "volume.read",
            &Resource::new("volume", volume, Some(owner)),
        )
        .await?;
    }
    Ok(())
}
pub(super) fn clone_plan(volume: Uuid, capacity: u64) -> volume_clone::Plan {
    volume_clone::Plan {
        volume,
        maximum_data: capacity,
        maximum_archive: capacity + 128 * 1024 * 1024,
    }
}

fn ids(value: &Value) -> Result<Vec<Uuid>> {
    let selected = value
        .as_array()
        .filter(|v| v.len() <= 16)
        .ok_or_else(|| Error::conflict("Invalid upgrade volume group"))?;
    let mut seen = BTreeSet::new();
    selected
        .iter()
        .map(|v| {
            v.as_str()
                .and_then(|s| s.parse::<Uuid>().ok())
                .filter(|id| !id.is_nil() && seen.insert(*id))
                .ok_or_else(|| Error::conflict("Invalid or duplicate upgrade volume identity"))
        })
        .collect()
}
pub(super) async fn mounts(app: &App, job: &Row) -> Result<Vec<volumes::Mount>> {
    let operation: Uuid = job.get("operation_id");
    let manifest = job.get::<_, Value>("target_metadata")["manifest"].clone();
    let rows = mappings(&app.db.connect().await?, operation).await?;
    let mut mounts = Vec::new();
    for mapping in rows {
        if mapping.get::<_, String>("state") != "cloned" {
            return Err(Error::conflict("Candidate volume lacks verified clone"));
        }
        let target: Uuid = mapping.get("target_volume_id");
        let private = volumes::private_restore_path(app, operation, target, false).await?;
        let request = manifest["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == private.spec["name"])
            .ok_or_else(|| Error::conflict("Candidate alias changed"))?;
        mounts.push(volumes::Mount {
            volume: target,
            source: private.path,
            destination: request["mount_path"].as_str().unwrap().into(),
            access: request["access"].as_str().unwrap().into(),
            fence: mapping
                .get::<_, Option<i64>>("target_fence")
                .ok_or_else(|| Error::conflict("Candidate fence missing"))?,
        });
    }
    Ok(mounts)
}
pub(super) async fn prepare_publication(
    app: &App,
    job: &Row,
) -> Result<Option<volumes::RestorePublication>> {
    let selected = ids(&job.get::<_, Value>("target_spec")["volume_ids"])?;
    if selected.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        volumes::prepare_restore_publication(app, job.get("operation_id"), &selected).await?,
    ))
}
/// Called in the same transaction as version/provider/Operation publication.
pub(super) async fn publish(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    job: &Row,
    publication: Option<volumes::RestorePublication>,
) -> Result<()> {
    let operation: Uuid = job.get("operation_id");
    let rows = mappings(c, operation).await?;
    if rows.is_empty() {
        if publication.is_some() {
            return Err(Error::conflict("Unexpected clone publication"));
        }
        return Ok(());
    }
    volumes::publish_restored(
        c,
        publication.ok_or_else(|| Error::conflict("Missing clone quota proof"))?,
    )
    .await?;
    let target_spec: Value = job.get("target_spec");
    let manifest = job.get::<_, Value>("target_metadata")["manifest"].clone();
    let enabled = target_spec["enabled"] == true;
    let candidate: Uuid = job.get("candidate_id");
    let generation: i64 = job.get("candidate_generation");
    let deployment: Uuid = job.get("deployment_id");
    for mapping in rows {
        let source: Uuid = mapping.get("source_volume_id");
        let target: Uuid = mapping.get("target_volume_id");
        let original = c.query_one("SELECT v.write_fence,v.writer_instance_id,v.spec,h.operation_id FROM volumes v JOIN volume_holds h ON h.volume_id=v.id WHERE v.id=$1 FOR UPDATE OF v", &[&source]).await?;
        if mapping.get::<_, String>("state") != "cloned"
            || original.get::<_, Uuid>("operation_id") != operation
            || Some(original.get::<_, i64>("write_fence"))
                != mapping.get::<_, Option<i64>>("source_fence")
            || original
                .get::<_, Option<Uuid>>("writer_instance_id")
                .is_some()
            || original.get::<_, Value>("spec") != mapping.get::<_, Value>("source_spec")
            || c.query_opt(
                "SELECT instance_id FROM volume_attachments WHERE volume_id=$1 LIMIT 1",
                &[&source],
            )
            .await?
            .is_some()
        {
            return Err(Error::conflict(
                "Original volume fence changed before publication",
            ));
        }
        let spec: Value = mapping.get("source_spec");
        let request = manifest["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == spec["name"])
            .ok_or_else(|| Error::conflict("Candidate alias changed"))?;
        let access = request["access"].as_str().unwrap();
        let destination = request["mount_path"].as_str().unwrap();
        let fence = mapping
            .get::<_, Option<i64>>("target_fence")
            .ok_or_else(|| Error::conflict("Missing candidate fence"))?;
        let writer = (enabled && access == "rw").then_some(candidate);
        c.execute("UPDATE volumes SET write_fence=$2,writer_instance_id=$3,state=CASE WHEN state='full' THEN state WHEN $4 THEN 'attached' ELSE 'available' END,revision=revision+1,updated_at=now() WHERE id=$1", &[&target,&fence,&writer,&enabled]).await?;
        if enabled {
            c.execute("INSERT INTO volume_attachments(volume_id,instance_id,access,fence,generation,mount_path) VALUES($1,$2,$3,$4,$5,$6)", &[&target,&candidate,&access,&fence,&generation,&destination]).await?;
        }
        c.execute("UPDATE deployment_upgrade_volumes SET state='published',updated_at=now() WHERE operation_id=$1 AND target_volume_id=$2", &[&operation,&target]).await?;
    }
    if enabled {
        volumes::approve_mounts(c, ctx, deployment, generation, &target_spec, &manifest).await?;
    } else {
        c.execute(
            "DELETE FROM volume_mount_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await?;
    }
    c.execute(
        "DELETE FROM volume_holds WHERE operation_id=$1 AND kind='migration'",
        &[&operation],
    )
    .await?;
    Ok(())
}
/// Positive process stops and all owned clone-worker joins precede quarantine.
pub(super) async fn failed(c: &(impl GenericClient + Sync), operation: Uuid) -> Result<()> {
    c.execute("UPDATE volume_allocations SET state='quarantined',updated_at=now() WHERE operation_id=$1 AND state IN ('restoring','restored') AND NOT EXISTS(SELECT 1 FROM volumes v WHERE v.id=volume_allocations.volume_id)", &[&operation]).await?;
    c.execute("UPDATE deployment_upgrade_volumes SET state='failed',updated_at=now() WHERE operation_id=$1 AND state<>'published'", &[&operation]).await?;
    c.execute(
        "DELETE FROM volume_holds WHERE operation_id=$1 AND kind='migration'",
        &[&operation],
    )
    .await?;
    Ok(())
}
async fn mappings(c: &(impl GenericClient + Sync), operation: Uuid) -> Result<Vec<Row>> {
    Ok(c.query(
        "SELECT * FROM deployment_upgrade_volumes WHERE operation_id=$1 ORDER BY ordinal",
        &[&operation],
    )
    .await?)
}
/// The saved paths must authorize both groups even after version publication.
/// Later grants cannot rescue the original credential's revoked intent.
pub(super) async fn authorize_saved(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    job: &Row,
    rules: &Value,
) -> Result<()> {
    let old = ids(&job.get::<_, Value>("original_spec")["volume_ids"])?;
    let new = ids(&job.get::<_, Value>("target_spec")["volume_ids"])?;
    let rows = mappings(c, job.get("operation_id")).await?;
    let terminal = matches!(
        job.get::<_, String>("state").as_str(),
        "succeeded" | "failed"
    );
    if old.len() != new.len() || rows.len() != old.len() {
        return Err(Error::conflict("Upgrade volume mapping is incomplete"));
    }
    if old.is_empty() {
        return Ok(());
    }
    if !auth::rules_allow(
        rules,
        "volume.create",
        &Resource::new("volume", "new", Some(ctx.subject)),
        ctx.peer,
    ) {
        return Err(Error::denied());
    }
    for (ordinal, mapping) in rows.iter().enumerate() {
        let source: Uuid = mapping.get("source_volume_id");
        let target: Uuid = mapping.get("target_volume_id");
        if source != old[ordinal] || target != new[ordinal] {
            return Err(Error::conflict("Upgrade mapping differs from intent"));
        }
        for volume in [source, target] {
            let row = c
                .query_one(
                    "SELECT owner_id,spec FROM volume_allocations WHERE volume_id=$1",
                    &[&volume],
                )
                .await?;
            if row.get::<_, Uuid>("owner_id") != ctx.subject
                || (!terminal
                    && row.get::<_, Value>("spec") != mapping.get::<_, Value>("source_spec"))
            {
                return Err(Error::conflict(
                    "Upgrade volume ownership or specification changed",
                ));
            }
            if !auth::rules_allow(
                rules,
                "volume.read",
                &Resource::new("volume", volume, Some(ctx.subject)),
                ctx.peer,
            ) {
                return Err(Error::denied());
            }
        }
    }
    Ok(())
}

/// Called only after positive stop proofs and old attachment release.
pub(super) async fn freeze(c: &(impl GenericClient + Sync), operation: Uuid) -> Result<()> {
    let rows = mappings(c, operation).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let metadata: Value = c
        .query_one(
            "SELECT source_metadata FROM deployment_upgrade_jobs WHERE operation_id=$1",
            &[&operation],
        )
        .await?
        .get(0);
    let mut maximum = 1i64;
    for mapping in &rows {
        let source: Uuid = mapping.get("source_volume_id");
        schema_binding(c, source, &metadata["manifest"]).await?;
        let row = c.query_one("SELECT v.write_fence,v.spec,v.writer_instance_id,a.state,a.data_schema_version,h.operation_id FROM volumes v JOIN volume_allocations a ON a.volume_id=v.id JOIN volume_holds h ON h.volume_id=v.id WHERE v.id=$1 FOR UPDATE OF v,a", &[&source]).await?;
        if row.get::<_, Uuid>("operation_id") != operation
            || row.get::<_, Value>("spec") != mapping.get::<_, Value>("source_spec")
            || row.get::<_, String>("state") != "ready"
            || row.get::<_, Option<String>>("data_schema_version")
                != mapping.get::<_, Option<String>>("data_schema_version")
            || row.get::<_, Option<Uuid>>("writer_instance_id").is_some()
            || c.query_opt(
                "SELECT instance_id FROM volume_attachments WHERE volume_id=$1 LIMIT 1",
                &[&source],
            )
            .await?
            .is_some()
        {
            return Err(Error::conflict("Upgrade source is not exclusively stopped"));
        }
        let next = row
            .get::<_, i64>("write_fence")
            .checked_add(1)
            .filter(|n| (1..9_007_199_254_740_991).contains(n))
            .ok_or_else(Error::exhausted)?;
        maximum = maximum.max(next);
        c.execute(
            "UPDATE volumes SET write_fence=$2,revision=revision+1,updated_at=now() WHERE id=$1",
            &[&source, &next],
        )
        .await?;
        c.execute("UPDATE deployment_upgrade_volumes SET source_fence=$3,updated_at=now() WHERE operation_id=$1 AND source_volume_id=$2 AND state='reserved'", &[&operation,&source,&next]).await?;
    }
    let target = maximum + 1;
    c.execute("UPDATE deployment_upgrade_volumes SET target_fence=$2,updated_at=now() WHERE operation_id=$1", &[&operation,&target]).await?;
    c.execute(
        "UPDATE deployment_upgrade_jobs SET volume_fence=$2 WHERE operation_id=$1",
        &[&operation, &target],
    )
    .await?;
    Ok(())
}

/// Interrupted partial/private candidates fail closed to the retained originals.
/// Never overwrite a partial destination or replay its plugin initialization.
pub(super) async fn clone_group(app: &App, job: &Row) -> Result<()> {
    let operation: Uuid = job.get("operation_id");
    let rows = mappings(&app.db.connect().await?, operation).await?;
    if rows.is_empty() {
        return Ok(());
    }
    if job.get::<_, String>("state") != "stopped"
        || rows
            .iter()
            .any(|r| r.get::<_, String>("state") != "reserved")
    {
        return Err(Error::conflict(
            "Interrupted volume candidate must recover its original group",
        ));
    }
    let selected: Vec<Uuid> = rows.iter().map(|r| r.get("source_volume_id")).collect();
    let sources = volumes::backup_sources(app, operation, &selected).await?;
    let pool = volume_clone::Pool::default();
    for mapping in rows {
        let source: Uuid = mapping.get("source_volume_id");
        let target: Uuid = mapping.get("target_volume_id");
        let original = sources
            .iter()
            .find(|s| s.id == source)
            .ok_or_else(|| Error::conflict("Missing clone source"))?;
        let mut c = app.db.connect().await?;
        let tx = c.transaction().await?;
        db::auth_lock(&tx, false).await?;
        revalidate(&tx, app, job).await?;
        if tx.execute("UPDATE deployment_upgrade_volumes SET state='copying',updated_at=now() WHERE operation_id=$1 AND target_volume_id=$2 AND state='reserved' AND source_fence IS NOT NULL AND target_fence IS NOT NULL", &[&operation,&target]).await? != 1 { return Err(Error::conflict("Clone source fence missing")); }
        tx.commit().await?;
        let private = volumes::private_restore_path(app, operation, target, true).await?;
        let remaining = (job.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            - chrono::Utc::now())
        .to_std()
        .map_err(|_| Error::unavailable("Clone deadline expired"))?
        .min(Duration::from_secs(3600));
        // Always await the owned transfer; cleanup must not release its hold
        // or quarantine its project while a filesystem worker is still alive.
        let proof = pool
            .clone_verified(
                original.path.clone(),
                private.path,
                clone_plan(source, original.capacity_bytes),
                remaining,
            )?
            .wait()
            .await?;
        let tx = c.transaction().await?;
        db::auth_lock(&tx, false).await?;
        revalidate(&tx, app, job).await?;
        inherit_schema(&tx, job, source, target, &json!(proof)).await?;
        if tx.execute("UPDATE deployment_upgrade_volumes SET state='cloned',proof=$3,updated_at=now() WHERE operation_id=$1 AND target_volume_id=$2 AND state='copying'", &[&operation,&target,&json!(proof)]).await? != 1 { return Err(Error::conflict("Clone journal changed")); }
        if tx.execute("UPDATE volume_allocations SET state='restored',updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring'", &[&target,&operation]).await? != 1 { return Err(Error::conflict("Clone allocation changed")); }
        tx.commit().await?;
    }
    Ok(())
}

/// A format string is scoped to its signed plugin/publisher. Disabled intent
/// and a declared label alone cannot provide a source for a versioned clone.
async fn schema_binding(
    c: &(impl GenericClient + Sync),
    volume: Uuid,
    manifest: &Value,
) -> Result<Option<Value>> {
    let row = c.query_one("SELECT data_schema_version,schema_state,schema_deployment_id,schema_plugin_id,schema_publisher_id,schema_manifest_digest,schema_operation_id,schema_proof FROM volume_allocations WHERE volume_id=$1", &[&volume]).await?;
    let schema: Option<String> = row.get("data_schema_version");
    let state: String = row.get("schema_state");
    if manifest["data_schema_version"] == "none" {
        if state != "unversioned" || schema.as_deref().is_some_and(|s| s != "none") {
            return Err(Error::conflict(
                "Unversioned plugin cannot clone versioned data",
            ));
        }
        return Ok(None);
    }
    let proof = row
        .get::<_, Option<Value>>("schema_proof")
        .filter(Value::is_object)
        .ok_or_else(|| Error::conflict("Clone source lacks an actual format proof"))?;
    if state != "verified"
        || schema.as_deref() != manifest["data_schema_version"].as_str()
        || row.get::<_, Option<String>>("schema_plugin_id").as_deref()
            != manifest["plugin_id"].as_str()
        || row.get::<_, Option<Uuid>>("schema_publisher_id") != Some(id(manifest, "publisher_id")?)
    {
        return Err(Error::conflict(
            "Clone source format or signed scope differs",
        ));
    }
    Ok(Some(
        json!({"schema":schema,"deployment_id":row.get::<_,Option<Uuid>>("schema_deployment_id"),"plugin_id":row.get::<_,Option<String>>("schema_plugin_id"),"publisher_id":row.get::<_,Option<Uuid>>("schema_publisher_id"),"manifest_digest":row.get::<_,Option<String>>("schema_manifest_digest"),"operation_id":row.get::<_,Option<Uuid>>("schema_operation_id"),"proof_digest":crate::contracts::digest(&serde_json::to_vec(&proof)?)}),
    ))
}

/// Called only after the owned physical clone has actually been read back.
/// Identical bytes can carry an already verified SAME format into a new UUID;
/// this never converts or relabels a different source schema.
async fn inherit_schema(
    c: &(impl GenericClient + Sync),
    job: &Row,
    source: Uuid,
    target: Uuid,
    copy: &Value,
) -> Result<()> {
    let source_metadata: Value = job.get("source_metadata");
    let target_metadata: Value = job.get("target_metadata");
    let Some(binding) = schema_binding(c, source, &source_metadata["manifest"]).await? else {
        return Ok(());
    };
    if source_metadata["manifest"]["data_schema_version"]
        != target_metadata["manifest"]["data_schema_version"]
    {
        return Err(Error::conflict(
            "A clone cannot prove a different target schema",
        ));
    }
    let proof = json!({"kind":"same_schema_clone","source_volume_id":source,"source_binding":binding,"copy":copy,"operation_id":job.get::<_,Uuid>("operation_id"),"instance_id":job.get::<_,Uuid>("candidate_id"),"generation":job.get::<_,i64>("candidate_generation"),"manifest_digest":target_metadata["manifest_digest"]});
    if serde_json::to_vec(&proof)?.len() > 65536 {
        return Err(Error::exhausted());
    }
    let changed = c.execute("UPDATE volume_allocations SET schema_state='verified',data_schema_version=$3,schema_deployment_id=$4,schema_plugin_id=$5,schema_publisher_id=$6,schema_manifest_digest=$7,schema_operation_id=$2,schema_proof=$8,updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring' AND NOT EXISTS(SELECT 1 FROM volumes WHERE id=$1)", &[&target,&job.get::<_,Uuid>("operation_id"),&target_metadata["manifest"]["data_schema_version"].as_str().unwrap(),&job.get::<_,Uuid>("deployment_id"),&target_metadata["plugin_id"].as_str().unwrap(),&id(&target_metadata,"publisher_id")?,&target_metadata["manifest_digest"].as_str().unwrap(),&proof]).await?;
    if changed != 1 {
        return Err(Error::conflict("Verified schema clone is not private"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "upgrades_schema_tests.rs"]
mod schema_tests;
