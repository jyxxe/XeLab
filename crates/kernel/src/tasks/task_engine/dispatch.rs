//! Lock the exact task identity inside the Broker's business transaction.
use super::{active, claim_authority, lease::step_deadline, AttemptLease, DispatchInput};
use crate::{
    auth::{Auth, Context},
    contracts::{id, string},
    error::{Error, Result},
    policy::Path,
    tasks::dispatch_input,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::time::Duration;
use tokio_postgres::Transaction;
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(crate) enum BrokerLeaseUse {
    Admission,
    Send,
    Observe,
}

pub(crate) struct BrokerDispatchProof {
    pub ctx: Context,
    pub business_paths: Vec<Path>,
    pub hard_deadline: DateTime<Utc>,
    pub lease_end: DateTime<Utc>,
    pub step_id: Uuid,
    pub deployment: Uuid,
    pub installation: Uuid,
    pub contract: Value,
    pub business_key: String,
    pub active: bool,
    pub renewable: bool,
}

/// The caller holds the shared authorization fence and owns the transaction.
/// Admission and settlement also hold the Call ledger before these row locks.
pub(crate) async fn broker_dispatch_proof(
    tx: &Transaction<'_>,
    auth: &Auth,
    lease: &AttemptLease,
    purpose: BrokerLeaseUse,
) -> Result<BrokerDispatchProof> {
    let stale = || Error::conflict("Worker lease does not match a live task dispatch");
    let row = active::lock_row(tx, lease).await?.ok_or_else(stale)?;
    let lease_end = active::lease_end(&row);
    if !row.try_get::<_, bool>("current_identity")? || !active::live(tx, lease_end).await? {
        return Err(stale());
    }
    let active = active::active(&row, matches!(purpose, BrokerLeaseUse::Observe));
    // Terminal or cancel-requested replay can inspect a durable Call. Only
    // a running task may begin transport for a new one.
    let state: &str = row.try_get("attempt_state")?;
    let can_replay = matches!(purpose, BrokerLeaseUse::Admission)
        && ((matches!(state, "succeeded" | "failed" | "unknown")
            && row.try_get::<_, &str>("step_state")? == state)
            || active::active(&row, true));
    if !active && !can_replay {
        return Err(stale());
    }
    if !row.try_get::<_, bool>("delegation_live")? || !row.try_get::<_, bool>("admitted")? {
        return Err(Error::denied());
    }
    let unsupported =
        || Error::unavailable("Task dispatch requires a first fixed unary Capability attempt");
    let step: Value = row.try_get("step")?;
    let deployment = row
        .try_get::<_, Option<Uuid>>("provider_deployment_id")?
        .ok_or_else(unsupported)?;
    if row.try_get::<_, i32>("number")? != 1 || step.get("skill_id").is_some() {
        return Err(unsupported());
    }
    let contract = &step["contract"];
    if string(contract, "name")? == "automation.execute" {
        return Err(unsupported());
    }
    let proof =
        claim_authority::revalidate_proof(tx, auth, &row, &step, lease.execution_id, true).await?;
    let admission: Value = row.try_get("admission")?;
    let step_key = string(&step, "id")?;
    let target = admission["steps"]
        .as_array()
        .and_then(|records| records.iter().find(|record| record["step_key"] == step_key))
        .ok_or_else(Error::denied)?;
    let installation = id(target, "installation_id").map_err(|_| Error::denied())?;
    let hard_deadline = row
        .try_get::<_, DateTime<Utc>>("deadline_at")?
        .min(row.try_get("expires_at")?)
        .min(step_deadline(&row, &step)?)
        .min(proof.until);
    Ok(BrokerDispatchProof {
        ctx: proof.ctx,
        business_paths: proof.business_paths,
        hard_deadline,
        lease_end,
        step_id: row.try_get("step_id")?,
        deployment,
        installation,
        contract: contract.clone(),
        business_key: row.try_get("idempotency_key")?,
        active,
        renewable: active::active(&row, false),
    })
}

pub(crate) async fn broker_input(
    tx: &Transaction<'_>,
    proof: &BrokerDispatchProof,
) -> Result<DispatchInput> {
    dispatch_input::build(tx, proof.step_id, proof.hard_deadline).await
}

pub(crate) async fn broker_paths_until(
    tx: &Transaction<'_>,
    ctx: &Context,
    paths: &[Path],
) -> Result<DateTime<Utc>> {
    claim_authority::dispatch_paths_until(tx, ctx, paths).await
}

pub(crate) async fn broker_remaining(
    tx: &Transaction<'_>,
    cutoff: DateTime<Utc>,
) -> Result<Duration> {
    let row = tx
        .query_one(
            "SELECT clock_timestamp(), NOT EXISTS (
        SELECT FROM platform_recovery_state WHERE phase<>'ready')",
            &[],
        )
        .await?;
    let now: DateTime<Utc> = row.try_get(0)?;
    if !row.try_get::<_, bool>(1)? || now >= cutoff {
        return Err(Error::conflict(
            "Task dispatch deadline or recovery fence has passed",
        ));
    }
    (cutoff - now)
        .to_std()
        .map_err(|_| Error::conflict("Task dispatch deadline has passed"))
}
