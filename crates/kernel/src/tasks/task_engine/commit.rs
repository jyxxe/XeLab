//! Accept a result only while the original claim and its authority are live.
use super::{
    active::{live, LockedAttempt, Operation},
    commit_effects, AttemptLease, ENGINE_TIMEOUT,
};
use crate::{
    auth::Auth,
    contracts::{reject_jsonb_nul, string, unsigned, validate},
    db::Db,
    error::{Error, Result},
};
use serde_json::Value;
use uuid::Uuid;

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
    tokio::time::timeout(ENGINE_TIMEOUT, commit_result(db, auth, worker_id, input))
        .await
        .map_err(|_| Error::unavailable("Task commit timed out"))?
}

async fn commit_result(db: &Db, auth: &Auth, worker: Uuid, value: &Value) -> Result<Value> {
    let input = Input::parse(value)?;
    let lease = AttemptLease {
        attempt_id: input.attempt,
        execution_id: input.execution,
        fence: input.fence,
        worker_id: worker,
    };
    let mut client = db.connect_scoped(ENGINE_TIMEOUT).await?;
    let attempt =
        LockedAttempt::lock(client.transaction().await?, &lease, Operation::Commit).await?;
    let attempt_state: &str = attempt.row.get("attempt_state");
    let replay = matches!(attempt_state, "succeeded" | "failed" | "unknown");
    if replay {
        // Compare the full accepted result with JSONB equality. No public
        // attempt/step projection needs to read that potentially large payload.
        let identical: bool = attempt
            .tx
            .query_one(
                "SELECT EXISTS(SELECT FROM attempt_commit_receipts
              WHERE attempt_id=$1 AND state=$2 AND output=$3 AND error=$4)",
                &[&input.attempt, &input.state, input.output, input.error],
            )
            .await?
            .get(0);
        if attempt_state != input.state
            || attempt.row.get::<_, &str>("step_state") != input.state
            || !identical
        {
            return attempt.reject().await;
        }
    } else if !attempt.active() {
        return attempt.reject().await;
    }

    let (attempt, authorized_until) = attempt.authorize(auth, replay).await?;
    let cutoff = attempt.lease_end.min(authorized_until);
    let tx = &attempt.tx;
    let step = attempt.row.get("step_id");
    let task = attempt.row.get("task_id");
    let result = if replay {
        commit_effects::read_step(tx, step).await?
    } else {
        // Keep the task/identity/authorization locks in the outer transaction
        // so an expired write can roll back its outbox as well as its result,
        // then persist the rejection audit on the same locked history.
        tx.batch_execute("SAVEPOINT before_result").await?;
        let result = commit_effects::apply(
            tx,
            input.attempt,
            step,
            task,
            &attempt.step,
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
    if !live(tx, cutoff).await? {
        if !replay {
            tx.batch_execute("ROLLBACK TO SAVEPOINT before_result")
                .await?;
        }
        return attempt.reject().await;
    }
    attempt.tx.commit().await?;
    Ok(result)
}
