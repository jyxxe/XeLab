//! Persist cancellation intent without inferring an external stop.
use crate::{
    api::etag,
    auth::{self, Context, Resource},
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
    authorize_resource(c, ctx, task, owner).await
}

async fn authorize_resource(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    task: Uuid,
    owner: Uuid,
) -> Result<()> {
    let resource = Resource::new("task", task, Some(owner));
    let result = auth::authorize(c, ctx, "task.cancel", &resource).await;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "PERMISSION_DENIED")
        && !crate::policy::allows(c, ctx, "task.read", &resource).await?
    {
        return Err(Error::not_found());
    }
    result
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
    authorize_resource(c, ctx, task, owner).await?;
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
    authorize_resource(c, ctx, task, owner).await?;
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
              WHERE s.task_id=$1 AND a.state IN ('leased','running','expired','unknown')
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
        "UPDATE tasks SET state=$2,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
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
