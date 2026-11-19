//! Record an operator's adjudication of an unknown, fenced attempt.
mod binding;
mod summary;

use super::{authorize_task_action, cancel, commit_effects};
use crate::{
    api::etag,
    auth::Context,
    contracts::{reject_jsonb_nul, string},
    db::outbox,
    error::{Error, Result},
};
use axum::http::HeaderMap;
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub(super) async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    step: Uuid,
) -> Result<()> {
    let row = c
        .query_opt(
            "SELECT t.id,t.owner_id FROM task_steps s JOIN tasks t ON t.id=s.task_id WHERE s.id=$1",
            &[&step],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    authorize_task_action(
        c,
        ctx,
        "task.resolve",
        row.try_get("id")?,
        row.try_get("owner_id")?,
    )
    .await
}

pub(super) async fn apply(
    tx: &Transaction<'_>,
    ctx: &Context,
    step: Uuid,
    headers: &HeaderMap,
    body: &Value,
) -> Result<Value> {
    let evidence = string(body, "evidence")?;
    if evidence.trim().is_empty() {
        return Err(Error::invalid("Resolution requires nonblank evidence"));
    }
    reject_jsonb_nul(body)?;
    let resolution = string(body, "resolution")?;
    let (state, output) = match (resolution, body.get("output")) {
        ("confirmed_succeeded", Some(output)) => ("succeeded", Some(output)),
        ("confirmed_succeeded", None) => {
            return Err(Error::invalid("Successful resolution requires output"))
        }
        ("confirmed_not_executed", None) => ("failed", None),
        ("confirmed_not_executed", Some(_)) => {
            return Err(Error::invalid("A step that did not execute has no output"))
        }
        _ => return Err(Error::invalid("Unsupported step resolution")),
    };

    // Match claim, worker settlement and cancellation: parent, step, attempt.
    let parent = tx
        .query_opt(
            "SELECT t.id,t.owner_id,t.state,t.workflow_id,t.admission
             FROM task_steps s JOIN tasks t ON t.id=s.task_id WHERE s.id=$1
             FOR NO KEY UPDATE OF t",
            &[&step],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let task: Uuid = parent.try_get("id")?;
    let owner: Uuid = parent.try_get("owner_id")?;
    authorize_task_action(tx, ctx, "task.resolve", task, owner).await?;
    let selected = tx
        .query_opt(
            "SELECT revision,state,step_key,provider_deployment_id,attempt_count,fence
             FROM task_steps WHERE id=$1 AND task_id=$2 FOR UPDATE",
            &[&step, &task],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    etag(headers, selected.try_get("revision")?)?;
    if selected.try_get::<_, &str>("state")? != "unknown" {
        return Err(Error::conflict(
            "Only a step with an unknown result may be resolved",
        ));
    }

    let attempt: Uuid = tx
        .query_opt(
            "SELECT a.id FROM attempts a WHERE a.step_id=$1 AND a.number=$2 AND a.fence=$3
              AND a.number>0 AND a.fence BETWEEN 1 AND 9007199254740991 AND a.state IN ('expired','unknown')
              AND NOT EXISTS (SELECT FROM attempts other WHERE other.step_id=$1
                              AND other.state IN ('leased','running'))
              AND NOT EXISTS (SELECT FROM task_step_resolutions r WHERE r.attempt_id=a.id)
              FOR UPDATE OF a",
            &[
                &step,
                &selected.try_get::<_, i32>("attempt_count")?,
                &selected.try_get::<_, i64>("fence")?,
            ],
        )
        .await?
        .ok_or_else(|| Error::conflict("No unresolved fenced attempt matches the unknown step"))?
        .try_get(0)?;

    let admission: Option<Value> = parent.try_get("admission")?;
    let pinned = binding::historical(
        tx,
        parent.try_get("workflow_id")?,
        admission.as_ref(),
        selected.try_get("step_key")?,
        selected.try_get("provider_deployment_id")?,
    )
    .await?;
    if let Some(output) = output {
        pinned.validate_output(tx, output).await?;
    }

    tx.execute(
        "INSERT INTO task_step_resolutions(attempt_id,resolution,evidence,subject_id,
                credential_id,credential_chain,request_id,created_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,clock_timestamp())",
        &[
            &attempt,
            &resolution,
            &evidence,
            &ctx.subject,
            &ctx.credential,
            &ctx.chain,
            &ctx.request,
        ],
    )
    .await?;
    tx.execute(
        "UPDATE task_steps SET state=$2,output=$3,revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1",
        &[&step, &state, &output],
    )
    .await?;
    outbox(
        tx,
        "task.step.resolved",
        step,
        &json!({"task_id":task,"step_id":step,"attempt_id":attempt,"resolution":resolution}),
    )
    .await?;
    summary::update(tx, task, parent.try_get("state")?).await?;
    let result = commit_effects::read_step(tx, step).await?;
    // Timed task.resolve rights may expire while waiting on row locks or outbox.
    authorize_task_action(tx, ctx, "task.resolve", task, owner).await?;
    Ok(result)
}
