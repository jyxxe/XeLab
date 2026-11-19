//! Persist or stop a schedule and its versioned admission proof.
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
    tasks::authorize_create(c, ctx).await?;
    admission::authorize_workflow_read(c, ctx, id(body, "workflow_id")?).await
}

pub(crate) async fn create(
    tx: &Transaction<'_>,
    ctx: &Context,
    body: &Value,
) -> Result<(Value, DateTime<Utc>)> {
    reject_jsonb_nul(body)?;
    let workflow_id = id(body, "workflow_id")?;
    let timezone = body
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_TIMEZONE);
    let cron = CronSchedule::parse(string(body, "cron")?, timezone)?;
    let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let admission = admission::prepare(
        tx,
        ctx,
        workflow_id,
        &body["input"],
        admission::Purpose::Schedule,
    )
    .await?;
    let proof = admission.schedule_record()?;
    let next_fire_at = if enabled {
        Some(
            cron.preview(admission.admitted_at, 1)?
                .fire_at
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Error::invalid(
                        "Enabled schedule has no occurrence within the cron search horizon",
                    )
                })?,
        )
    } else {
        None
    };
    let mut spec = body.clone();
    spec["workflow_id"] = json!(workflow_id);
    spec["enabled"] = json!(enabled);
    spec["timezone"] = json!(timezone);
    for key in ["misfire", "overlap"] {
        if spec.get(key).is_none() {
            spec[key] = json!("skip");
        }
    }
    let schedule = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    tx.execute(
        "INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,created_at)
         VALUES($1,$2,$3,$4,'user',$5,'active',$6)",
        &[&delegation, &ctx.subject, &ctx.credential, &admission.maximum_rules, &admission.expires_at, &admission.admitted_at],
    ).await?;
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
    let owner = c
        .query_opt("SELECT owner_id FROM schedules WHERE id=$1", &[&schedule])
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    authorize_owner(c, ctx, schedule, owner).await
}

async fn authorize_owner(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    schedule: Uuid,
    owner: Uuid,
) -> Result<()> {
    let resource = Resource::new("schedule", schedule, Some(owner));
    let result = auth::authorize(c, ctx, "schedule.delete", &resource).await;
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
    authorize_owner(tx, ctx, schedule, row.get("owner_id")).await?;
    etag(headers, row.get("revision"))?;

    // Serialize plan updates and fire creation on the same parent row. Task
    // links are retained, including legacy rows with a shared delegation.
    let skipped = tx
        .execute(
            "UPDATE schedule_fires SET outcome='skipped'
             WHERE schedule_id=$1 AND task_id IS NULL AND outcome IN ('pending','paused')",
            &[&schedule],
        )
        .await?;
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
