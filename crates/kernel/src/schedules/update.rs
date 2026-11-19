//! Update the schedule and settle unsubmitted fires under the trigger's lock.
use super::{
    authorize_existing, authorize_owner, authorize_submission, next_fire, normalize_spec,
    parse_cron, persist_delegation, require_live_execution, skip_unsubmitted_fires,
};
use crate::{
    api::etag,
    auth::{Auth, Context},
    contracts::{id, reject_jsonb_nul},
    error::{Error, Result},
    tasks::{self, admission},
};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

const ACTION: &str = "schedule.update";

pub(crate) async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    schedule: Uuid,
) -> Result<()> {
    authorize_existing(c, ctx, ACTION, schedule).await
}

pub(crate) async fn apply(
    tx: &Transaction<'_>,
    auth: &Auth,
    ctx: &Context,
    schedule: Uuid,
    body: &Value,
    headers: &HeaderMap,
) -> Result<(Value, Option<DateTime<Utc>>)> {
    // The scheduler, stop and edits lock the same row before touching a fire.
    let row = tx
        .query_opt(
            "SELECT s.owner_id,s.revision,s.spec,s.admission,s.delegation_id,s.next_fire_at,
                    e.subject_id,e.issuer_credential_id,e.maximum_rules,e.expires_at,
                    (e.status='active' AND e.run_as='user' AND e.service_account_id IS NULL
                     AND e.subject_id=s.owner_id AND e.issuer_credential_id IS NOT NULL
                     AND e.expires_at>clock_timestamp()
                     AND NOT EXISTS (SELECT FROM calls q WHERE q.delegation_id=e.id)
                     AND NOT EXISTS (SELECT FROM tasks t WHERE t.delegation_id=e.id)) AS delegation_live
               FROM schedules s JOIN execution_delegations e ON e.id=s.delegation_id
              WHERE s.id=$1 FOR UPDATE OF s FOR SHARE OF e",
            &[&schedule],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let owner: Uuid = row.try_get("owner_id")?;
    authorize_owner(tx, ctx, ACTION, schedule, owner).await?;
    etag(headers, row.try_get("revision")?)?;
    reject_jsonb_nul(body)?;
    let spec = normalize_spec(body);
    let workflow = id(&spec, "workflow_id")?;
    let cron = parse_cron(&spec)?;
    let mut previous = normalize_spec(&row.try_get("spec")?);
    let mut cutoff = None;
    if spec != previous {
        let workflow_changed = previous["workflow_id"] != spec["workflow_id"];
        let input_changed = previous["input"] != spec["input"];
        if let Some(fields) = previous.as_object_mut() {
            fields.insert("name".to_owned(), spec["name"].clone());
        }
        let execution_changed = previous != spec;
        let enabled = spec["enabled"] == true;
        let mut proof: Option<Value> = row.try_get("admission")?;
        let mut delegation: Uuid = row.try_get("delegation_id")?;

        if workflow_changed || input_changed || (execution_changed && enabled) {
            if ctx.subject != owner {
                return Err(Error::denied());
            }
            authorize_submission(tx, ctx, workflow).await?;
            if workflow_changed {
                // Only an execution change may create a proof. A different
                // Workflow is an explicit choice of a new execution scope.
                let admitted = admission::prepare(
                    tx,
                    ctx,
                    workflow,
                    &spec["input"],
                    admission::Purpose::Schedule,
                )
                .await?;
                proof = Some(admitted.schedule_record()?);
                delegation = persist_delegation(tx, ctx, &admitted).await?;
                cutoff = Some(admitted.expires_at);
            } else {
                // Recheck the stored origin and live editor against the same
                // pinned targets. An input or cadence edit cannot replace them
                // or bootstrap a proof for historical rows without one.
                if proof.is_none() {
                    return Err(Error::denied());
                }
                cutoff = Some(
                    tasks::validate_scheduled(tx, auth, &row, &spec, Some(ctx))
                        .await?
                        .cutoff,
                );
            }
        }
        let next = if execution_changed {
            let clock: DateTime<Utc> = tx.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
            let next = next_fire(&cron, enabled, clock)?;
            skip_unsubmitted_fires(tx, schedule).await?;
            next
        } else {
            row.try_get("next_fire_at")?
        };
        tx.execute(
            "UPDATE schedules SET spec=$2,delegation_id=$3,admission=$4,next_fire_at=$5,
                                  revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
            &[&schedule, &spec, &delegation, &proof, &next],
        )
        .await?;
    }
    let value = crate::plugin_catalog::read_schedule(tx, schedule, Some(owner))
        .await?
        .ok_or_else(Error::not_found)?;
    Ok((value, cutoff))
}

pub(crate) async fn validate_commit(
    tx: &Transaction<'_>,
    ctx: &Context,
    schedule: Uuid,
    body: &Value,
    cutoff: Option<DateTime<Utc>>,
) -> Result<()> {
    authorize(tx, ctx, schedule).await?;
    if let Some(cutoff) = cutoff {
        authorize_submission(tx, ctx, id(body, "workflow_id")?).await?;
        require_live_execution(tx, cutoff).await?;
    }
    Ok(())
}
