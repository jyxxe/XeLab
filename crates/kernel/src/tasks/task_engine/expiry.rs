//! Expire worker claims without inferring their external outcome.
use super::{commit_effects, ENGINE_TIMEOUT};
use crate::{
    db::{try_auth_lock_shared, Db},
    error::{Error, Result},
};
use uuid::Uuid;

const EXPIRY_BATCH_SIZE: usize = 16;

// Lock the parent first, in the same order as worker renew/commit. A new
// statement after acquiring it sees any renewal that won the lock first.
const NEXT_EXPIRED: &str = "
    SELECT a.id, t.id AS task_id
      FROM attempts a JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
     WHERE a.state IN ('leased','running') AND a.worker_id IS NOT NULL
       AND s.state='running' AND s.fence=a.fence AND s.attempt_count=a.number
       AND a.fence BETWEEN 1 AND 9007199254740991
       AND t.state IN ('running','waiting_dependency','paused_authorization','cancel_requested')
       AND least(a.lease_expires_at,t.deadline_at)<=statement_timestamp()
       AND NOT (a.id=ANY($1::uuid[]))
       AND NOT EXISTS (SELECT FROM platform_recovery_state WHERE phase<>'ready')
     ORDER BY least(a.lease_expires_at,t.deadline_at),a.id LIMIT 1 FOR NO KEY UPDATE OF t SKIP LOCKED";

const EXPIRE_CURRENT: &str = "
    WITH current_attempt AS MATERIALIZED (
        SELECT a.id, s.id AS step_id, t.state AS task_state,
               least(a.lease_expires_at,t.deadline_at) AS cutoff
          FROM attempts a JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
         WHERE a.id=$1 AND t.id=$2 AND a.state IN ('leased','running') AND a.worker_id IS NOT NULL
           AND s.state='running' AND s.fence=a.fence AND s.attempt_count=a.number
           AND a.fence BETWEEN 1 AND 9007199254740991
           AND t.state IN ('running','waiting_dependency','paused_authorization','cancel_requested')
         FOR UPDATE OF a,s SKIP LOCKED
    )
    UPDATE attempts a SET state='expired',revision=a.revision+1,updated_at=clock_timestamp()
      FROM current_attempt p WHERE a.id=p.id AND clock_timestamp()>=p.cutoff
    RETURNING p.step_id,p.task_state";

pub(crate) async fn expire_leases(db: &Db) -> Result<()> {
    tokio::time::timeout(ENGINE_TIMEOUT, expire_batch(db))
        .await
        .map_err(|_| Error::unavailable("Task lease reconciliation timed out"))?
}

async fn expire_batch(db: &Db) -> Result<()> {
    let mut client = db.connect_scoped(ENGINE_TIMEOUT).await?;
    let mut skipped: Vec<Uuid> = Vec::new();
    for _ in 0..EXPIRY_BATCH_SIZE {
        let tx = client.transaction().await?;
        // Serialize with offline recovery and preserve the engine lock order.
        if !try_auth_lock_shared(&tx).await? {
            tx.rollback().await?;
            break;
        }
        let Some(candidate) = tx.query_opt(NEXT_EXPIRED, &[&skipped]).await? else {
            tx.rollback().await?;
            break;
        };
        let attempt_id: Uuid = candidate.get("id");
        let task_id: Uuid = candidate.get("task_id");
        let Some(expired) = tx
            .query_opt(EXPIRE_CURRENT, &[&attempt_id, &task_id])
            .await?
        else {
            tx.rollback().await?;
            skipped.push(attempt_id);
            continue;
        };
        commit_effects::expire_step(
            &tx,
            attempt_id,
            expired.get("step_id"),
            task_id,
            expired.get::<_, &str>("task_state") == "running",
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
