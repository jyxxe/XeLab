//! Trusted, in-process TaskEngine operations.
mod active;
mod commit;
mod dispatch;
mod expiry;
mod lease;
pub use super::dispatch_input::DispatchInput;
pub use crate::broker::dispatch_task as dispatch;
pub(super) use active::live;
pub use commit::commit;
pub(crate) use dispatch::{
    broker_dispatch_proof, broker_input, broker_paths_until, broker_remaining, BrokerDispatchProof,
    BrokerLeaseUse,
};
pub(crate) use expiry::expire_leases;
pub use lease::{prepare_dispatch, renew, AttemptLease};

use super::{claim_authority, commit_effects, retry};
use crate::{
    auth::Auth,
    contracts::unsigned,
    db::{auth_lock, Db},
    error::{Error, Result},
    plugin_catalog::attempt_value,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

const ENGINE_TIMEOUT: Duration = Duration::from_secs(10);

const READY_STEP: &str = "
    SELECT s.id AS step_id, s.task_id, s.fence, s.attempt_count, s.provider_deployment_id,
           t.owner_id, t.workflow_id, t.deadline_at, t.admission, w.owner_id AS workflow_owner_id,
           e.subject_id, e.issuer_credential_id, e.maximum_rules, e.expires_at,
           (e.status='active' AND e.run_as='user' AND e.service_account_id IS NULL
              AND e.issuer_credential_id IS NOT NULL AND e.subject_id=t.owner_id
              AND e.expires_at>clock_timestamp()) AS delegation_live,
           node.definition AS step
      FROM task_steps s JOIN tasks t ON t.id=s.task_id
      JOIN workflows w ON w.id=t.workflow_id
      JOIN execution_delegations e ON e.id=t.delegation_id
      CROSS JOIN LATERAL jsonb_array_elements(w.spec->'steps') node(definition)
     WHERE s.state='ready' AND s.attempt_count BETWEEN 0 AND 4 AND s.fence BETWEEN 0 AND 9007199254740990
       AND NOT (s.id=ANY($1::uuid[]))
       AND t.state IN ('queued','running') AND t.deadline_at>clock_timestamp()
       AND t.admission IS NOT NULL AND node.definition->>'id'=s.step_key
       AND jsonb_typeof(node.definition->'depends_on')='array'
       AND NOT EXISTS (SELECT FROM task_steps stopped WHERE stopped.task_id=t.id AND stopped.state IN ('failed','unknown'))
       AND ((s.attempt_count=0 AND NOT EXISTS (SELECT FROM attempts a WHERE a.step_id=s.id))
         OR (s.attempt_count>0 AND EXISTS (
             SELECT FROM attempts a JOIN outbox q ON q.id=a.id AND q.topic='task.step.retry_requested'
             WHERE a.step_id=s.id AND a.number=s.attempt_count AND a.fence=s.fence)))
       AND NOT EXISTS (
           SELECT FROM jsonb_array_elements_text(node.definition->'depends_on') dependency(key)
           WHERE NOT EXISTS (SELECT FROM task_steps upstream
                              WHERE upstream.task_id=t.id AND upstream.step_key=dependency.key
                                AND upstream.state='succeeded'))
       AND NOT EXISTS (SELECT FROM platform_recovery_state WHERE phase<>'ready')
     ORDER BY s.created_at,s.id LIMIT 1
       FOR UPDATE OF s SKIP LOCKED FOR NO KEY UPDATE OF t SKIP LOCKED FOR SHARE OF e,w SKIP LOCKED";

/// Claim a first or explicitly requeued attempt for a trusted kernel worker. The lease is bounded by
/// the original authority, the step timeout, the task deadline and 30 seconds.
/// The worker must revalidate authority and the lease before dispatch.
pub async fn claim(db: &Db, auth: &Auth, worker_id: Uuid) -> Result<Option<Value>> {
    tokio::time::timeout(ENGINE_TIMEOUT, claim_ready(db, auth, worker_id))
        .await
        .map_err(|_| Error::unavailable("Task claim timed out"))?
}

async fn claim_ready(db: &Db, auth: &Auth, worker_id: Uuid) -> Result<Option<Value>> {
    let mut client = db.connect_scoped(ENGINE_TIMEOUT).await?;
    let mut skipped_steps: Vec<Uuid> = Vec::new();
    loop {
        let tx = client.transaction().await?;
        auth_lock(&tx, false).await?;
        let Some(candidate) = tx.query_opt(READY_STEP, &[&skipped_steps]).await? else {
            tx.rollback().await?;
            return Ok(None);
        };
        let step: Value = candidate.get("step");
        let task_id: Uuid = candidate.get("task_id");
        let step_id: Uuid = candidate.get("step_id");
        let previous: i32 = candidate.get("attempt_count");
        if !still_claimable(&tx, task_id, &step["depends_on"]).await? {
            tx.rollback().await?;
            skipped_steps.push(step_id);
            continue;
        }
        let stable_key = if previous == 0 {
            step_id.to_string()
        } else if let Some(key) = retry::queued_key(&tx, step_id, &step, previous).await? {
            key
        } else {
            tx.rollback().await?;
            skipped_steps.push(step_id);
            continue;
        };
        let authority = if candidate.get("delegation_live") {
            claim_authority::revalidate(&tx, auth, &candidate, &step, worker_id).await
        } else {
            Err(Error::denied())
        };
        let authorized_until = match authority {
            Ok(expiry) => expiry,
            Err(error) => {
                let state = match error.code {
                    "PERMISSION_DENIED" | "INVALID_CREDENTIAL" => "paused_authorization",
                    "TASK_PROVIDER_UNAVAILABLE" => "waiting_dependency",
                    _ => return Err(error),
                };
                suspend(&tx, task_id, state).await?;
                tx.commit().await?;
                continue;
            }
        };
        let expiry = authorized_until
            .min(candidate.get("expires_at"))
            .min(candidate.get("deadline_at"));
        let timeout_ms = unsigned(&step["timeout_ms"])?;
        let Some(attempt) =
            lease(&tx, &candidate, worker_id, expiry, timeout_ms, &stable_key).await?
        else {
            // Issuance crossed the authority cutoff or the task deadline.
            suspend(&tx, task_id, "paused_authorization").await?;
            tx.commit().await?;
            continue;
        };
        let value = attempt_value(&attempt)?;
        let expires: DateTime<Utc> = attempt.get("lease_expires_at");
        // Start the timer before sampling the database clock so the remaining
        // lifetime also covers query latency and the commit acknowledgement.
        let checked = Instant::now();
        let now: DateTime<Utc> = tx.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
        let remaining = (expires - now).to_std().unwrap_or_default();
        if checked.elapsed() >= remaining {
            tx.rollback().await?;
            skipped_steps.push(step_id);
            continue;
        }
        tx.commit().await?;
        // A committed attempt stays in history even when its lease elapsed
        // while waiting for COMMIT. Never hand that fence to a worker.
        return Ok((checked.elapsed() < remaining).then_some(value));
    }
}

// READY_STEP uses a pre-lock snapshot. Under the task lock check for a failed
// sibling again and lock the chosen child's predecessors through commit.
async fn still_claimable(
    tx: &Transaction<'_>,
    task_id: Uuid,
    dependencies: &Value,
) -> Result<bool> {
    let row = tx
        .query_one(
            "WITH required AS (
                 SELECT DISTINCT step_key FROM jsonb_array_elements_text($2) AS dep(step_key)
             ), locked AS (
                 SELECT s.id FROM task_steps s JOIN required USING (step_key)
                 WHERE s.task_id=$1 AND s.state='succeeded'
                 ORDER BY s.id FOR SHARE OF s SKIP LOCKED
             )
             SELECT (SELECT count(*) FROM locked) = (SELECT count(*) FROM required)
                AND NOT EXISTS (SELECT FROM task_steps WHERE task_id=$1 AND state IN ('failed','unknown'))",
            &[&task_id, dependencies],
        )
        .await?;
    Ok(row.get(0))
}

