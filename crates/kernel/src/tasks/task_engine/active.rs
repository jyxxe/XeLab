//! Shared identity, locking and frozen authorization checks for a claimed attempt.
use super::{claim_authority, AttemptLease};
use crate::{
    auth::Auth,
    db::auth_lock,
    error::{Error, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

// The parent task is locked before any of its steps. Sibling commits must not
// each hold a different child lock while trying to aggregate the same task.
const LOCK_TASK: &str = "
    SELECT t.id FROM tasks t JOIN task_steps s ON s.task_id=t.id
    JOIN attempts a ON a.step_id=s.id WHERE a.id=$1 FOR NO KEY UPDATE OF t";

const LOCK_ATTEMPT: &str = "
    SELECT a.step_id,s.task_id,s.provider_deployment_id,t.owner_id,t.workflow_id,
           t.admission,t.admission IS NOT NULL AS admitted,t.deadline_at,t.state AS task_state,w.owner_id AS workflow_owner_id,
           e.subject_id,e.issuer_credential_id,e.maximum_rules,e.expires_at,
           e.status='active' AND e.run_as='user' AND e.service_account_id IS NULL
             AND e.issuer_credential_id IS NOT NULL AND e.subject_id=t.owner_id
             AND e.expires_at>clock_timestamp() AS delegation_live,
           coalesce((SELECT node FROM jsonb_array_elements(w.spec->'steps') node
             WHERE node->>'id'=s.step_key),'null'::jsonb) AS step,
           a.execution_id=$2 AND a.worker_id IS NOT DISTINCT FROM $3 AND a.fence=$4
             AND a.fence BETWEEN 1 AND 9007199254740991
             AND s.fence=a.fence AND s.attempt_count=a.number AS current_identity,
           a.created_at,a.lease_expires_at,a.state AS attempt_state,s.state AS step_state
      FROM attempts a JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
      JOIN workflows w ON w.id=t.workflow_id JOIN execution_delegations e ON e.id=t.delegation_id
     WHERE a.id=$1 AND t.id=$5 FOR UPDATE OF a,s FOR SHARE OF e,w";

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Commit,
    Renew,
    PrepareDispatch,
}

impl Operation {
    fn audit_codes(self) -> (&'static str, &'static str) {
        match self {
            Self::Commit => ("task.attempt.commit", "STALE_ATTEMPT_COMMIT"),
            Self::Renew => ("task.attempt.renew", "STALE_ATTEMPT_RENEW"),
            Self::PrepareDispatch => ("task.attempt.prepare_dispatch", "STALE_ATTEMPT_DISPATCH"),
        }
    }

    pub(super) async fn reject<T>(
        self,
        tx: Transaction<'_>,
        lease: &AttemptLease,
        row: Option<&Row>,
    ) -> Result<T> {
        let owner: Option<Uuid> = row.map(|r| r.get("owner_id"));
        let resource = match row {
            Some(row) => json!({"kind":"task","match":"ids","ids":[row.get::<_, Uuid>("task_id")]}),
            None => json!({"kind":"platform","match":"all"}),
        };
        let (action, reason) = self.audit_codes();
        tx.execute(
            "INSERT INTO audit_events(id,subject_id,request_id,action,resource,outcome,policy_revision,
                                      reason_code,created_at,updated_at)
             VALUES($1,$2,$3,$4,$5,'denied',
                    coalesce((SELECT authz_revision FROM accounts WHERE id=$2),0),
                    $6,clock_timestamp(),clock_timestamp())",
            &[&Uuid::new_v4(), &owner, &lease.execution_id, &action, &resource, &reason],
        ).await?;
        tx.commit().await?;
        Err(Error::conflict(match self {
            Self::Commit => "Attempt result does not match a current live claim",
            _ => "Worker lease does not match a current live claim",
        }))
    }
}

pub(super) struct LockedAttempt<'a> {
    pub tx: Transaction<'a>,
    pub row: Row,
    pub step: Value,
    pub lease_end: DateTime<Utc>,
    pub lease: AttemptLease,
    operation: Operation,
}

