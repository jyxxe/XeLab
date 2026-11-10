//! One compact maintenance slot, with bounded handoff between supervisors and
//! stable pagination within each persisted queue. No task waits on this gate.
use crate::error::Result;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::{Client, Row};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Backup,
    Upgrade,
    Schema,
    Cleanup,
}

pub(crate) struct Permit {
    permit: Option<OwnedSemaphorePermit>,
    gate: Option<Arc<Gate>>,
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            // Release and start the next polling window under the same mutex
            // used by admission, so a new arrival cannot jump queued kinds.
            let mut waiting = gate.waiting.lock().unwrap_or_else(|e| e.into_inner());
            drop(self.permit.take());
            waiting.since = tokio::time::Instant::now();
        }
    }
}

struct Waiting {
    // At most one marker for each of four fixed supervisors; never job data.
    kinds: VecDeque<Kind>,
    since: tokio::time::Instant,
}
struct Gate {
    slot: Arc<Semaphore>,
    waiting: Mutex<Waiting>,
}
impl Gate {
    fn new() -> Self {
        Self {
            slot: Arc::new(Semaphore::new(1)),
            waiting: Mutex::new(Waiting {
                kinds: VecDeque::with_capacity(4),
                since: tokio::time::Instant::now(),
            }),
        }
    }
    fn try_start(self: &Arc<Self>, compact: bool, kind: Kind) -> Option<Permit> {
        if !compact {
            return Some(Permit {
                permit: None,
                gate: None,
            });
        }
        let mut waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        if !waiting.kinds.contains(&kind) {
            waiting.kinds.push_back(kind);
        }
        if self.slot.available_permits() == 0 {
            return None;
        }
        // A kind can lose its last pending job or stop polling. Its handoff
        // expires after two compact polling intervals; it cannot strand work.
        if waiting.kinds.front() != Some(&kind) && waiting.since.elapsed() >= Duration::from_secs(2)
        {
            waiting.kinds.pop_front();
            waiting.since = tokio::time::Instant::now();
        }
        if waiting.kinds.front() != Some(&kind) {
            return None;
        }
        let permit = self.slot.clone().try_acquire_owned().ok()?;
        waiting.kinds.pop_front();
        Some(Permit {
            permit: Some(permit),
            gate: Some(self.clone()),
        })
    }
}

/// Hold through detached processing and error cleanup, including when an HTTP
/// waiter or supervisor receiver disappears.
pub(crate) fn try_start(compact: bool, kind: Kind) -> Option<Permit> {
    static GATE: OnceLock<Arc<Gate>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(Gate::new()))
        .try_start(compact, kind)
}

#[derive(Default)]
pub(crate) struct Queue {
    after: Option<Uuid>,
}
impl Queue {
    pub(crate) fn attempted(&mut self, operation: Uuid) {
        self.after = Some(operation);
    }

