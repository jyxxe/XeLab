//! Cross-table consistency for offline task and batch metadata. Recovery revokes
//! active delegations; importing this history never restores execution authority.
use super::*;

pub(crate) async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    let inconsistent: bool = c
        .query_one(
            "SELECT EXISTS (
                 SELECT FROM attempts a JOIN task_steps s ON s.id=a.step_id
                   JOIN tasks t ON t.id=s.task_id
                  WHERE a.state IN ('leased','running')
                    AND (s.state IN ('succeeded','failed','unknown','canceled')
                      OR t.state IN ('succeeded','failed','unknown','canceled'))
             ) OR EXISTS (
                 SELECT FROM skill_batch_items i JOIN skill_batches b ON b.id=i.batch_id
                  WHERE i.state='running' AND b.state IN ('succeeded','partially_succeeded','failed','canceled')
             ) OR EXISTS (
                 SELECT FROM attempts a LEFT JOIN attempt_commit_receipts r ON r.attempt_id=a.id
                  WHERE (r.attempt_id IS NOT NULL AND (a.worker_id IS NULL OR r.state<>a.state))
                     OR (a.worker_id IS NOT NULL AND a.state IN ('succeeded','failed','unknown')
                         AND r.attempt_id IS NULL)
             )",
            &[],
        )
        .await?
        .get(0);
    if inconsistent {
        return Err(Error::conflict(
            "Snapshot contains inconsistent task or batch execution history",
        ));
    }
    Ok(())
}
