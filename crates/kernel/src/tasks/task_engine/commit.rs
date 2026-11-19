//! Accept a result only while the original claim and its authority are live.
use super::{claim_authority, commit_effects};
use crate::{
    auth::Auth,
    contracts::{reject_jsonb_nul, string, unsigned, validate},
    db::{auth_lock, Db},
    error::{Error, Result},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Input<'a> {
    attempt: Uuid,
    execution: Uuid,
    fence: i64,
    state: &'a str,
    output: &'a Value,
    error: &'a Value,
}

impl<'a> Input<'a> {
    fn parse(value: &'a Value) -> Result<Self> {
        let fields = value
            .as_object()
            .filter(|o| o.len() == 6)
            .ok_or_else(|| Error::invalid("Expected the six AttemptCommitInput fields"))?;
        let uuid = |key| {
            let text = string(value, key)?;
            if text.len() != 36 {
                return Err(Error::invalid("Expected a hyphenated UUID"));
            }
            Uuid::try_parse(text).map_err(|_| Error::invalid("Invalid commit UUID"))
        };
        let attempt = uuid("attempt_id")?;
        let execution = uuid("execution_id")?;
        let fence = unsigned(&value["fence"])?;
        if !(1..=9_007_199_254_740_991).contains(&fence) {
            return Err(Error::invalid("Commit fence is out of range"));
        }
        let state = string(value, "state")?;
        let output = fields
            .get("output")
            .ok_or_else(|| Error::invalid("Missing commit output"))?;
        let error = fields
            .get("error")
            .ok_or_else(|| Error::invalid("Missing commit error"))?;
        match state {
            "succeeded" if error.is_null() => (),
            "failed" | "unknown" if output.is_null() => {
                if !error.is_null() {
                    validate("ErrorDetail", error)?;
                }
            }
            _ => return Err(Error::invalid("Invalid terminal state, output or error")),
        }
        reject_jsonb_nul(output)?;
        reject_jsonb_nul(error)?;
        Ok(Self {
            attempt,
            execution,
            fence: fence as i64,
            state,
            output,
            error,
        })
    }
}

/// Commit a trusted worker result or acknowledge its identical, still-valid receipt.
pub async fn commit(db: &Db, auth: &Auth, worker_id: Uuid, input: &Value) -> Result<Value> {
    tokio::time::timeout(TIMEOUT, commit_result(db, auth, worker_id, input))
        .await
        .map_err(|_| Error::unavailable("Task commit timed out"))?
}

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
             AND s.fence=a.fence AND s.attempt_count=a.number AS current_identity,
           a.lease_expires_at,a.state AS attempt_state,s.state AS step_state,
           NOT EXISTS(SELECT FROM platform_recovery_state WHERE phase<>'ready') AS ready
      FROM attempts a JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
      JOIN workflows w ON w.id=t.workflow_id JOIN execution_delegations e ON e.id=t.delegation_id
     WHERE a.id=$1 AND t.id=$5 FOR UPDATE OF a,s FOR SHARE OF e,w";

