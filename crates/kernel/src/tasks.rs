//! Task submission persists metadata and an authorization ceiling. Execution
//! must revalidate the original grants and provider binding before each step.
use crate::{
    auth::{self, Auth, Context, Resource},
    contracts::{id, string, unsigned},
    error::{Error, Result},
};
use axum::http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub(crate) mod admission;
mod authority;
mod cancel;
mod claim_authority;
mod commit_effects;
mod dispatch_input;
mod reads;
mod resolve;
#[cfg(test)]
mod restart_tests;
pub(crate) mod retry;
pub(crate) mod snapshot;
mod targets;
pub mod task_engine;

pub(crate) fn handles(name: &str) -> bool {
    matches!(
        name,
        "createTask"
            | "cancelTask"
            | "resolveUnknownStep"
            | "retryTask"
            | "getTask"
            | "listTasks"
            | "listTaskSteps"
            | "listAttempts"
    )
}

pub(crate) fn validate_query(name: &str, query: &str) -> Result<()> {
    if matches!(
        name,
        "createTask" | "cancelTask" | "resolveUnknownStep" | "retryTask" | "getTask"
    ) && !query.is_empty()
    {
        return Err(Error::invalid("Unexpected task query parameter"));
    }
    Ok(())
}

pub(crate) async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
) -> Result<()> {
    if name == "cancelTask" {
        return cancel::authorize(c, ctx, crate::api::path_id(params, "task_id")?).await;
    }
    if name == "resolveUnknownStep" {
        return resolve::authorize(c, ctx, crate::api::path_id(params, "step_id")?).await;
    }
    if name == "retryTask" {
        return retry::authorize(c, ctx, crate::api::path_id(params, "task_id")?).await;
    }
    if name != "createTask" {
        return reads::authorize(c, ctx, name, params).await;
    }
    authorize_create(c, ctx).await?;
    admission::authorize_workflow_read(c, ctx, id(body, "workflow_id")?).await
}

async fn authorize_task_action(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    task: Uuid,
    owner: Uuid,
) -> Result<()> {
    let resource = Resource::new("task", task, Some(owner));
    let result = auth::authorize(c, ctx, action, &resource).await;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "PERMISSION_DENIED")
        && !crate::policy::allows(c, ctx, "task.read", &resource).await?
    {
        return Err(Error::not_found());
    }
    result
}

pub(crate) async fn authorize_create(c: &(impl GenericClient + Sync), ctx: &Context) -> Result<()> {
    auth::authorize(
        c,
        ctx,
        "task.create",
        &Resource::new("task", "new", Some(ctx.subject)),
    )
    .await
}

#[allow(clippy::too_many_arguments)] // Shared HTTP dispatch inputs.
pub(crate) async fn execute(
    c: &Transaction<'_>,
    auth: &Auth,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    match name {
        "createTask" => Ok((create(c, ctx, body).await?, 201)),
        "cancelTask" => Ok((
            cancel::request(c, ctx, crate::api::path_id(params, "task_id")?, headers).await?,
            200,
        )),
        "resolveUnknownStep" => Ok((
            resolve::apply(
                c,
                ctx,
                crate::api::path_id(params, "step_id")?,
                headers,
                body,
            )
            .await?,
            200,
        )),
        _ => reads::execute(c, auth, ctx, name, params, query).await,
    }
}

async fn create(c: &(impl GenericClient + Sync), ctx: &Context, body: &Value) -> Result<Value> {
    if body["run_as"] == "service" {
        // The contract requires a separate service approval. A workflow or a
        // task.create grant never substitutes for that approval.
        return Err(Error::denied());
    }
    if body.get("service_account_id").is_some() {
        return Err(Error::invalid(
            "A user task cannot specify a service account",
        ));
    }
    let workflow = id(body, "workflow_id")?;
    let timeout = body
        .get("timeout_ms")
        .map(unsigned)
        .transpose()?
        .unwrap_or(admission::DEFAULT_TASK_TIMEOUT_MS);
    let admission = admission::prepare(
        c,
        ctx,
        workflow,
        &body["input"],
        admission::Purpose::Task(timeout),
    )
    .await?;
    let task = Uuid::new_v4();
    let delegation = Uuid::new_v4();
    c.execute(
        "INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,created_at)
         VALUES($1,$2,$3,$4,'user',$5,'active',$6)",
        &[&delegation, &ctx.subject, &ctx.credential, &admission.maximum_rules, &admission.expires_at, &admission.submitted],
    ).await?;
    c.execute(
        "INSERT INTO tasks(id,owner_id,workflow_id,delegation_id,input,deadline_at,state,created_at,updated_at,admission)
         VALUES($1,$2,$3,$4,$5,$6,'queued',$7,$7,$8)",
        &[&task, &ctx.subject, &workflow, &delegation, &body["input"], &admission.deadline, &admission.submitted, &admission.record],
    ).await?;
    for (step, binding) in admission.steps.iter().zip(&admission.bindings) {
        let state = if step["depends_on"].as_array().unwrap().is_empty() {
            "ready"
        } else {
            "pending"
        };
        c.execute(
            "INSERT INTO task_steps(id,task_id,step_key,provider_deployment_id,state,created_at,updated_at)
             VALUES($1,$2,$3,$4,$5,$6,$6)",
            &[&Uuid::new_v4(), &task, &string(step, "id")?, &binding.deployment_id(), &state, &admission.submitted],
        ).await?;
    }
    crate::plugin_catalog::read_task(c, task, None)
        .await?
        .ok_or_else(Error::not_found)
}

/// Controller restart revokes transient authority. Submitted tasks and schedules
/// retain their original, time-bounded proof for later revalidation.
pub(crate) async fn revoke_transient_delegations(c: &(impl GenericClient + Sync)) -> Result<()> {
    c.execute(
        "UPDATE execution_delegations e SET status='revoked' WHERE e.status='active'
         AND NOT EXISTS (SELECT FROM tasks t WHERE t.delegation_id=e.id AND t.admission IS NOT NULL)
         AND NOT EXISTS (
             SELECT FROM schedules s WHERE s.delegation_id=e.id AND s.admission IS NOT NULL
               AND s.owner_id=e.subject_id AND e.issuer_credential_id IS NOT NULL
               AND e.run_as='user' AND e.service_account_id IS NULL
               AND NOT EXISTS (SELECT FROM calls WHERE delegation_id=s.delegation_id)
         )",
        &[],
    )
    .await?;
    Ok(())
}
