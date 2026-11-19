//! Explicit requeue backed by a confirmed non-execution decision.
use super::{authorize_task_action, claim_authority, commit_effects};
use crate::{
    api::etag,
    auth::{Auth, Context},
    contracts::unsigned,
    error::{Error, Result},
    plugin_catalog::read_task,
};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

const MAX_ATTEMPTS: u64 = 5; // WorkflowStep, Attempt and TaskStep contracts.

// Only explicit retryTask writes this event. Its primary key is the previous
// attempt ID, so one decision can authorize at most one successor. The normal
// HTTP audit and event commit together and share the transaction timestamp.
// The audit lookup uses the subject/time index; input validation also makes
// malformed restored JSON fail closed without a UUID cast error.
const QUEUE_PROOF: &str = "
    SELECT FROM outbox q JOIN audit_events audit
      ON audit.subject_id=CASE WHEN pg_input_is_valid(q.payload->>'subject_id', 'uuid')
                               THEN (q.payload->>'subject_id')::uuid END
     AND audit.created_at=q.created_at
     WHERE q.id=prior.id AND q.aggregate_id=prior.id AND q.topic='task.step.retry_requested'
       AND audit.action='retryTask' AND audit.outcome='succeeded' AND audit.reason_code='COMMITTED'
       AND audit.resource=jsonb_build_object('kind','task','match','ids','ids',jsonb_build_array(s.task_id))
       AND q.payload=jsonb_build_object('task_id',s.task_id,'step_id',s.id,
             'attempt_id',prior.id,'fence',prior.fence,'request_id',audit.request_id,
             'subject_id',audit.subject_id,'credential_id',audit.credential_id)";

// A newer proof does not override an older execution or an unknown outcome.
// The unique step/number and step/fence constraints identify the bounded history.
const SAFE_HISTORY: &str = "
    SELECT count(*)=prior.number AND bool_and(old.number BETWEEN 1 AND prior.number
           AND old.fence BETWEEN 1 AND prior.fence AND old.idempotency_key=prior.idempotency_key
           AND old.state IN ('expired','unknown') AND coalesce(r.resolution='confirmed_not_executed',false))
      FROM attempts old LEFT JOIN task_step_resolutions r ON r.attempt_id=old.id
     WHERE old.step_id=s.id";

pub(super) async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    task: Uuid,
) -> Result<()> {
    let owner = c
        .query_opt("SELECT owner_id FROM tasks WHERE id=$1", &[&task])
        .await?
        .ok_or_else(Error::not_found)?
        .try_get(0)?;
    authorize_task_action(c, ctx, "task.retry", task, owner).await
}

