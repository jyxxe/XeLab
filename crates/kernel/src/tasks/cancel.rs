//! Persist cancellation intent without inferring an external stop.
use super::{authorize_task_action, retry};
use crate::{
    api::etag,
    auth::{Context, Resource},
    db,
    error::{Error, Result},
    plugin_catalog::read_task,
};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub(super) async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    task: Uuid,
) -> Result<()> {
    let owner = c
        .query_opt("SELECT owner_id FROM tasks WHERE id=$1", &[&task])
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    authorize_task_action(c, ctx, "task.cancel", task, owner).await
}

pub(super) async fn was_requested(c: &(impl GenericClient + Sync), task: Uuid) -> Result<bool> {
    // Older successful cancellations audited the raw URL parameter. Retain
    // their intent when an accepted UUID spelling was not canonical.
    Ok(c.query_one(
        "SELECT EXISTS(SELECT FROM audit_events WHERE action='cancelTask'
               AND outcome='succeeded' AND reason_code='COMMITTED'
               AND (resource=$1 OR (
                   resource=jsonb_build_object('kind','task','match','ids','ids',jsonb_build_array(resource#>>'{ids,0}'))
                   AND CASE WHEN pg_input_is_valid(resource#>>'{ids,0}', 'uuid')
                            THEN (resource#>>'{ids,0}')::uuid END=$2)))",
        &[&Resource::new("task", task, None).json(), &task],
    ).await?.try_get(0)?)
}

pub(super) async fn request(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    task: Uuid,
    headers: &HeaderMap,
) -> Result<Value> {
    // Claim, lease reconciliation and result commit all lock the parent.
    let row = c
        .query_opt(
            "SELECT owner_id,state,revision FROM tasks WHERE id=$1 FOR NO KEY UPDATE",
            &[&task],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let owner = row.try_get("owner_id")?;
    authorize_task_action(c, ctx, "task.cancel", task, owner).await?;
    etag(headers, row.try_get("revision")?)?;
    let state: &str = row.try_get("state")?;
    if !matches!(state, "succeeded" | "failed" | "canceled") {
        let stopped = c
            .execute(
                "UPDATE task_steps s SET state='canceled',revision=revision+1,updated_at=clock_timestamp()
                 WHERE s.task_id=$1 AND s.state IN ('pending','ready') AND s.attempt_count=0
                   AND NOT EXISTS (SELECT FROM attempts a WHERE a.step_id=s.id)",
                &[&task],
            )
            .await?;
        let stopped = stopped + retry::cancel_queued(c, task).await?;
        let next_state = if state == "unknown" {
            state
        } else if settled(c, task).await? {
            "canceled"
        } else {
            "cancel_requested"
        };
        if next_state != state || stopped > 0 {
            update_state(c, task, next_state).await?;
        }
    }
    let task_record = read_task(c, task, None)
        .await?
        .ok_or_else(Error::not_found)?;
    // Timed rules may expire while waiting on step writes or the outbox.
    authorize_task_action(c, ctx, "task.cancel", task, owner).await?;
    Ok(task_record)
}

/// Call while holding the task row lock. Expiry or uncertain termination alone
/// cannot establish that the external work has stopped.
pub(super) async fn settled(c: &(impl GenericClient + Sync), task: Uuid) -> Result<bool> {
    Ok(c.query_one(
        "SELECT NOT EXISTS (
             SELECT FROM task_steps WHERE task_id=$1 AND state IN ('pending','ready','running','unknown')
         ) AND NOT EXISTS (
             SELECT FROM attempts a JOIN task_steps s ON s.id=a.step_id
              WHERE s.task_id=$1 AND (a.state IN ('leased','running')
                 OR (a.state IN ('expired','unknown') AND NOT EXISTS (
                     SELECT FROM task_step_resolutions r WHERE r.attempt_id=a.id
                 )))
         )",
        &[&task],
    ).await?.try_get(0)?)
}

pub(super) async fn update_state(
    c: &(impl GenericClient + Sync),
    task: Uuid,
    state: &str,
) -> Result<()> {
    c.execute(
        "UPDATE tasks SET state=$2,revision=revision+1,updated_at=clock_timestamp(),
            error=CASE WHEN $2='canceled' AND error->>'code'='OUTCOME_UNKNOWN' THEN NULL ELSE error END
         WHERE id=$1",
        &[&task, &state],
    )
    .await?;
    db::outbox(
        c,
        if state == "canceled" {
            "task.canceled"
        } else {
            "task.cancel.requested"
        },
        task,
        &json!({"task_id":task,"state":state}),
    )
    .await
}
