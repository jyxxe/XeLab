//! The existing deployment PUT, restricted to a fully stopped ordinary OCI
//! deployment. Runtime startup, resource admission and storage changes retain
//! their existing enable/upgrade/restore workflows.
use crate::{
    api::{etag, path_id, read_object, App},
    artifact::Artifact,
    auth::{self, Context, Resource},
    contracts::{id, unsigned, validate},
    db,
    error::{Error, Result},
};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub fn handles(name: &str) -> bool {
    name == "updateDeployment"
}

pub async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    _body: &Value,
) -> Result<()> {
    if !handles(name) {
        return Err(Error::not_found());
    }
    let deployment = path_id(params, "deployment_id")?;
    let row = c
        .query_opt(
            "SELECT owner_id FROM deployments WHERE id=$1 AND archived_at IS NULL",
            &[&deployment],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    auth::authorize(
        c,
        ctx,
        "plugin.configure",
        &Resource::new("deployment", deployment, Some(row.get("owner_id"))),
    )
    .await
}

fn volume_ids(spec: &Value) -> Result<BTreeSet<Uuid>> {
    let values = spec["volume_ids"]
        .as_array()
        .ok_or_else(|| Error::conflict("Stored volume selection is invalid"))?;
    let ids: BTreeSet<Uuid> = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| Error::invalid("Invalid selected volume"))
        })
        .collect::<Result<_>>()?;
    if ids.len() != values.len() {
        return Err(Error::invalid("Duplicate selected volume"));
    }
    Ok(ids)
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    if !query.is_empty() {
        return Err(Error::invalid("Unexpected deployment query parameter"));
    }
    authorize(c, ctx, name, params, body).await?;
    validate("DeploymentPatch", body)?;
    let deployment = path_id(params, "deployment_id")?;
    crate::dependencies::dependency_lock(c).await?;
    // The caller retains the exclusive authorization transaction until commit.
    // Never wait on a Controller fence while holding that authorization lock:
    // Controller may be waiting for its shared authorization check.
    let _runtime_fence = db::deployment_fence(&app.db, deployment)
        .await?
        .ok_or_else(|| Error::conflict("Deployment runtime mutation is in progress"))?;
    let row = c
        .query_opt(
            "SELECT * FROM deployments WHERE id=$1 AND archived_at IS NULL FOR UPDATE",
            &[&deployment],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    etag(headers, row.get("revision"))?;
    let previous: Value = row.get("spec");
    if previous["enabled"] == true || row.get::<_, String>("phase") != "disabled" {
        return Err(Error::conflict(
            "Disable and fully stop the deployment before configuration",
        ));
    }
    let mut wanted = body["spec"].clone();
    if wanted["enabled"] == true {
        return Err(Error::invalid(
            "Enable the deployment through setDeploymentEnabled",
        ));
    }
    let installation: Uuid = row.get("installation_id");
    if id(&wanted, "installation_id")? != installation
        || id(&previous, "installation_id")? != installation
    {
        return Err(Error::conflict(
            "Changing an installation requires deployment upgrade",
        ));
    }
    let selected = volume_ids(&wanted)?;
    if selected != volume_ids(&previous)? {
        return Err(Error::conflict(
            "Changing volume bindings requires the storage workflow",
        ));
    }
    if wanted
        .get("replicas")
        .map(unsigned)
        .transpose()?
        .unwrap_or(1)
        != 1
    {
        return Err(Error::invalid("This runtime profile requires one replica"));
    }
    wanted["enabled"] = json!(false);
    wanted["replicas"] = json!(1);
    for field in ["cpu_millis", "memory_bytes", "ephemeral_bytes", "pids"] {
        wanted["limits"][field] = json!(unsigned(&wanted["limits"][field])?);
    }
    crate::backups::ensure_unheld(c, deployment).await?;
    let held:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM instances WHERE deployment_id=$1 AND (phase<>'stopped' OR runtime_id IS NOT NULL)) OR EXISTS(SELECT 1 FROM reservations r LEFT JOIN instances i ON i.id=r.instance_id LEFT JOIN calls q ON q.id=r.id LEFT JOIN instances p ON p.id=q.provider_instance_id WHERE r.status<>'released' AND (i.deployment_id=$1 OR p.deployment_id=$1)) OR EXISTS(SELECT 1 FROM calls q JOIN instances i ON i.id=q.provider_instance_id WHERE i.deployment_id=$1 AND q.state IN ('accepted','running')) OR EXISTS(SELECT 1 FROM plugin_http_requests h JOIN instances i ON i.id=h.instance_id WHERE i.deployment_id=$1) OR EXISTS(SELECT 1 FROM volume_attachments a JOIN instances i ON i.id=a.instance_id WHERE i.deployment_id=$1) OR EXISTS(SELECT 1 FROM volume_holds WHERE volume_id=ANY($2))", &[&deployment,&selected.iter().copied().collect::<Vec<_>>()]).await?.get(0);
    if held {
        return Err(Error::conflict(
            "Deployment has unfinished runtime or storage resources",
        ));
    }
    let installed=c.query_opt("SELECT n.* FROM installations n JOIN publishers p ON p.id=n.publisher_id WHERE n.id=$1 AND n.state='installed' AND p.status='trusted' FOR SHARE OF n,p", &[&installation]).await?.ok_or_else(||Error::unavailable("Deployment source installation is unavailable"))?;
    let manifest: Value = installed.get("manifest");
    crate::artifact::validate_runtime_profile(&manifest)?;
    for field in ["cpu_millis", "memory_bytes", "ephemeral_bytes", "pids"] {
        if unsigned(&wanted["limits"][field])? < unsigned(&manifest["minimum_resources"][field])? {
            return Err(Error::invalid(
                "Deployment limits are below the signed minimum",
            ));
        }
    }
    Artifact {
        image: String::new(),
        manifest: manifest.clone(),
        raw: Vec::new(),
        image_digest: installed.get("image_digest"),
        manifest_digest: installed.get("manifest_digest"),
        http_contract: None,
    }
    .validate_deployment(&wanted)?;
    crate::http_contracts::validate_actions(c, &manifest).await?;
    crate::volumes::validate_selection(c, ctx, &wanted, &manifest).await?;
    if row.get::<_, i64>("revision") >= i32::MAX as i64 {
        return Err(Error::exhausted());
    }
    crate::dependencies::clear(c, deployment).await?;
    c.execute("UPDATE deployments SET spec=$2,revision=revision+1,conditions='[]',updated_at=now() WHERE id=$1", &[&deployment,&wanted]).await?;
    let operation = db::operation(c, ctx.subject, name, Some(deployment.to_string())).await?;
    let result = read_object(c, "deployments", deployment, "Deployment").await?;
    db::complete(c, operation, Ok(result)).await?;
    db::outbox(c, "deployment.changed", deployment, &json!({})).await?;
    Ok((db::operation_value(c, operation).await?, 202))
}

#[cfg(test)]
#[path = "deployments_tests.rs"]
mod tests;