impl<'a> LockedAttempt<'a> {
    pub async fn lock(
        tx: Transaction<'a>,
        lease: &AttemptLease,
        operation: Operation,
    ) -> Result<Self> {
        auth_lock(&tx, false).await?;
        let Some(task) = tx.query_opt(LOCK_TASK, &[&lease.attempt_id]).await? else {
            return operation.reject(tx, lease, None).await;
        };
        let row = tx
            .query_opt(
                LOCK_ATTEMPT,
                &[
                    &lease.attempt_id,
                    &lease.execution_id,
                    &lease.worker_id,
                    &lease.fence,
                    &task.get::<_, Uuid>(0),
                ],
            )
            .await?;
        let Some(row) = row else {
            return operation.reject(tx, lease, None).await;
        };
        // A locking SELECT can wait after evaluating its projection.
        let lease_end = row
            .get::<_, DateTime<Utc>>("lease_expires_at")
            .min(row.get("deadline_at"));
        if !row.get::<_, bool>("current_identity") || !live(&tx, lease_end).await? {
            return operation.reject(tx, lease, Some(&row)).await;
        }
        Ok(Self {
            step: row.try_get("step")?,
            tx,
            row,
            lease_end,
            lease: *lease,
            operation,
        })
    }

    pub fn active(&self) -> bool {
        matches!(
            self.row.get::<_, &str>("attempt_state"),
            "leased" | "running"
        ) && self.row.get::<_, &str>("step_state") == "running"
            && self.row.get::<_, &str>("task_state") == "running"
    }

    pub async fn finish(self) -> Result<Row> {
        self.tx.commit().await?;
        Ok(self.row)
    }

    pub async fn reject<T>(self) -> Result<T> {
        self.operation
            .reject(self.tx, &self.lease, Some(&self.row))
            .await
    }

    /// Commit replays validate the same frozen proof but never suspend a task.
    pub async fn authorize(self, auth: &Auth, replay: bool) -> Result<(Self, DateTime<Utc>)> {
        let authority = if self.step.is_object()
            && self.row.get("delegation_live")
            && self.row.get("admitted")
        {
            claim_authority::revalidate(&self.tx, auth, &self.row, &self.step, self.lease.worker_id)
                .await
        } else {
            Err(Error::denied())
        };
        let error = match authority {
            Ok(until) => {
                let until = until
                    .min(self.row.get("expires_at"))
                    .min(self.row.get("deadline_at"));
                return Ok((self, until));
            }
            Err(error) => error,
        };
        let (state, error) = match error.code {
            "INVALID_CREDENTIAL" | "PERMISSION_DENIED" => ("paused_authorization", Error::denied()),
            "TASK_PROVIDER_UNAVAILABLE" => (
                "waiting_dependency",
                Error::conflict("Admitted task provider is unavailable"),
            ),
            _ => return Err(error),
        };
        if !live(&self.tx, self.lease_end).await? {
            return self.reject().await;
        }
        if !replay {
            self.tx
                .batch_execute("SAVEPOINT before_suspend; SET CONSTRAINTS ALL IMMEDIATE")
                .await?;
            self.tx
                .execute(
                    "UPDATE tasks SET state=$2,revision=revision+1,updated_at=clock_timestamp()
                 WHERE id=$1 AND state='running' AND clock_timestamp()<$3",
                    &[&self.row.get::<_, Uuid>("task_id"), &state, &self.lease_end],
                )
                .await?;
            if !live(&self.tx, self.lease_end).await? {
                self.tx
                    .batch_execute("ROLLBACK TO SAVEPOINT before_suspend")
                    .await?;
                return self.reject().await;
            }
        }
        self.tx.commit().await?;
        Err(error)
    }
}

pub(super) async fn live(tx: &Transaction<'_>, until: DateTime<Utc>) -> Result<bool> {
    Ok(tx
        .query_one(
            "SELECT clock_timestamp()<$1 AND NOT EXISTS (
                        SELECT FROM platform_recovery_state WHERE phase<>'ready')",
            &[&until],
        )
        .await?
        .get(0))
}
