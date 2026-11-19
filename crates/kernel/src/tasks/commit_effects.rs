//! Business changes made under the caller's task, step, attempt and authority locks.
use crate::{
    api::object_value,
    contracts::{id, string, unsigned, validate_business},
    db::outbox,
    error::{Error, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::io::{self, Write};
use tokio_postgres::Transaction;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)] // Identities come from the locked rows; the result is validated by the caller.
pub(super) async fn apply(
    tx: &Transaction<'_>,
    attempt_id: Uuid,
    step_id: Uuid,
    task_id: Uuid,
    workflow_step: &Value,
    state: &str,
    output: &Value,
    error: &Value,
) -> Result<Value> {
    let key = string(workflow_step, "id")?;
    if state == "succeeded" {
        validate_output(tx, step_id, workflow_step, output).await?;
    }

    let changed = tx
        .execute(
            "UPDATE attempts SET state=$3, revision=revision+1, updated_at=clock_timestamp()
         WHERE id=$1 AND step_id=$2 AND state IN ('leased','running')",
            &[&attempt_id, &step_id, &state],
        )
        .await?;
    if changed != 1 {
        return Err(Error::conflict("Attempt is no longer active"));
    }
    let changed = tx.execute(
        "UPDATE task_steps SET state=$4, output=$5, revision=revision+1, updated_at=clock_timestamp()
          WHERE id=$1 AND task_id=$2 AND step_key=$3 AND state='running'",
        &[&step_id, &task_id, &key, &state, &(state == "succeeded").then_some(output)],
    ).await?;
    if changed != 1 {
        return Err(Error::conflict("Step is no longer running"));
    }
    let can_wake = aggregate(tx, task_id, step_id, state, error).await?;
    outbox(
        tx,
        "task.step.committed",
        step_id,
        &json!({"task_id":task_id,"step_id":step_id,"attempt_id":attempt_id,"state":state}),
    )
    .await?;
    if can_wake {
        wake_dependents(tx, task_id, Some(key)).await?;
    }
    read_step(tx, step_id).await
}

/// Project the public TaskStep for both a new commit and its acknowledgement.
/// Typed timestamps give a stable UTC representation across PostgreSQL sessions.
pub(super) async fn read_step(tx: &Transaction<'_>, step_id: Uuid) -> Result<Value> {
    let row = tx.query_one(
        "SELECT to_jsonb(s),s.created_at,s.updated_at FROM (
             SELECT id,task_id,step_key,provider_deployment_id,attempt_count,state,revision,created_at,updated_at
               FROM task_steps WHERE id=$1
         ) s",
        &[&step_id],
    ).await?;
    let mut step: Value = row.try_get(0)?;
    for field in ["created_at", "updated_at"] {
        step[field] = json!(row.try_get::<_, DateTime<Utc>>(field)?);
    }
    object_value(&step, "TaskStep")
}

/// Record an unknown result for the step of a locked, expired attempt.
pub(super) async fn expire_step(
    tx: &Transaction<'_>,
    attempt_id: Uuid,
    step_id: Uuid,
    task_id: Uuid,
    aggregate_running_task: bool,
) -> Result<()> {
    let changed = tx
        .execute(
            "UPDATE task_steps SET state='unknown',output=NULL,revision=revision+1,updated_at=clock_timestamp()
             WHERE id=$1 AND task_id=$2 AND state='running'",
            &[&step_id, &task_id],
        )
        .await?;
    if changed != 1 {
        return Err(Error::conflict(
            "Expired attempt no longer has a running step",
        ));
    }
    let error = Error::new(
        "OUTCOME_UNKNOWN",
        "Task claim expired before the result was committed",
        axum::http::StatusCode::CONFLICT,
    );
    if aggregate_running_task {
        aggregate(tx, task_id, step_id, "unknown", &error.detail()).await?;
    }
    outbox(
        tx,
        "task.step.lease_expired",
        step_id,
        &json!({"task_id":task_id,"step_id":step_id,"attempt_id":attempt_id,"state":"unknown","reason_code":error.code}),
    )
    .await
}

