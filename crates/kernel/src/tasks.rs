//! Task submission persists metadata and an authorization ceiling. Execution
//! must revalidate the original grants and provider binding before each step.
use crate::{
    auth::{self, Auth, Context, Resource},
    contracts::{id, string, unsigned, validate, validate_business},
    error::{Error, Result},
};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::collections::HashMap;
use tokio_postgres::{GenericClient, Row, Transaction};
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
    let task = TaskSubmission {
        owner: ctx.subject,
        issuer: ctx.credential,
        workflow,
        input: &body["input"],
        proof: &admission.record,
        maximum: &admission.maximum_rules,
        submitted: admission.submitted,
        deadline: admission.deadline,
        expires: admission.expires_at,
    }
    .persist(
        c,
        admission
            .steps
            .iter()
            .zip(admission.bindings.iter().map(|bound| bound.deployment_id())),
    )
    .await?;
    crate::plugin_catalog::read_task(c, task, None)
        .await?
        .ok_or_else(Error::not_found)
}

struct TaskSubmission<'a> {
    owner: Uuid,
    issuer: Uuid,
    workflow: Uuid,
    input: &'a Value,
    proof: &'a Value,
    maximum: &'a Value,
    submitted: DateTime<Utc>,
    deadline: DateTime<Utc>,
    expires: DateTime<Utc>,
}

impl<'a> TaskSubmission<'a> {
    async fn persist(
        &self,
        c: &(impl GenericClient + Sync),
        steps: impl Iterator<Item = (&'a Value, Option<Uuid>)> + Send,
    ) -> Result<Uuid> {
        let task = Uuid::new_v4();
        let delegation = Uuid::new_v4();
        c.execute(
            "INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,created_at)
             VALUES($1,$2,$3,$4,'user',$5,'active',$6)",
            &[&delegation, &self.owner, &self.issuer, self.maximum, &self.expires, &self.submitted],
        ).await?;
        c.execute(
            "INSERT INTO tasks(id,owner_id,workflow_id,delegation_id,input,deadline_at,state,created_at,updated_at,admission)
             VALUES($1,$2,$3,$4,$5,$6,'queued',$7,$7,$8)",
            &[&task, &self.owner, &self.workflow, &delegation, self.input, &self.deadline, &self.submitted, self.proof],
        ).await?;
        for (step, deployment) in steps {
            let root = step["depends_on"]
                .as_array()
                .ok_or_else(Error::denied)?
                .is_empty();
            let state = if root { "ready" } else { "pending" };
            c.execute(
                "INSERT INTO task_steps(id,task_id,step_key,provider_deployment_id,state,created_at,updated_at)
                 VALUES($1,$2,$3,$4,$5,$6,$6)",
                &[&Uuid::new_v4(), &task, &string(step, "id")?, &deployment, &state, &self.submitted],
            ).await?;
        }
        Ok(task)
    }
}

pub(crate) async fn create_scheduled(
    tx: &Transaction<'_>,
    auth: &Auth,
    schedule: &Row,
    spec: &Value,
) -> Result<(Uuid, DateTime<Utc>, Context)> {
    if !schedule.try_get::<_, bool>("delegation_live")? {
        return Err(Error::denied());
    }
    let workflow = id(spec, "workflow_id")?;
    let row = tx
        .query_opt(
            "SELECT owner_id,spec,status FROM workflows WHERE id=$1 FOR SHARE",
            &[&workflow],
        )
        .await?
        .ok_or_else(Error::denied)?;
    if row.try_get::<_, &str>("status")? != "active" {
        return Err(Error::new(
            "SCHEDULE_WORKFLOW_ARCHIVED",
            "The scheduled workflow version is archived",
            StatusCode::CONFLICT,
        ));
    }
    let workflow_resource = Resource::new("workflow", workflow, Some(row.try_get("owner_id")?));
    let workflow_spec: Value = row.try_get("spec")?;
    validate("WorkflowSpec", &workflow_spec)?;
    validate_business(&workflow_spec["input_schema"], &spec["input"])?;
    let (ctx, cutoff, deployments) = claim_authority::revalidate_schedule(
        tx,
        auth,
        schedule,
        workflow_resource,
        &workflow_spec,
        Uuid::new_v4(),
    )
    .await?;
    let submitted: DateTime<Utc> = tx.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
    let deadline = submitted + Duration::milliseconds(admission::DEFAULT_TASK_TIMEOUT_MS as i64);
    let expires = cutoff.min(schedule.try_get("expires_at")?).min(deadline);
    if expires <= submitted {
        return Err(Error::denied());
    }
    let proof: Value = schedule.try_get("admission")?;
    let maximum: Value = schedule.try_get("maximum_rules")?;
    let task = TaskSubmission {
        owner: ctx.subject,
        issuer: ctx.credential,
        workflow,
        input: &spec["input"],
        proof: &proof["task_admission"],
        maximum: &maximum,
        submitted,
        deadline,
        expires,
    }
    .persist(
        tx,
        workflow_spec["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(deployments),
    )
    .await?;
    Ok((task, expires, ctx))
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
