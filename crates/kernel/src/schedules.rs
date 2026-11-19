//! Manage schedules and their versioned admission proof.
use crate::{
    api::etag,
    auth::{self, Context, Resource},
    contracts::{id, reject_jsonb_nul, string},
    error::{Error, Result},
    schedule_cron::{CronSchedule, DEFAULT_TIMEZONE},
    tasks::{self, admission},
};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub(crate) mod trigger;
pub(crate) mod update;

pub(crate) async fn authorize_create(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    body: &Value,
) -> Result<()> {
    auth::authorize(
        c,
        ctx,
        "schedule.create",
        &Resource::new("schedule", "new", Some(ctx.subject)),
    )
    .await?;
    authorize_submission(c, ctx, id(body, "workflow_id")?).await
}

async fn authorize_submission(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    workflow: Uuid,
) -> Result<()> {
    tasks::authorize_create(c, ctx).await?;
    admission::authorize_workflow_read(c, ctx, workflow).await
}

fn normalize_spec(value: &Value) -> Value {
    let mut spec = value.clone();
    // Legacy schedules may omit explicit defaults. Compare their effective
    // configuration so a name change never refreshes the original authority.
    if let Some(fields) = spec.as_object_mut() {
        for (key, default) in [
            ("enabled", json!(true)),
            ("timezone", json!(DEFAULT_TIMEZONE)),
            ("misfire", json!("skip")),
            ("overlap", json!("skip")),
        ] {
            fields.entry(key).or_insert(default);
        }
    }
    if let Ok(workflow) = id(value, "workflow_id") {
        spec["workflow_id"] = json!(workflow);
    }
    spec
}

fn parse_cron(spec: &Value) -> Result<CronSchedule> {
    CronSchedule::parse(string(spec, "cron")?, string(spec, "timezone")?)
}

fn next_fire(
    cron: &CronSchedule,
    enabled: bool,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    if !enabled {
        return Ok(None);
    }
    Ok(Some(
        cron.preview(after, 1)?
            .fire_at
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::invalid("Enabled schedule has no occurrence within the cron search horizon")
            })?,
    ))
}

async fn persist_delegation(
    tx: &Transaction<'_>,
    ctx: &Context,
    admission: &admission::FrozenWorkflow,
) -> Result<Uuid> {
    let delegation = Uuid::new_v4();
    tx.execute(
        "INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,created_at)
         VALUES($1,$2,$3,$4,'user',$5,'active',$6)",
        &[&delegation, &ctx.subject, &ctx.credential, &admission.maximum_rules, &admission.expires_at, &admission.admitted_at],
    ).await?;
    Ok(delegation)
}

pub(crate) async fn create(
    tx: &Transaction<'_>,
    ctx: &Context,
    body: &Value,
) -> Result<(Value, DateTime<Utc>)> {
    reject_jsonb_nul(body)?;
    let spec = normalize_spec(body);
    let workflow_id = id(&spec, "workflow_id")?;
    let cron = parse_cron(&spec)?;
    let enabled = spec["enabled"] == true;
    let admission = admission::prepare(
        tx,
        ctx,
        workflow_id,
        &spec["input"],
        admission::Purpose::Schedule,
    )
    .await?;
    let proof = admission.schedule_record()?;
    let next_fire_at = next_fire(&cron, enabled, admission.admitted_at)?;
    let schedule = Uuid::new_v4();
    let delegation = persist_delegation(tx, ctx, &admission).await?;
    tx.execute(
        "INSERT INTO schedules(id,owner_id,spec,delegation_id,next_fire_at,created_at,updated_at,admission)
         VALUES($1,$2,$3,$4,$5,$6,$6,$7)",
        &[&schedule, &ctx.subject, &spec, &delegation, &next_fire_at, &admission.admitted_at, &proof],
    ).await?;
    let value = crate::plugin_catalog::read_schedule(tx, schedule, Some(ctx.subject))
        .await?
        .ok_or_else(Error::not_found)?;
    Ok((value, admission.expires_at))
}

pub(crate) async fn validate_create_commit(
    tx: &Transaction<'_>,
    ctx: &Context,
    body: &Value,
    cutoff: DateTime<Utc>,
) -> Result<()> {
    authorize_create(tx, ctx, body).await?;
    require_live_execution(tx, cutoff).await
}

async fn require_live_execution(tx: &Transaction<'_>, cutoff: DateTime<Utc>) -> Result<()> {
    let live: bool = tx.query_one(
        "SELECT clock_timestamp()<$1 AND NOT EXISTS(SELECT FROM platform_recovery_state WHERE phase<>'ready')",
        &[&cutoff],
    ).await?.get(0);
    if !live {
        return Err(Error::conflict(
            "Schedule execution authority is no longer live",
        ));
    }
    Ok(())
}

pub(crate) async fn authorize_delete(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    schedule: Uuid,
) -> Result<()> {
    authorize_existing(c, ctx, "schedule.delete", schedule).await
}

async fn authorize_existing(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    schedule: Uuid,
) -> Result<()> {
    let owner = c
        .query_opt("SELECT owner_id FROM schedules WHERE id=$1", &[&schedule])
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    authorize_owner(c, ctx, action, schedule, owner).await
}

async fn authorize_owner(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    schedule: Uuid,
    owner: Uuid,
) -> Result<()> {
    let resource = Resource::new("schedule", schedule, Some(owner));
    let result = auth::authorize(c, ctx, action, &resource).await;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "PERMISSION_DENIED")
        && !crate::policy::allows(c, ctx, "schedule.read", &resource).await?
    {
        return Err(Error::not_found());
    }
    result
}

pub(crate) async fn stop(
    tx: &Transaction<'_>,
    ctx: &Context,
    schedule: Uuid,
    headers: &HeaderMap,
) -> Result<()> {
    let row = tx
        .query_opt(
            "SELECT owner_id,revision FROM schedules WHERE id=$1 FOR UPDATE",
            &[&schedule],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    authorize_owner(tx, ctx, "schedule.delete", schedule, row.get("owner_id")).await?;
    etag(headers, row.get("revision"))?;

    // Serialize plan updates and fire creation on the same parent row. Task
    // links are retained, including legacy rows with a shared delegation.
    let skipped = skip_unsubmitted_fires(tx, schedule).await?;
    tx.execute(
        "UPDATE schedules SET spec=jsonb_set(spec,'{enabled}','false'),next_fire_at=NULL,
            revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1 AND ($2 OR next_fire_at IS NOT NULL
                          OR spec->'enabled' IS DISTINCT FROM 'false'::jsonb)",
        &[&schedule, &(skipped != 0)],
    )
    .await?;
    Ok(())
}

async fn skip_unsubmitted_fires(tx: &Transaction<'_>, schedule: Uuid) -> Result<u64> {
    Ok(tx
        .execute(
            "UPDATE schedule_fires SET outcome='skipped'
         WHERE schedule_id=$1 AND task_id IS NULL AND outcome IN ('pending','paused')",
            &[&schedule],
        )
        .await?)
}