async fn validate_output(
    tx: &Transaction<'_>,
    step_id: Uuid,
    step: &Value,
    output: &Value,
) -> Result<()> {
    let mut contract = &step["contract"];
    let definition: Value;
    if step.get("skill_id").is_some() {
        // Revalidation holds these exact Skill/deployment/installation rows and
        // verified the signed candidate (including the HTTP operation, if any).
        definition = tx.query_opt(
            "SELECT k.definition FROM task_steps s JOIN deployments d ON d.id=s.provider_deployment_id
             JOIN skills k ON k.installation_id=d.installation_id WHERE s.id=$1 AND k.id=$2",
            &[&step_id, &id(step,"skill_id")?],
        ).await?.ok_or_else(|| Error::conflict("Skill output binding is unavailable"))?.try_get(0)?;
        if definition["target"]["kind"] == "http" {
            let maximum = if definition["stream_item_schema"].is_null() {
                crate::plugin_http::MAXIMUM_RESPONSE_BYTES
            } else {
                // The final output is inside one bounded HTTP SSE frame.
                1024 * 1024
            };
            check_output_bytes(output, maximum as u64)?;
            return validate_business(&definition["output_schema"], output);
        }
        // The revalidated signed capability Skill has identical schemas to
        // its provider. Check the provider's limit before schema evaluation.
        contract = &definition["target"]["contract"];
    }
    // Only unbound stateless steps use the registered contract. For a pinned
    // deployment, use its signed manifest, even if other providers now exist.
    let capability: Value = tx
        .query_opt(
            "SELECT CASE WHEN s.provider_deployment_id IS NULL THEN c.spec ELSE cap.definition END
         FROM task_steps s
         JOIN capability_contracts c ON c.name=$2 AND c.version=$3 AND c.schema_digest=$4
         LEFT JOIN deployments d ON d.id=s.provider_deployment_id
         LEFT JOIN installations i ON i.id=d.installation_id
         LEFT JOIN LATERAL jsonb_array_elements(i.manifest->'capabilities') cap(definition)
             ON cap.definition->'contract'=$5
         WHERE s.id=$1 AND (s.provider_deployment_id IS NULL OR cap.definition IS NOT NULL)
         FOR SHARE OF c",
            &[
                &step_id,
                &string(contract, "name")?,
                &string(contract, "version")?,
                &string(contract, "schema_digest")?,
                contract,
            ],
        )
        .await?
        .ok_or_else(|| Error::conflict("Capability output binding is unavailable"))?
        .try_get(0)?;
    check_output_bytes(output, unsigned(&capability["maximum_output_bytes"])?)?;
    validate_business(&capability["output_schema"], output)
}