async fn suspend(tx: &Transaction<'_>, task_id: Uuid, state: &str) -> Result<()> {
    tx.execute(
        "UPDATE tasks SET state=$2,revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1 AND deadline_at>clock_timestamp()",
        &[&task_id, &state],
    )
    .await?;
    Ok(())
}

async fn lease(
    tx: &Transaction<'_>,
    candidate: &Row,
    worker_id: Uuid,
    expiry: DateTime<Utc>,
    timeout_ms: u64,
    stable_key: &str,
) -> Result<Option<Row>> {
    let step_id: Uuid = candidate.get("step_id");
    let task_id: Uuid = candidate.get("task_id");
    let fence = candidate.get::<_, i64>("fence") + 1;
    let number = candidate.get::<_, i32>("attempt_count") + 1;
    let lease_ms = timeout_ms.min(30_000) as i32;
    let attempt = tx
        .query_opt(
            "WITH issued AS MATERIALIZED (SELECT clock_timestamp() AS at)
         INSERT INTO attempts AS a(id,step_id,number,execution_id,idempotency_key,fence,
                                   lease_expires_at,state,created_at,updated_at,worker_id)
         SELECT $1,$2,$3,$4,$5,$6,least($7,at+$8::int*interval '1 millisecond'),'leased',at,at,$9
           FROM issued WHERE at<$7
         RETURNING to_jsonb(a) AS attempt,created_at,updated_at,lease_expires_at",
            &[
                &Uuid::new_v4(),
                &step_id,
                &number,
                &Uuid::new_v4(),
                &stable_key,
                &fence,
                &expiry,
                &lease_ms,
                &worker_id,
            ],
        )
        .await?;
    if let Some(attempt) = &attempt {
        let issued: DateTime<Utc> = attempt.get("created_at");
        tx.execute(
            "UPDATE task_steps SET state='running',attempt_count=$2,fence=$3,revision=revision+1,updated_at=$4 WHERE id=$1",
            &[&step_id, &number, &fence, &issued],
        ).await?;
        tx.execute(
            "UPDATE tasks SET state='running',revision=revision+1,updated_at=$2 WHERE id=$1 AND state='queued'",
            &[&task_id, &issued],
        ).await?;
    }
    Ok(attempt)
}
