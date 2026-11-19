//! Broker request preparation inside the worker's locked, authorized transaction.
mod mapping;
#[cfg(test)]
mod tests;

use crate::{
    contracts::{id, string, unsigned, validate},
    error::{Error, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio_postgres::Transaction;
use uuid::Uuid;

/// Prepared business data for an in-process worker. Actual dispatch requires
/// the live attempt, its original authority and its remaining hard deadline.
#[derive(Debug)]
pub struct DispatchInput {
    /// Selects the Skill invocation path. A bare capability carries its contract in the body.
    pub skill_id: Option<Uuid>,
    /// The task's admitted deployment, also encoded in the Broker body when pinned.
    pub provider_deployment_id: Option<Uuid>,
    /// InvokeRequest or SkillInvokeRequest. The key is stable across attempts.
    /// timeout_ms is the static RPC ceiling; prepare_dispatch owns the final time bound.
    pub request: Value,
    /// Absolute step, task and revalidated authority cutoff from prepare_dispatch.
    /// Broker admission and queue exit must enforce it alongside the live lease.
    /// Kept outside the fingerprinted request; it conveys no authority.
    pub deadline_at: DateTime<Utc>,
}

/// The caller holds the parent task, current step and active attempt locks and
/// has revalidated the pinned Skill, provider and original authorization paths.
pub(super) async fn build(
    tx: &Transaction<'_>,
    step_id: Uuid,
    deadline_at: DateTime<Utc>,
) -> Result<DispatchInput> {
    // The API accepts up to 4 MiB of compact workflow JSON. Allow room for
    // jsonb's text separators; measuring compressed TOAST storage is insufficient.
    let row = tx
        .query_one(
            "SELECT s.task_id,s.step_key,s.provider_deployment_id,a.idempotency_key,
                CASE WHEN octet_length(w.spec::text)<=8388608 THEN w.spec END AS spec
           FROM task_steps s JOIN tasks t ON s.task_id=t.id JOIN workflows w ON w.id=t.workflow_id
           JOIN attempts a ON a.step_id=s.id AND a.fence=s.fence AND a.number=s.attempt_count
          WHERE s.id=$1",
            &[&step_id],
        )
        .await?;
    let spec = row
        .try_get::<_, Option<Value>>("spec")?
        .ok_or_else(Error::exhausted)?;
    // Reuse publication's strict mapping grammar, pointer escape checks and
    // transitive ancestry validation before looking up any predecessor output.
    validate("WorkflowSpec", &spec)?;
    let key: &str = row.try_get("step_key")?;
    let step = spec["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == key)
        .ok_or_else(|| Error::invalid("Step is missing from its task workflow"))?;
    let skill_id = step
        .get("skill_id")
        .map(|_| id(step, "skill_id"))
        .transpose()?;
    let provider = row.try_get("provider_deployment_id")?;
    let mut timeout = unsigned(&step["timeout_ms"])?.min(300_000);
    let mut request = json!({"idempotency_key":row.try_get::<_, &str>("idempotency_key")?});
    let (schema, capability_name) = if let Some(skill) = skill_id {
        let row = tx
            .query_one(
                "SELECT definition->'maximum_timeout_ms',
                    CASE WHEN definition->'target'->>'kind'='capability'
                         THEN definition->'target'->'contract'->>'name' END
               FROM skills WHERE id=$1",
                &[&skill],
            )
            .await?;
        timeout = timeout.min(unsigned(&row.try_get::<_, Value>(0)?)?);
        ("SkillInvokeRequest", row.try_get::<_, Option<String>>(1)?)
    } else {
        request["contract"] = step["contract"].clone();
        (
            "InvokeRequest",
            Some(string(&step["contract"], "name")?.to_owned()),
        )
    };
    if capability_name.as_deref() == Some("automation.execute") {
        return Err(Error::unavailable(
            "Workflow target requires TaskWorker.Execute",
        ));
    }
    if let Some(deployment) = provider {
        request["provider_deployment_id"] = json!(deployment);
    }
    request["timeout_ms"] = json!(timeout);
    request["input"] =
        mapping::resolve(tx, row.try_get("task_id")?, &step["input_mapping"]).await?;
    validate(schema, &request)?;
    Ok(DispatchInput {
        skill_id,
        provider_deployment_id: provider,
        request,
        deadline_at,
    })
}
