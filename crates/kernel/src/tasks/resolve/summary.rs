//! Refresh the parent after resolution without rearming a stopped workflow.
use super::{cancel, commit_effects};
use crate::error::{Error, Result};
use axum::http::StatusCode;
use serde_json::Value;
use tokio_postgres::Transaction;
use uuid::Uuid;

pub(super) async fn update(tx: &Transaction<'_>, task: Uuid, previous: &str) -> Result<()> {
    // An operator can resolve an old step without reviving paused, failed
    // or canceled work. Cancellation settles only with proof for each attempt.
    if !matches!(previous, "running" | "unknown" | "cancel_requested") {
        return Ok(());
    }
    let steps = tx
        .query_one(
            "SELECT bool_or(s.state='unknown') AS unknown, bool_or(s.state='failed') AS failed,
                bool_or(s.state='canceled') AS canceled, bool_and(s.state='succeeded') AS succeeded,
                EXISTS (SELECT FROM attempts a JOIN task_steps other ON other.id=a.step_id
                         WHERE other.task_id=$1 AND a.state IN ('leased','running')) AS live
         FROM task_steps s WHERE s.task_id=$1",
            &[&task],
        )
        .await?;
    // cancelTask deliberately retains the unknown state. Its success
    // audit also records a cancellation when there were no pending steps.
    let canceled_unknown = previous == "unknown" && cancel::was_requested(tx, task).await?;
    if previous == "cancel_requested" || canceled_unknown || steps.try_get::<_, bool>("canceled")? {
        if cancel::settled(tx, task).await? {
            cancel::update_state(tx, task, "canceled").await?;
        }
        return Ok(());
    }
    let live: bool = steps.try_get("live")?;
    if !live && steps.try_get::<_, bool>("succeeded")? {
        return commit_effects::complete_task(tx, task).await;
    }

    let unknown: bool = steps.try_get("unknown")?;
    let failed: bool = steps.try_get("failed")?;
    let viability = tx
        .query_one(
            "SELECT t.deadline_at>clock_timestamp() AS in_time,
                t.admission IS NOT NULL AND e.status='active' AND e.run_as='user'
                 AND e.subject_id=t.owner_id AND e.issuer_credential_id IS NOT NULL
                 AND e.expires_at>clock_timestamp() AS delegated
         FROM tasks t JOIN execution_delegations e ON e.id=t.delegation_id
         WHERE t.id=$1",
            &[&task],
        )
        .await?;
    let in_time: bool = viability.try_get("in_time")?;
    let delegated: bool = viability.try_get("delegated")?;
    let (state, error) = if unknown {
        let detail = Error::new(
            "OUTCOME_UNKNOWN",
            "A task step outcome remains unknown",
            StatusCode::CONFLICT,
        )
        .detail();
        (if live { "running" } else { "unknown" }, Some(detail))
    } else if failed {
        (
            if live { "running" } else { "failed" },
            step_failure(tx, task).await?,
        )
    } else if live {
        ("running", None)
    } else if !in_time {
        let detail = Error::new(
            "DEADLINE_EXCEEDED",
            "The original task deadline has passed",
            StatusCode::GATEWAY_TIMEOUT,
        )
        .detail();
        ("failed", Some(detail))
    } else if !delegated {
        ("paused_authorization", None)
    } else {
        ("running", None)
    };
    tx.execute(
        "UPDATE tasks SET state=$2,error=$3,output=NULL,revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1",
        &[&task, &state, &error],
    )
    .await?;
    if state == "running" && !unknown && !failed && in_time && delegated {
        // This enables first attempts only. Each still independently validates
        // the original admission, not the operator's resolution authority.
        commit_effects::wake_dependents(tx, task, None).await?;
    }
    Ok(())
}

async fn step_failure(tx: &Transaction<'_>, task: Uuid) -> Result<Option<Value>> {
    let row = tx.query_opt(
        "SELECT r.resolution, NULLIF(c.error,'null'::jsonb) AS error
         FROM task_steps s JOIN attempts a ON a.step_id=s.id
           AND a.number=s.attempt_count AND a.fence=s.fence
         LEFT JOIN task_step_resolutions r ON r.attempt_id=a.id
         LEFT JOIN attempt_commit_receipts c ON c.attempt_id=a.id
         WHERE s.task_id=$1 AND s.state='failed'
           AND (r.resolution='confirmed_not_executed' OR (c.state='failed' AND c.error<>'null'::jsonb))
         ORDER BY s.updated_at,s.id LIMIT 1",
        &[&task],
    ).await?;
    match row {
        Some(row)
            if row.try_get::<_, Option<&str>>("resolution")? == Some("confirmed_not_executed") =>
        {
            Ok(Some(
                Error::new(
                    "CONFIRMED_NOT_EXECUTED",
                    "An operator confirmed that the step did not execute",
                    StatusCode::CONFLICT,
                )
                .detail(),
            ))
        }
        Some(row) => Ok(row.try_get("error")?),
        None => Ok(None),
    }
}
