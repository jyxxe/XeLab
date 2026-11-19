//! Freeze the workflow, business input, exact targets and original grant paths
//! shared by immediate tasks and versioned schedule templates.
use super::{authority, targets};
use crate::{
    auth::{Context, Resource},
    contracts::{reject_jsonb_nul, unsigned, validate, validate_business},
    error::{Error, Result},
    policy::{self, Path},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub(crate) const DEFAULT_TASK_TIMEOUT_MS: u64 = 86_400_000;

pub(crate) enum Purpose {
    Task(u64),
    Schedule,
}

pub(crate) struct FrozenWorkflow {
    pub(super) steps: Vec<Value>,
    pub(super) bindings: Vec<targets::StepAdmission>,
    pub(super) submitted: DateTime<Utc>,
    pub(super) deadline: DateTime<Utc>,
    pub(crate) admitted_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) maximum_rules: Value,
    pub(super) record: Value,
    workflow_id: Uuid,
    task_create_paths: Option<Vec<Path>>,
}

impl FrozenWorkflow {
    pub(crate) fn schedule_record(&self) -> Result<Value> {
        let record = json!({
            "version":1,
            "workflow_id":self.workflow_id,
            "task_create_paths":self.task_create_paths.as_ref().ok_or_else(Error::denied)?,
            "task_admission":self.record
        });
        crate::plugin_catalog::reserve_authority_bytes(&mut 0, serde_json::to_vec(&record)?.len())?;
        Ok(record)
    }
}

pub(crate) async fn authorize_workflow_read(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    workflow: Uuid,
) -> Result<()> {
    let owner = c
        .query_opt("SELECT owner_id FROM workflows WHERE id=$1", &[&workflow])
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    let resource = Resource::new("workflow", workflow, Some(owner));
    if !policy::allows(c, ctx, "workflow.read", &resource).await? {
        return Err(Error::not_found());
    }
    Ok(())
}