async fn commit_result(db: &Db, auth: &Auth, worker: Uuid, value: &Value) -> Result<Value> {
    let input = Input::parse(value)?;
    let mut client = db.connect_scoped(TIMEOUT).await?;
    let tx = client.transaction().await?;
    auth_lock(&tx, false).await?;
    let task: Option<Uuid> = tx
        .query_opt(LOCK_TASK, &[&input.attempt])
        .await?
        .map(|r| r.get(0));
    let Some(task) = task else {
        return stale(tx, &input, None).await;
    };
    let row = tx
        .query_opt(
            LOCK_ATTEMPT,
            &[
                &input.attempt,
                &input.execution,
                &worker,
                &input.fence,
                &task,
            ],
        )
        .await?;
    let Some(row) = row else {
        return stale(tx, &input, None).await;
    };
    // FOR UPDATE can itself wait. Sample time in a new statement after the
    // locks are held, including for a read-only acknowledgement.
    let lease_end = row
        .get::<_, DateTime<Utc>>("lease_expires_at")
        .min(row.get("deadline_at"));
    if !row.get::<_, bool>("current_identity")
        || !row.get::<_, bool>("ready")
        || !live(&tx, lease_end).await?
    {
        return stale(tx, &input, Some(&row)).await;
    }
    let attempt_state: &str = row.get("attempt_state");
    let replay = matches!(attempt_state, "succeeded" | "failed" | "unknown");
    if replay {
        // Compare the full accepted result with JSONB equality. No public
        // attempt/step projection needs to read that potentially large payload.
        let identical: bool = tx
            .query_one(
                "SELECT EXISTS(SELECT FROM attempt_commit_receipts
              WHERE attempt_id=$1 AND state=$2 AND output=$3 AND error=$4)",
                &[&input.attempt, &input.state, input.output, input.error],
            )
            .await?
            .get(0);
        if attempt_state != input.state
            || row.get::<_, &str>("step_state") != input.state
            || !identical
        {
            return stale(tx, &input, Some(&row)).await;
        }
    } else if !matches!(attempt_state, "leased" | "running")
        || row.get::<_, &str>("step_state") != "running"
        || row.get::<_, &str>("task_state") != "running"
    {
        return stale(tx, &input, Some(&row)).await;
    }

    let workflow_step: Value = row.try_get("step")?;
    let authority =
        if workflow_step.is_object() && row.get("delegation_live") && row.get("admitted") {
            claim_authority::revalidate(&tx, auth, &row, &workflow_step, worker).await
        } else {
            Err(Error::denied())
        };
    let authorized_until = match authority {
        Ok(until) => until.min(row.get("expires_at")),
        Err(error) => {
            let (state, error) = match error.code {
                "INVALID_CREDENTIAL" | "PERMISSION_DENIED" => {
                    ("paused_authorization", Error::denied())
                }
                "TASK_PROVIDER_UNAVAILABLE" => (
                    "waiting_dependency",
                    Error::conflict("Admitted task provider is unavailable"),
                ),
                _ => return Err(error),
            };
            if !live(&tx, lease_end).await? {
                return stale(tx, &input, Some(&row)).await;
            }
            if !replay {
                tx.execute(
                    "UPDATE tasks SET state=$2,revision=revision+1,updated_at=clock_timestamp()
                            WHERE id=$1 AND state='running' AND clock_timestamp()<$3",
                    &[&task, &state, &lease_end],
                )
                .await?;
            }
            tx.commit().await?;
            return Err(error);
        }
    };
    let cutoff = lease_end.min(authorized_until);
    let step = row.get("step_id");
    let result = if replay {
        commit_effects::read_step(&tx, step).await?
    } else {
        // Keep the task/identity/authorization locks in the outer transaction
        // so an expired write can roll back its outbox as well as its result,
        // then persist the rejection audit on the same locked history.
        tx.batch_execute("SAVEPOINT before_result").await?;
        let result = commit_effects::apply(
            &tx,
            input.attempt,
            step,
            task,
            &workflow_step,
            input.state,
            input.output,
            input.error,
        )
        .await?;
        tx.execute(
            "INSERT INTO attempt_commit_receipts(attempt_id,state,output,error)
                    VALUES($1,$2,$3,$4)",
            &[&input.attempt, &input.state, input.output, input.error],
        )
        .await?;
        result
    };
    // Last operation before COMMIT. All identity rows remain locked and this
    // fresh clock sample includes waits on result, dependency and outbox rows.
    if !live(&tx, cutoff).await? {
        if !replay {
            tx.batch_execute("ROLLBACK TO SAVEPOINT before_result")
                .await?;
        }
        return stale(tx, &input, Some(&row)).await;
    }
    tx.commit().await?;
    Ok(result)
}

async fn live(tx: &Transaction<'_>, until: DateTime<Utc>) -> Result<bool> {
    Ok(tx
        .query_one("SELECT clock_timestamp()<$1", &[&until])
        .await?
        .get(0))
}

async fn stale(tx: Transaction<'_>, input: &Input<'_>, row: Option<&Row>) -> Result<Value> {
    let owner: Option<Uuid> = row.map(|r| r.get("owner_id"));
    let resource = match row {
        Some(row) => json!({"kind":"task","match":"ids","ids":[row.get::<_, Uuid>("task_id")]}),
        None => json!({"kind":"platform","match":"all"}),
    };
    tx.execute(
        "INSERT INTO audit_events(id,subject_id,request_id,action,resource,outcome,policy_revision,
                                  reason_code,created_at,updated_at)
         VALUES($1,$2,$3,'task.attempt.commit',$4,'denied',
                coalesce((SELECT authz_revision FROM accounts WHERE id=$2),0),
                'STALE_ATTEMPT_COMMIT',clock_timestamp(),clock_timestamp())",
        &[&Uuid::new_v4(), &owner, &input.execution, &resource],
    )
    .await?;
    tx.commit().await?;
    Err(Error::conflict(
        "Attempt result does not match a current live claim",
    ))
}