    pub(crate) async fn candidates(
        &mut self,
        c: &Client,
        kind: Kind,
        compact: bool,
    ) -> Result<Vec<Row>> {
        let (table, predicate) = match kind {
            Kind::Backup => ("storage_recovery_jobs", "state NOT IN ('succeeded','failed') OR EXISTS(SELECT 1 FROM deployment_storage_holds h WHERE h.operation_id=j.operation_id)"),
            Kind::Upgrade => ("deployment_upgrade_jobs", "state NOT IN ('succeeded','failed') OR EXISTS(SELECT 1 FROM deployment_storage_holds h WHERE h.operation_id=j.operation_id)"),
            Kind::Schema => ("volume_schema_jobs", "kind='bootstrap' AND (state NOT IN ('verified','failed') OR EXISTS(SELECT 1 FROM deployment_storage_holds h WHERE h.operation_id=j.operation_id))"),
            Kind::Cleanup => unreachable!("upload cleanup has no job queue"),
        };
        if !compact {
            return Ok(c.query(&format!("SELECT operation_id,deployment_id FROM {table} j WHERE {predicate} ORDER BY updated_at,operation_id LIMIT 4"), &[]).await?);
        }
        // UUID order does not change when a failing job rewrites updated_at.
        // Advance only after acquiring the slot; wrap without reading all jobs.
        let sql = format!("SELECT operation_id,deployment_id FROM {table} j WHERE ({predicate}) AND ($1::uuid IS NULL OR operation_id>$1) ORDER BY operation_id LIMIT 4");
        let rows = c.query(&sql, &[&self.after]).await?;
        if rows.is_empty() && self.after.is_some() {
            self.after = None;
            return Ok(c.query(&sql, &[&self.after]).await?);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn maintenance_slot_survives_dropped_waiter_until_job_cleanup_finishes() {
        let gate = Arc::new(Gate::new());
        let permit = gate.try_start(true, Kind::Backup).unwrap();
        let (cleanup, cleanup_complete) = tokio::sync::oneshot::channel();
        let (result, waiter) = tokio::sync::oneshot::channel();
        let owner = tokio::spawn(async move {
            let _permit = permit;
            cleanup_complete.await.unwrap();
            let _ = result.send(());
        });
        drop(waiter);
        assert!(gate.try_start(true, Kind::Upgrade).is_none());
        assert!(gate.try_start(false, Kind::Backup).is_some());
        cleanup.send(()).unwrap();
        owner.await.unwrap();
        assert!(gate.try_start(true, Kind::Upgrade).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn failing_first_kind_cannot_steal_handoff_and_absent_kind_expires() {
        let gate = Arc::new(Gate::new());
        let failing_backup = gate.try_start(true, Kind::Backup).unwrap();
        assert!(gate.try_start(true, Kind::Upgrade).is_none());
        assert!(gate.try_start(true, Kind::Schema).is_none());
        drop(failing_backup);
        assert!(gate.try_start(true, Kind::Backup).is_none());
        let upgrade = gate.try_start(true, Kind::Upgrade).unwrap();
        drop(upgrade);
        assert!(gate.try_start(true, Kind::Backup).is_none());
        // Schema no longer has a pending job; its reserved turn is bounded.
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(gate.try_start(true, Kind::Backup).is_some());
    }

    #[tokio::test]
    async fn actual_postgres_failing_oldest_job_does_not_starve_later_pages() {
        let Ok(dsn) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: maintenance queue fairness requires PostgreSQL");
            return;
        };
        let db = crate::db::Db::new(dsn).unwrap();
        let c = db.connect().await.unwrap();
        c.batch_execute("CREATE TEMP TABLE storage_recovery_jobs(operation_id uuid, deployment_id uuid, state text, updated_at timestamptz); CREATE TEMP TABLE deployment_storage_holds(operation_id uuid)").await.unwrap();
        for number in 1..=7u128 {
            c.execute(
                "INSERT INTO storage_recovery_jobs VALUES($1,$1,'pending',now())",
                &[&Uuid::from_u128(number)],
            )
            .await
            .unwrap();
        }
        let blocked = Uuid::from_u128(1);
        c.query_one(
            "SELECT pg_advisory_lock(hashtextextended($1,0))",
            &[&format!("xelab/deployment-runtime/{blocked}")],
        )
        .await
        .unwrap();
        let gate = Arc::new(Gate::new());
        let mut queue = Queue::default();
        let mut attempted = Vec::new();
        let mut runnable = Vec::new();
        for _ in 0..8 {
            let rows = queue.candidates(&c, Kind::Backup, true).await.unwrap();
            assert!(rows.len() <= 4);
            let operation: Uuid = rows[0].get(0);
            let permit = gate.try_start(true, Kind::Backup).unwrap();
            queue.attempted(operation);
            attempted.push(operation.as_u128());
            let fence = crate::db::deployment_fence(&db, operation).await.unwrap();
            if operation == blocked {
                assert!(
                    fence.is_none(),
                    "oldest job repeatedly fails its real PostgreSQL fence"
                );
            } else {
                assert!(fence.is_some());
                runnable.push(operation.as_u128());
            }
            // Leave all jobs pending with their original updated_at, including
            // the fenced oldest operation; the next page must still progress.
            drop(fence);
            drop(permit);
        }
        assert_eq!(attempted, vec![1, 2, 3, 4, 5, 6, 7, 1]);
        assert_eq!(runnable, vec![2, 3, 4, 5, 6, 7]);
    }
}
