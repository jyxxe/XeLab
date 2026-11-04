//! Parent-owned format transitions: admission and validation of a private
//! target group while the public deployment still identifies its source.
use super::*;
use crate::{schema_jobs, volumes};
use migration_backup::Registration;

pub(super) struct Admission {
    registration: Registration,
    scratch: Uuid,
    reservation: volumes::RestorePlan,
}

pub(super) fn precheck(
    deployment: &Row,
    body: &Value,
    source: &Value,
    target: &Value,
) -> Result<bool> {
    if source["manifest"]["data_schema_version"] == target["manifest"]["data_schema_version"] {
        return Ok(false);
    }
    if deployment.get::<_, Value>("spec")["enabled"] != true
        || deployment.get::<_, String>("phase") != "ready"
        || source["manifest"]["data_schema_version"] == "none"
        || target["manifest"]["data_schema_version"] == "none"
    {
        return Err(Error::conflict(
            "Format transition requires an enabled ready versioned source",
        ));
    }
    if body.get("backup_id").is_none() {
        return Err(Error::conflict(
            "Format transition requires an explicit available backup",
        ));
    }
    id(body, "backup_id")?;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn plan(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    deployment: &Row,
    action: &str,
    body: &Value,
    source: &Value,
    target: &Value,
    group: Option<&volume_group::Admission>,
) -> Result<Option<Admission>> {
    if !precheck(deployment, body, source, target)? {
        return Ok(None);
    }
    let group = group.filter(|g| !g.items.is_empty()).ok_or_else(|| {
        Error::conflict("Format transition requires its complete business volume group")
    })?;
    let backup = id(body, "backup_id")?;
    let wanted = if action == "plugin.upgrade" {
        source
    } else {
        target
    };
    let registration =
        Registration::capture(c, app, ctx, backup, deployment.get("id"), wanted).await?;
    let paths = Policy::for_context(c, ctx)
        .credential(ctx.credential, 0)
        .await?;
    let point = registration.read(c, app, ctx, &paths).await?;
    let historical = point.volumes();
    if historical.len() != group.items.len() {
        return Err(Error::conflict("Backup business volume group differs"));
    }
    for item in &group.items {
        let saved = historical
            .iter()
            .find(|(_, spec)| spec["name"] == item.spec["name"])
            .ok_or_else(|| Error::conflict("Backup business alias differs"))?;
        if saved.1["capacity_bytes"]
            .as_u64()
            .is_none_or(|n| Some(n) > item.spec["capacity_bytes"].as_u64())
            || saved.1["access"] != item.spec["access"]
            || saved.1["scope"] != item.spec["scope"]
        {
            return Err(Error::conflict(
                "Backup capacity or access differs from the private target",
            ));
        }
    }
    let scratch = Uuid::new_v4();
    auth::authorize(
        c,
        ctx,
        "volume.read",
        &Resource::new("volume", scratch, Some(ctx.subject)),
    )
    .await?;
    // Kept separate from the at-most-16 business group, before any budget/D lock.
    let reservation = volumes::plan_restore(app, vec![(scratch, json!({"name":"kernel-schema-scratch","capacity_bytes":67108864,"access":"single_writer","scope":"deployment"}), None)]).await?;
    Ok(Some(Admission {
        registration,
        scratch,
        reservation,
    }))
}

pub(super) async fn reserve(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    operation: Uuid,
    admission: Admission,
) -> Result<()> {
    let parent = parent(c, operation).await?;
    let paths: Vec<policy::Path> = serde_json::from_value(parent.get("authority"))?;
    admission.registration.revalidate(c, ctx, &paths).await?;
    volumes::reserve_restore(c, ctx, operation, admission.reservation).await?;
    let target: Value = parent.get("target_metadata");
    let spec: Value = parent.get("target_spec");
    let mut physical = Vec::new();
    for value in spec["volume_ids"].as_array().unwrap() {
        let volume: Uuid = value
            .as_str()
            .unwrap()
            .parse()
            .map_err(|_| Error::conflict("Invalid private target"))?;
        let allocation = c.query_one("SELECT a.root_id,a.project_id,a.spec,r.filesystem_uuid FROM volume_allocations a JOIN volume_storage_root r ON r.id=a.root_id AND r.state='ready' WHERE a.volume_id=$1 AND a.operation_id=$2 AND a.owner_id=$3 AND a.state='restoring' AND NOT EXISTS(SELECT 1 FROM volumes WHERE id=$1)", &[&volume,&operation,&ctx.subject]).await?;
        physical.push(json!({"id":volume,"root":allocation.get::<_,Uuid>("root_id"),"filesystem":allocation.get::<_,Uuid>("filesystem_uuid"),"project":allocation.get::<_,i32>("project_id"),"spec":allocation.get::<_,Value>("spec")}));
    }
    let kind = if parent.get::<_, String>("action") == "plugin.upgrade" {
        "upgrade"
    } else {
        "rollback"
    };
    c.execute("INSERT INTO volume_schema_jobs(operation_id,kind,deployment_id,instance_id,generation,installation_id,credential_id,subject_id,peer,authority,deployment_spec,installation_metadata,source_schema,target_schema,scratch_volume_id,volume_ids,volume_plan,state,backup_id,backup_registration,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9::text::inet,$10,$11,$12,$13,$14,$15,$16,$17,'planning',$18,$19,$20)", &[&operation,&kind,&parent.get::<_,Uuid>("deployment_id"),&parent.get::<_,Uuid>("candidate_id"),&parent.get::<_,i64>("candidate_generation"),&parent.get::<_,Uuid>("target_installation_id"),&ctx.credential,&ctx.subject,&ctx.peer.to_string(),&parent.get::<_,Value>("authority"),&spec,&target,&parent.get::<_,Value>("source_metadata")["manifest"]["data_schema_version"].as_str().unwrap(),&target["manifest"]["data_schema_version"].as_str().unwrap(),&admission.scratch,&spec["volume_ids"],&json!(physical),&admission.registration.backup(),&serde_json::to_value(admission.registration)?,&parent.get::<_,chrono::DateTime<chrono::Utc>>("expires_at")]).await?;
    Ok(())
}

pub(super) async fn parent(c: &(impl GenericClient + Sync), operation: Uuid) -> Result<Row> {
    Ok(c.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM deployment_upgrade_jobs j WHERE operation_id=$1", &[&operation]).await?)
}

pub(crate) async fn revalidate(
    c: &(impl GenericClient + Sync),
    app: &App,
    child: &Row,
) -> Result<Context> {
    let operation: Uuid = child.get("operation_id");
    let parent = parent(c, operation).await?;
    validate_parent_binding(&parent, child)?;
    let ctx = super::revalidate(c, app, &parent).await?;
    schema_jobs::authorize_child(c, &ctx, child).await?;
    validate_private(c, child, &ctx).await?;
    Ok(ctx)
}

fn validate_parent_binding(parent: &Row, child: &Row) -> Result<()> {
    let expected_kind = if parent.get::<_, String>("action") == "plugin.upgrade" {
        "upgrade"
    } else {
        "rollback"
    };
    if child.get::<_, String>("kind") != expected_kind
        || child.get::<_, String>("approved_peer") != parent.get::<_, String>("approved_peer")
        || child.get::<_, Option<i64>>("volume_fence")
            != parent.get::<_, Option<i64>>("volume_fence")
        || child.get::<_, Option<Uuid>>("backup_id") != parent.get::<_, Option<Uuid>>("backup_id")
        || child.get::<_, Uuid>("deployment_id") != parent.get::<_, Uuid>("deployment_id")
        || child.get::<_, Uuid>("instance_id") != parent.get::<_, Uuid>("candidate_id")
        || child.get::<_, i64>("generation") != parent.get::<_, i64>("candidate_generation")
        || child.get::<_, Uuid>("installation_id")
            != parent.get::<_, Uuid>("target_installation_id")
        || child.get::<_, Uuid>("credential_id") != parent.get::<_, Uuid>("credential_id")
        || child.get::<_, Uuid>("subject_id") != parent.get::<_, Uuid>("subject_id")
        || child.get::<_, Value>("authority") != parent.get::<_, Value>("authority")
        || child.get::<_, Value>("deployment_spec") != parent.get::<_, Value>("target_spec")
        || child.get::<_, Value>("volume_ids")
            != parent.get::<_, Value>("target_spec")["volume_ids"]
        || child.get::<_, Value>("installation_metadata")
            != parent.get::<_, Value>("target_metadata")
        || json!(child.get::<_, String>("source_schema"))
            != parent.get::<_, Value>("source_metadata")["manifest"]["data_schema_version"]
        || json!(child.get::<_, String>("target_schema"))
            != parent.get::<_, Value>("target_metadata")["manifest"]["data_schema_version"]
        || child.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            != parent.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
        || child.get::<_, chrono::DateTime<chrono::Utc>>("expires_at") <= chrono::Utc::now()
    {
        return Err(Error::conflict(
            "Format child differs from its original upgrade intent",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "upgrades_transition_tests.rs"]
mod tests;

async fn validate_private(
    c: &(impl GenericClient + Sync),
    child: &Row,
    ctx: &Context,
) -> Result<()> {
    let operation: Uuid = child.get("operation_id");
    let instance = c
        .query_one(
            "SELECT deployment_id,generation,limits,phase FROM instances WHERE id=$1",
            &[&child.get::<_, Uuid>("instance_id")],
        )
        .await?;
    if instance.get::<_, Uuid>("deployment_id") != child.get::<_, Uuid>("deployment_id")
        || instance.get::<_, i64>("generation") != child.get::<_, i64>("generation")
        || instance.get::<_, Value>("limits") != child.get::<_, Value>("deployment_spec")["limits"]
        || instance.get::<_, String>("phase") == "stopped"
    {
        return Err(Error::conflict("Format candidate instance intent changed"));
    }
    let registration = registration(child)?;
    let paths: Vec<policy::Path> = serde_json::from_value(child.get("authority"))?;
    registration.revalidate(c, ctx, &paths).await?;
    let items: Value = child.get("volume_plan");
    if items.as_array().map_or(0, Vec::len)
        != child
            .get::<_, Value>("volume_ids")
            .as_array()
            .map_or(0, Vec::len)
    {
        return Err(Error::conflict("Format target group is incomplete"));
    }
    for (ordinal, item) in items
        .as_array()
        .ok_or_else(|| Error::conflict("Invalid format physical plan"))?
        .iter()
        .enumerate()
    {
        if item["id"] != child.get::<_, Value>("volume_ids")[ordinal] {
            return Err(Error::conflict("Format physical selection differs"));
        }
        let row = c.query_one("SELECT a.*,r.filesystem_uuid FROM volume_allocations a JOIN volume_storage_root r ON r.id=a.root_id AND r.state='ready' WHERE a.volume_id=$1 AND NOT EXISTS(SELECT 1 FROM volumes WHERE id=$1)", &[&id(item,"id")?]).await?;
        if row.get::<_, Uuid>("operation_id") != operation
            || row.get::<_, Uuid>("owner_id") != ctx.subject
            || row.get::<_, Uuid>("root_id") != id(item, "root")?
            || row.get::<_, Uuid>("filesystem_uuid") != id(item, "filesystem")?
            || Some(row.get::<_, i32>("project_id") as i64) != item["project"].as_i64()
            || row.get::<_, Value>("spec") != item["spec"]
            || !matches!(
                row.get::<_, String>("state").as_str(),
                "restoring" | "restored"
            )
        {
            return Err(Error::conflict("Format private allocation changed"));
        }
    }
    let scratch = c
        .query_one(
            "SELECT operation_id,owner_id,state FROM volume_allocations WHERE volume_id=$1",
            &[&child.get::<_, Uuid>("scratch_volume_id")],
        )
        .await?;
    if scratch.get::<_, Uuid>("operation_id") != operation
        || scratch.get::<_, Uuid>("owner_id") != ctx.subject
        || scratch.get::<_, String>("state") != "restoring"
    {
        return Err(Error::conflict("Format scratch allocation changed"));
    }
    Ok(())
}

pub(crate) fn registration(child: &Row) -> Result<Registration> {
    let value = child
        .get::<_, Option<Value>>("backup_registration")
        .ok_or_else(|| Error::conflict("Format backup registration is absent"))?;
    let registration: Registration = serde_json::from_value(value)
        .map_err(|_| Error::conflict("Format backup registration is invalid"))?;
    if Some(registration.backup()) != child.get::<_, Option<Uuid>>("backup_id") {
        return Err(Error::conflict("Format backup identity differs"));
    }
    Ok(registration)
}

/// Incompatible rollback extracts the admitted OLD target backup into fresh
/// private UUIDs; it never asks Apply to invert the current public format.
pub(super) async fn restore_group(app: &App, job: &Row, fence: &db::DeploymentFence) -> Result<()> {
    let operation: Uuid = job.get("operation_id");
    let child = app.db.connect().await?.query_one("SELECT j.*,host(j.peer) AS approved_peer FROM volume_schema_jobs j WHERE operation_id=$1", &[&operation]).await?;
    let mappings = app
        .db
        .connect()
        .await?
        .query(
            "SELECT * FROM deployment_upgrade_volumes WHERE operation_id=$1 ORDER BY ordinal",
            &[&operation],
        )
        .await?;
    if mappings
        .iter()
        .all(|r| r.get::<_, String>("state") == "cloned")
    {
        return Ok(());
    }
    if job.get::<_, String>("state") != "stopped"
        || child.get::<_, String>("state") != "planning"
        || mappings.is_empty()
        || mappings
            .iter()
            .any(|r| r.get::<_, String>("state") != "reserved")
    {
        return Err(Error::conflict(
            "Partial format restore must retain the original group",
        ));
    }
    let mut c = app.db.connect().await?;
    let ctx = revalidate(&c, app, &child).await?;
    let paths: Vec<policy::Path> = serde_json::from_value(child.get("authority"))?;
    let registration = registration(&child)?;
    let point = registration.read(&c, app, &ctx, &paths).await?;
    let pool = crate::storage::backup_transfer::Pool::default();
    for mapping in mappings {
        if !fence.protects(job.get("deployment_id")) {
            return Err(Error::unavailable("Restore deployment fence is lost"));
        }
        let tx = c.transaction().await?;
        db::auth_lock(&tx, false).await?;
        revalidate(&tx, app, &child).await?;
        let target: Uuid = mapping.get("target_volume_id");
        if tx.execute("UPDATE deployment_upgrade_volumes SET state='copying',updated_at=now() WHERE operation_id=$1 AND target_volume_id=$2 AND state='reserved' AND target_fence IS NOT NULL AND source_fence IS NOT NULL", &[&operation,&target]).await? != 1 { return Err(Error::conflict("Restore mapping changed")); }
        tx.commit().await?;
        let private = volumes::private_restore_path(app, operation, target, true).await?;
        let timeout = (child.get::<_, chrono::DateTime<chrono::Utc>>("expires_at")
            - chrono::Utc::now())
        .to_std()
        .map_err(|_| Error::unavailable("Format restore expired"))?
        .min(std::time::Duration::from_secs(3600));
        // The owned pool handle is always awaited, including transfer errors.
        let proof = point
            .restore(
                &pool,
                private.spec["name"].as_str().unwrap(),
                private.path,
                crate::backups::master(&app.config)?,
                timeout,
            )
            .await?;
        let tx = c.transaction().await?;
        db::auth_lock(&tx, false).await?;
        revalidate(&tx, app, &child).await?;
        if !fence.protects(job.get("deployment_id")) {
            return Err(Error::unavailable("Restore deployment fence is lost"));
        }
        let proof = json!({"kind":"verified_backup_restore","backup_id":registration.backup(),"manifest_digest":registration.manifest_digest(),"restore":proof});
        if tx.execute("UPDATE deployment_upgrade_volumes SET state='cloned',proof=$3,updated_at=now() WHERE operation_id=$1 AND target_volume_id=$2 AND state='copying'", &[&operation,&target,&proof]).await? != 1 {return Err(Error::conflict("Restore journal changed"));}
        if tx.execute("UPDATE volume_allocations SET state='restored',updated_at=now() WHERE volume_id=$1 AND operation_id=$2 AND state='restoring'", &[&target,&operation]).await? != 1 {return Err(Error::conflict("Restore allocation changed"));}
        tx.commit().await?;
    }
    Ok(())
}