pub(crate) async fn request(
    tx: &Transaction<'_>,
    auth: &Auth,
    ctx: &Context,
    task: Uuid,
    headers: &HeaderMap,
) -> Result<(Value, DateTime<Utc>)> {
    // As in settlement and cancellation, the parent fences all child changes.
    let parent = tx
        .query_opt(
            "SELECT owner_id,revision,state,deadline_at FROM tasks WHERE id=$1 FOR NO KEY UPDATE",
            &[&task],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let owner = parent.try_get("owner_id")?;
    authorize_task_action(tx, ctx, "task.retry", task, owner).await?;
    etag(headers, parent.try_get("revision")?)?;
    if parent.try_get::<_, &str>("state")? != "failed" {
        return Err(Error::conflict(
            "Only a failed, uncanceled task can be retried",
        ));
    }

    let sql = format!("SELECT s.id AS step_id,s.provider_deployment_id,s.attempt_count,s.fence,
            t.owner_id,t.workflow_id,t.admission,w.owner_id AS workflow_owner_id,
            e.subject_id,e.issuer_credential_id,e.maximum_rules,e.expires_at,
            (e.status='active' AND e.run_as='user' AND e.service_account_id IS NULL
             AND e.issuer_credential_id IS NOT NULL AND e.subject_id=t.owner_id
             AND e.expires_at>clock_timestamp() AND t.admission IS NOT NULL) AS delegation_live,
            node.definition AS step,prior.id AS prior_id
         FROM task_steps s JOIN tasks t ON t.id=s.task_id
         JOIN workflows w ON w.id=t.workflow_id JOIN execution_delegations e ON e.id=t.delegation_id
         LEFT JOIN LATERAL (SELECT value AS definition FROM jsonb_array_elements(w.spec->'steps')
              WHERE value->>'id'=s.step_key) node ON true
         LEFT JOIN attempts prior ON prior.step_id=s.id AND prior.number=s.attempt_count AND prior.fence=s.fence
           AND prior.state IN ('expired','unknown')
           AND char_length(prior.idempotency_key) BETWEEN 1 AND 128
           AND ({SAFE_HISTORY})
         WHERE s.task_id=$1 AND s.state='failed'
         ORDER BY s.id FOR UPDATE OF s FOR SHARE OF e,w");
    let candidates = tx.query(&sql, &[&task]).await?;
    if candidates.is_empty() || !history_is_settled(tx, task).await? {
        return Err(Error::conflict(
            "Task has no retryable failure or still has unknown work",
        ));
    }

    let mut cutoff: DateTime<Utc> = parent.try_get("deadline_at")?;
    for candidate in &candidates {
        let step: Value = candidate
            .try_get::<_, Option<Value>>("step")?
            .ok_or_else(unsafe_step)?;
        if candidate.try_get::<_, Option<Uuid>>("prior_id")?.is_none()
            || !within_budget(&step, candidate.try_get("attempt_count")?)?
            || !(1..9_007_199_254_740_991_i64).contains(&candidate.try_get("fence")?)
            || !dependencies_succeeded(tx, task, &step["depends_on"]).await?
        {
            return Err(unsafe_step());
        }
        if !candidate.try_get::<_, bool>("delegation_live")? {
            return Err(Error::denied());
        }
        cutoff = cutoff
            .min(candidate.try_get("expires_at")?)
            .min(claim_authority::revalidate(tx, auth, candidate, &step, ctx.request).await?);
    }
    check_cutoff(tx, cutoff).await?;

    for candidate in &candidates {
        let attempt: Uuid = candidate.try_get("prior_id")?;
        let step: Uuid = candidate.try_get("step_id")?;
        let fence: i64 = candidate.try_get("fence")?;
        let payload = json!({"task_id":task,"step_id":step,"attempt_id":attempt,"fence":fence,
                            "request_id":ctx.request,"subject_id":ctx.subject,"credential_id":ctx.credential});
        tx.execute(
            "INSERT INTO outbox(id,topic,aggregate_id,payload,created_at)
             VALUES($1,'task.step.retry_requested',$1,$2,now())",
            &[&attempt, &payload],
        )
        .await?;
        tx.execute(
            "UPDATE task_steps SET state='ready',output=NULL,revision=revision+1,updated_at=clock_timestamp()
             WHERE id=$1",
            &[&step],
        ).await?;
    }
    tx.execute(
        "UPDATE tasks SET state='queued',error=NULL,output=NULL,revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1",
        &[&task],
    ).await?;
    commit_effects::wake_dependents(tx, task, None).await?;
    let result = read_task(tx, task, None)
        .await?
        .ok_or_else(Error::not_found)?;
    Ok((result, cutoff))
}

pub(crate) async fn validate_commit(
    tx: &Transaction<'_>,
    ctx: &Context,
    task: Uuid,
    cutoff: DateTime<Utc>,
) -> Result<()> {
    authorize(tx, ctx, task).await?;
    check_cutoff(tx, cutoff).await
}

fn unsafe_step() -> Error {
    Error::conflict(
        "A failed step lacks a safe non-execution decision, live dependency, or retry budget",
    )
}

fn within_budget(step: &Value, previous: i32) -> Result<bool> {
    let maximum = step
        .get("maximum_attempts")
        .map(unsigned)
        .transpose()?
        .unwrap_or(1);
    Ok(u64::try_from(previous)
        .is_ok_and(|count| count > 0 && count < maximum && maximum <= MAX_ATTEMPTS))
}

async fn check_cutoff(tx: &Transaction<'_>, cutoff: DateTime<Utc>) -> Result<()> {
    if !super::task_engine::live(tx, cutoff).await? {
        return Err(Error::conflict(
            "The original task or execution authority is no longer live",
        ));
    }
    Ok(())
}

async fn history_is_settled(c: &(impl GenericClient + Sync), task: Uuid) -> Result<bool> {
    Ok(c.query_one(
        "SELECT NOT EXISTS (SELECT FROM task_steps WHERE task_id=$1 AND state IN ('running','unknown','canceled'))
         AND NOT EXISTS (SELECT FROM outbox WHERE aggregate_id=$1 AND topic IN ('task.cancel.requested','task.canceled'))
         AND NOT EXISTS (SELECT FROM attempts a JOIN task_steps s ON s.id=a.step_id WHERE s.task_id=$1
             AND (a.state IN ('leased','running') OR (a.state IN ('expired','unknown') AND NOT EXISTS
                 (SELECT FROM task_step_resolutions r WHERE r.attempt_id=a.id))))",
        &[&task],
    ).await?.try_get(0)?)
}

async fn dependencies_succeeded(
    tx: &Transaction<'_>,
    task: Uuid,
    dependencies: &Value,
) -> Result<bool> {
    if !dependencies.is_array() {
        return Ok(false);
    }
    Ok(tx.query_one(
        "SELECT NOT EXISTS (SELECT FROM jsonb_array_elements_text($2) dep(key)
         WHERE NOT EXISTS (SELECT FROM task_steps WHERE task_id=$1 AND step_key=dep.key AND state='succeeded'))",
        &[&task, dependencies],
    ).await?.try_get(0)?)
}

// Caller holds the task and candidate step locks. Read the prior stable key
// only after establishing both the resolution and the explicit queue request.
pub(super) async fn queued_key(
    tx: &Transaction<'_>,
    step_id: Uuid,
    step: &Value,
    previous: i32,
) -> Result<Option<String>> {
    if !within_budget(step, previous)? {
        return Ok(None);
    }
    let sql = format!("SELECT prior.idempotency_key FROM task_steps s
        JOIN attempts prior ON prior.step_id=s.id AND prior.number=s.attempt_count AND prior.fence=s.fence
        WHERE s.id=$1 AND s.state='ready' AND prior.number BETWEEN 1 AND 4
          AND prior.state IN ('expired','unknown') AND char_length(prior.idempotency_key) BETWEEN 1 AND 128
          AND ({SAFE_HISTORY})
          AND EXISTS ({QUEUE_PROOF})");
    tx.query_opt(&sql, &[&step_id])
        .await?
        .map(|row| row.try_get(0).map_err(Error::from))
        .transpose()
}

pub(super) async fn cancel_queued(c: &(impl GenericClient + Sync), task: Uuid) -> Result<u64> {
    let sql = format!("UPDATE task_steps s SET state='canceled',revision=s.revision+1,updated_at=clock_timestamp()
        FROM attempts prior JOIN task_step_resolutions r ON r.attempt_id=prior.id
        WHERE s.task_id=$1 AND s.state='ready' AND prior.step_id=s.id AND prior.number=s.attempt_count
          AND prior.fence=s.fence AND prior.state IN ('expired','unknown') AND r.resolution='confirmed_not_executed'
          AND NOT EXISTS (SELECT FROM attempts a WHERE a.step_id=s.id AND a.state IN ('leased','running'))
          AND EXISTS ({QUEUE_PROOF})");
    Ok(c.execute(&sql, &[&task]).await?)
}

pub(super) async fn inconsistent_snapshot(c: &(impl GenericClient + Sync)) -> Result<bool> {
    let sql = format!("SELECT EXISTS (SELECT FROM task_steps s
       LEFT JOIN attempts prior ON prior.step_id=s.id AND prior.number=s.attempt_count AND prior.fence=s.fence
       LEFT JOIN task_step_resolutions r ON r.attempt_id=prior.id AND r.resolution='confirmed_not_executed'
       WHERE ((s.state='ready' AND (s.attempt_count<>0 OR EXISTS(SELECT FROM attempts a WHERE a.step_id=s.id)))
                   OR (s.state='canceled' AND r.attempt_id IS NOT NULL))
         AND (r.attempt_id IS NULL OR s.output IS NOT NULL OR prior.number NOT BETWEEN 1 AND 4
              OR prior.fence NOT BETWEEN 1 AND 9007199254740990
              OR char_length(prior.idempotency_key) NOT BETWEEN 1 AND 128 OR NOT ({SAFE_HISTORY})
              OR NOT EXISTS ({QUEUE_PROOF})))
       OR EXISTS (SELECT FROM attempts successor JOIN task_steps s ON s.id=successor.step_id
           WHERE successor.worker_id IS NOT NULL AND successor.number>1 AND NOT EXISTS (
               SELECT FROM attempts prior JOIN task_step_resolutions r ON r.attempt_id=prior.id
               WHERE prior.step_id=s.id AND prior.number=successor.number-1 AND prior.fence=successor.fence-1
                 AND prior.idempotency_key=successor.idempotency_key AND r.resolution='confirmed_not_executed'
                 AND EXISTS ({QUEUE_PROOF})))");
    Ok(c.query_one(&sql, &[]).await?.try_get(0)?)
}
