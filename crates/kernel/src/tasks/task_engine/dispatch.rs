//! Lock the exact task identity inside the Broker's business transaction.
use super::{active, claim_authority, lease::step_deadline, retry, AttemptLease, DispatchInput};
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
use tokio_postgres::{Row, Transaction};
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
    pub binding: TaskCallBinding,
    pub business_key: String,
    pub active: bool,
    pub renewable: bool,
    attempt_number: i32,
    step_definition: Value,
}

pub(crate) struct TaskCallBinding {
    pub deployment: Uuid,
    pub installation: Uuid,
    pub contract: Value,
    pub skill: Option<(Uuid, String)>,
}

impl TaskCallBinding {
    pub(super) fn from_admission(
        admission: &Value,
        step: &Value,
        deployment: Uuid,
    ) -> Result<Self> {
        let record = claim_authority::step_admission(admission, step)?;
        let binding = &record["binding"];
        let skill = match binding["kind"].as_str() {
            Some("capability") => None,
            Some("skill_capability") => Some((
                id(binding, "skill_id").map_err(|_| Error::denied())?,
                string(binding, "definition_digest")
                    .map_err(|_| Error::denied())?
                    .to_owned(),
            )),
            _ => {
                return Err(Error::unavailable(
                    "Task dispatch requires a fixed unary Capability target",
                ))
            }
        };
        Ok(Self {
            deployment,
            installation: id(record, "installation_id").map_err(|_| Error::denied())?,
            contract: binding["contract"].clone(),
            skill,
        })
    }

    // Both the send/settle path and result receipts use the exact linked Call.
    pub(crate) fn matches_call(&self, row: &Row) -> Result<bool> {
        let skill = self.skill.as_ref();
        Ok(
            row.try_get::<_, Uuid>("installation_id")? == self.installation
                && row.try_get::<_, Option<Uuid>>("deployment_id")? == Some(self.deployment)
                && row.try_get::<_, Value>("contract")? == self.contract
                && row.try_get::<_, Option<Uuid>>("skill_id")? == skill.map(|(id, _)| *id)
                && row.try_get::<_, Option<&str>>("skill_definition_digest")?
                    == skill.map(|(_, digest)| digest.as_str())
                && row.try_get::<_, Option<Value>>("http_binding")?.is_none(),
        )
    }
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
        || Error::unavailable("Task dispatch requires a fixed unary Capability attempt");
    let step: Value = row.try_get("step")?;
    let attempt_number: i32 = row.try_get("number")?;
    let deployment = row
        .try_get::<_, Option<Uuid>>("provider_deployment_id")?
        .ok_or_else(unsupported)?;
    if !(1..=5).contains(&attempt_number) {
        return Err(unsupported());
    }
    let proof =
        claim_authority::revalidate_proof(tx, auth, &row, &step, lease.execution_id, true).await?;
    let admission: Value = row.try_get("admission")?;
    let binding = TaskCallBinding::from_admission(&admission, &step, deployment)?;
    if string(&binding.contract, "name")? == "automation.execute"
        || (attempt_number > 1 && binding.skill.is_some())
    {
        return Err(unsupported());
    }
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
        binding,
        business_key: row.try_get("idempotency_key")?,
        active,
        renewable: active::active(&row, false),
        attempt_number,
        step_definition: step,
    })
}

/// Additional admission proof for a successor Call. A live worker lease alone
/// cannot authorize reusing the step's business key.
pub(crate) async fn broker_retry_input(
    tx: &Transaction<'_>,
    proof: &BrokerDispatchProof,
    lease: &AttemptLease,
    input_digest: &str,
) -> Result<()> {
    if proof.attempt_number > 1 {
        retry::validate_linked_successor(
            tx,
            proof.step_id,
            &proof.step_definition,
            lease.attempt_id,
            proof.attempt_number,
            input_digest,
        )
        .await?;
    }
    Ok(())
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
