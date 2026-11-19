//! Accept a result only while the original claim and its authority are live.
use super::{
    active::{live, LockedAttempt, Operation},
    claim_authority, commit_effects, AttemptLease, ENGINE_TIMEOUT,
};
use crate::{
    auth::Auth,
    contracts::{reject_jsonb_nul, string, unsigned, validate},
    db::Db,
    error::{Error, Result},
};
use chrono::{DateTime, Utc};
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
    let mut cutoff = attempt.lease_end.min(authorized_until);
    if let Some(call_cutoff) = linked_result_cutoff(&attempt, auth, &input).await? {
        cutoff = cutoff.min(call_cutoff);
    }
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

// Caller holds the authorization, parent task and exact worker/attempt locks.
// The Call is already settled: its short-lived physical RPC token is revoked.
async fn linked_result_cutoff(
    attempt: &LockedAttempt<'_>,
    auth: &Auth,
    input: &Input<'_>,
) -> Result<Option<DateTime<Utc>>> {
    let row = &attempt.row;
    let owner: Uuid = row.try_get("owner_id")?;
    let issuer: Option<Uuid> = row.try_get("issuer_credential_id")?;
    let key: &str = row.try_get("idempotency_key")?;
    let tx = &attempt.tx;
    let result = tx
        .query_opt(
            "SELECT c.id,c.owner_id=$5 AND c.credential_id=$6 AND c.idempotency_key=$7
            AND c.state=$2 AND CASE
                WHEN $2='succeeded' THEN c.result->'output'=$3 AND c.error IS NULL
                     AND c.completed_at<=c.deadline_at
                WHEN $2='failed' THEN c.result IS NULL AND coalesce(c.error,'null'::jsonb)=$4
                     AND c.completed_at<=c.deadline_at
                WHEN $2='unknown' THEN c.result IS NULL AND coalesce(c.error,'null'::jsonb)=$4
                ELSE false END AS matches
           FROM task_attempt_calls b JOIN calls c ON c.id=b.call_id
          WHERE b.attempt_id=$1 FOR SHARE OF c",
            &[
                &input.attempt,
                &input.state,
                input.output,
                input.error,
                &owner,
                &issuer,
                &key,
            ],
        )
        .await?;
    let Some(result) = result else {
        return Ok(None);
    };
    if result.try_get::<_, Option<bool>>("matches")? != Some(true) {
        return Err(Error::conflict(
            "Attempt result does not match its settled Call",
        ));
    }
    if input.state == "unknown" {
        return Ok(None);
    }
    let admission: Value = row.try_get("admission")?;
    let ctx =
        claim_authority::resume_original(tx, auth, row, attempt.lease.execution_id, &admission)
            .await?;
    let call = result.try_get("id")?;
    let paths = crate::host_blobs::checked_call_sources(tx, &ctx, call).await?;
    Ok(Some(
        claim_authority::dispatch_paths_until(tx, &ctx, &paths).await?,
    ))
}