pub(crate) async fn prepare(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    workflow: Uuid,
    input: &Value,
    purpose: Purpose,
) -> Result<FrozenWorkflow> {
    let row = c
        .query_opt(
            "SELECT owner_id,spec,status FROM workflows WHERE id=$1 FOR SHARE",
            &[&workflow],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let workflow_resource = Resource::new("workflow", workflow, Some(row.get("owner_id")));
    if matches!(purpose, Purpose::Schedule)
        && !policy::allows(c, ctx, "workflow.read", &workflow_resource).await?
    {
        return Err(Error::not_found());
    }
    if row.get::<_, String>("status") != "active" {
        return Err(Error::conflict("Workflow version is archived"));
    }
    let spec: Value = row.get("spec");
    validate("WorkflowSpec", &spec)?;
    reject_jsonb_nul(input)?;
    validate_business(&spec["input_schema"], input)?;
    let timeout = match purpose {
        Purpose::Task(timeout) => timeout,
        Purpose::Schedule => DEFAULT_TASK_TIMEOUT_MS,
    };
    let submitted: DateTime<Utc> = c.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
    let deadline = submitted + Duration::milliseconds(timeout as i64);
    let steps = spec["steps"].as_array().unwrap().clone();
    let mut bindings = Vec::with_capacity(steps.len());
    for step in &steps {
        if unsigned(&step["timeout_ms"])? > timeout {
            return Err(Error::invalid("Workflow step exceeds the task deadline"));
        }
        bindings.push(targets::resolve(c, ctx, step).await?);
    }

    // Freeze only after all provider locks, which can outlive grant conditions.
    let mut workflow_paths = authority::freeze(c, ctx, "workflow.read", &workflow_resource).await?;
    let mut budget = 0;
    crate::plugin_catalog::reserve_authority_bytes(
        &mut budget,
        serde_json::to_vec(&workflow_paths)?.len(),
    )?;
    for binding in &mut bindings {
        for target in &mut binding.targets {
            let paths = authority::freeze(c, ctx, &target.action, &target.resource).await?;
            crate::plugin_catalog::reserve_authority_bytes(
                &mut budget,
                serde_json::to_vec(&paths)?.len(),
            )?;
            target.paths = paths;
        }
    }
    let mut task_create_paths = if matches!(purpose, Purpose::Schedule) {
        // Task creation is a separate target from per-step execution. Keep
        // its proof for the trigger without adding it to the child task ceiling.
        let paths = authority::freeze_task_creation(c, ctx).await?;
        crate::plugin_catalog::reserve_authority_bytes(
            &mut budget,
            serde_json::to_vec(&paths)?.len(),
        )?;
        Some(paths)
    } else {
        None
    };
    let target_paths = bindings
        .iter()
        .flat_map(|binding| &binding.targets)
        .flat_map(|target| &target.paths);
    let grant_ids: Vec<_> = workflow_paths
        .iter()
        .chain(target_paths)
        .chain(task_create_paths.iter().flatten())
        .flat_map(|path| path.sources.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let source_expiries = authority::source_expiries(c, ctx, &grant_ids).await?;
    super::authorize_create(c, ctx).await?;
    let issuer_expires = authority::issuer_expiry(c, ctx).await?;
    let mut expires_at = match purpose {
        Purpose::Task(_) => deadline.min(issuer_expires),
        Purpose::Schedule => issuer_expires,
    };
    let admitted_at: DateTime<Utc> = c.query_one("SELECT clock_timestamp()", &[]).await?.get(0);
    if expires_at <= admitted_at {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            match purpose {
                Purpose::Task(_) => "Task admission exceeded its live deadline",
                Purpose::Schedule => "Schedule admission exceeded its live authority",
            },
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    let mut rules = BTreeMap::new();
    let mut paths_expire_at = collect_live_rules(
        &mut workflow_paths,
        ctx,
        admitted_at,
        &source_expiries,
        &mut rules,
    )?;
    for binding in &mut bindings {
        for target in &mut binding.targets {
            paths_expire_at = paths_expire_at.min(collect_live_rules(
                &mut target.paths,
                ctx,
                admitted_at,
                &source_expiries,
                &mut rules,
            )?);
        }
    }
    if let Some(paths) = task_create_paths.as_mut() {
        paths_expire_at = paths_expire_at.min(retain_live_sources(
            paths,
            ctx,
            admitted_at,
            &source_expiries,
        )?);
        expires_at = expires_at.min(paths_expire_at);
    }
    let record = json!({
        "version": 1,
        "peer": ctx.peer,
        "browser_origin": ctx.browser_origin,
        "credential_chain": ctx.chain,
        "workflow_paths": workflow_paths,
        "steps": bindings.iter().map(targets::StepAdmission::record).collect::<Vec<_>>(),
    });
    crate::plugin_catalog::reserve_authority_bytes(&mut 0, serde_json::to_vec(&record)?.len())?;
    Ok(FrozenWorkflow {
        steps,
        bindings,
        submitted,
        deadline,
        admitted_at,
        expires_at,
        maximum_rules: Value::Array(rules.into_values().collect()),
        record,
        workflow_id: workflow,
        task_create_paths,
    })
}

pub(super) fn collect_live_rules(
    paths: &mut Vec<Path>,
    ctx: &Context,
    checked_at: DateTime<Utc>,
    source_expiries: &HashMap<Uuid, DateTime<Utc>>,
    rules: &mut BTreeMap<String, Value>,
) -> Result<DateTime<Utc>> {
    let expiry = retain_live_sources(paths, ctx, checked_at, source_expiries)?;
    for path in paths {
        rules
            .entry(serde_json::to_string(&path.rule)?)
            .or_insert_with(|| path.rule.clone());
    }
    Ok(expiry)
}

fn retain_live_sources(
    paths: &mut Vec<Path>,
    ctx: &Context,
    checked_at: DateTime<Utc>,
    source_expiries: &HashMap<Uuid, DateTime<Utc>>,
) -> Result<DateTime<Utc>> {
    paths.retain(|path| {
        path.sources.iter().all(|grant| {
            source_expiries
                .get(grant)
                .is_some_and(|expiry| *expiry > checked_at)
        })
    });
    authority::retain_live_paths(paths, ctx, checked_at)?;
    // A target remains valid while at least one of its original paths is live.
    let mut latest = checked_at;
    for path in paths {
        let condition_expiry = path.rule["conditions"]
            .get("expires_at")
            .map(policy::timestamp)
            .transpose()?
            .unwrap_or(DateTime::<Utc>::MAX_UTC);
        let expiry = path
            .sources
            .iter()
            .filter_map(|grant| source_expiries.get(grant))
            .fold(condition_expiry, |expiry, source| expiry.min(*source));
        latest = latest.max(expiry);
    }
    Ok(latest)
}