pub(super) fn check_output_bytes(output: &Value, maximum: u64) -> Result<()> {
    // Count compact UTF-8 JSON, including escaping, without allocating a
    // second copy of an oversized result. Stop as soon as its budget runs out.
    struct Remaining(u64);
    impl Write for Remaining {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len() as u64)
                .ok_or_else(|| io::Error::other("Output limit exceeded"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    serde_json::to_writer(Remaining(maximum), output)
        .map_err(|_| Error::invalid("Step output exceeds the provider limit"))
}

/// Update the task, returning whether this success may unblock children.
async fn aggregate(
    tx: &Transaction<'_>,
    task_id: Uuid,
    step_id: Uuid,
    state: &str,
    error: &Value,
) -> Result<bool> {
    // All claim/commit transactions for this task serialize on its parent row.
    // An expired but unreconciled leased attempt is still active in the ledger.
    let facts = tx
        .query_one(
            "SELECT bool_or(s.id<>$2 AND s.state='unknown') AS prior_unknown,
                bool_or(s.id<>$2 AND s.state='failed') AS prior_failed,
                bool_and(s.state='succeeded') AS all_succeeded,
                EXISTS (SELECT FROM attempts a JOIN task_steps other ON a.step_id=other.id
                         WHERE other.task_id=$1 AND a.state IN ('leased','running')) AS live,
                (SELECT state FROM tasks WHERE id=$1) AS task_state
         FROM task_steps s WHERE s.task_id=$1",
            &[&task_id, &step_id],
        )
        .await?;
    if facts.try_get::<_, &str>("task_state")? == "cancel_requested" {
        if super::cancel::settled(tx, task_id).await? {
            super::cancel::update_state(tx, task_id, "canceled").await?;
        }
        return Ok(false);
    }
    let prior_unknown: bool = facts.try_get("prior_unknown")?;
    let prior_failed: bool = facts.try_get("prior_failed")?;
    let unknown = prior_unknown || state == "unknown";
    let failed = prior_failed || state == "failed";
    let task_state = if facts.try_get("live")? {
        "running"
    } else if unknown {
        "unknown"
    } else if failed {
        "failed"
    } else if facts.try_get("all_succeeded")? {
        "succeeded"
    } else {
        "running"
    };
    // Retain the first error at the highest severity, including during drain.
    let replace_error =
        !prior_unknown && (state == "unknown" || (state == "failed" && !prior_failed));
    if task_state == "succeeded" {
        complete_task(tx, task_id).await?;
    } else if replace_error || task_state != "running" {
        tx.execute(
            "UPDATE tasks SET state=$2, error=CASE WHEN $3 THEN NULLIF($4::jsonb,'null'::jsonb) ELSE error END,
                    output=NULL, revision=revision+1, updated_at=clock_timestamp()
             WHERE id=$1 AND state='running'",
            &[&task_id, &task_state, &replace_error, error],
        )
        .await?;
    }
    Ok(state == "succeeded" && task_state == "running" && !unknown && !failed)
}

pub(super) async fn complete_task(tx: &Transaction<'_>, task_id: Uuid) -> Result<()> {
    // WorkflowSpec has no explicit output mapping. Select DAG sinks by
    // dependency key so parallel completion order cannot affect the result.
    tx.execute(
        "WITH nodes AS (
             SELECT node FROM tasks t JOIN workflows w ON w.id=t.workflow_id,
                  jsonb_array_elements(w.spec->'steps') node WHERE t.id=$1
         ), sinks AS (
             SELECT s.id FROM task_steps s WHERE s.task_id=$1 AND
             NOT EXISTS (SELECT FROM nodes WHERE (node->'depends_on') ? s.step_key) LIMIT 2
         )
         UPDATE tasks SET state='succeeded',error=NULL,output=(
             SELECT s.output FROM sinks JOIN task_steps s USING (id) WHERE (SELECT count(*) FROM sinks)=1
         ),revision=revision+1,updated_at=clock_timestamp() WHERE id=$1 AND state IN ('running','unknown')",
        &[&task_id],
    )
    .await?;
    Ok(())
}

pub(super) async fn wake_dependents(
    tx: &Transaction<'_>,
    task_id: Uuid,
    upstream: Option<&str>,
) -> Result<()> {
    let ready = tx.query(
        "UPDATE task_steps s SET state='ready',revision=s.revision+1,updated_at=clock_timestamp()
         FROM tasks t JOIN workflows w ON w.id=t.workflow_id,
              jsonb_array_elements(w.spec->'steps') node
         WHERE t.id=$1 AND t.state IN ('queued','running') AND s.task_id=t.id AND s.step_key=node->>'id'
           AND s.state='pending' AND s.attempt_count=0 AND ($2::text IS NULL OR (node->'depends_on') ? $2)
           AND NOT EXISTS (SELECT FROM attempts a WHERE a.step_id=s.id)
           AND NOT EXISTS (SELECT FROM jsonb_array_elements_text(node->'depends_on') dep(key)
                            WHERE NOT EXISTS (SELECT FROM task_steps p WHERE p.task_id=t.id
                                               AND p.step_key=dep.key AND p.state='succeeded'))
         RETURNING s.id",
        &[&task_id, &upstream],
    ).await?;
    for row in ready {
        let step_id: Uuid = row.try_get(0)?;
        outbox(
            tx,
            "task.step.ready",
            step_id,
            &json!({"task_id":task_id,"step_id":step_id,"state":"ready"}),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
