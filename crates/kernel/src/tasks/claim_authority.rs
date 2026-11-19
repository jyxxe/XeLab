//! Revalidate the proof persisted by task submission under the claim's authorization fence.
use super::{authority, targets::Target};
use crate::{
    auth::{Auth, Context, Resource},
    contracts::{id, string},
    error::{Error, Result},
    policy::{Path, Policy},
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    net::IpAddr,
};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

fn provider_unavailable() -> Error {
    Error::new(
        "TASK_PROVIDER_UNAVAILABLE",
        "The admitted task provider binding is unavailable",
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    )
}

// Skill validation can identify a changed signed candidate or immutable contract.
// In particular, NOT_READY can come from PostgreSQL or the schema compiler.
fn binding_error(error: Error) -> Error {
    match error.code {
        "RESOURCE_NOT_FOUND" | "CONTRACT_CONFLICT" | "STATE_CONFLICT" => provider_unavailable(),
        _ => error,
    }
}

pub(super) async fn revalidate(
    tx: &Transaction<'_>,
    auth: &Auth,
    candidate: &Row,
    step: &Value,
    worker_id: Uuid,
) -> Result<DateTime<Utc>> {
    Ok(
        revalidate_proof(tx, auth, candidate, step, worker_id, false)
            .await?
            .until,
    )
}

pub(super) struct Revalidated {
    pub ctx: Context,
    pub business_paths: Vec<Path>,
    pub until: DateTime<Utc>,
}

pub(super) async fn resume_original(
    tx: &Transaction<'_>,
    auth: &Auth,
    candidate: &Row,
    request_id: Uuid,
    admission: &Value,
) -> Result<Context> {
    if admission["version"] != 1 {
        return Err(Error::denied());
    }
    let issuer = candidate
        .try_get::<_, Option<Uuid>>("issuer_credential_id")?
        .ok_or_else(Error::credential)?;
    let peer =
        serde_json::from_value(admission["peer"].clone()).map_err(|_| Error::credential())?;
    let mut ctx = auth.resume(tx, issuer, peer, request_id).await?;
    if ctx.subject != candidate.try_get::<_, Uuid>("subject_id")?
        || ctx.subject != candidate.try_get::<_, Uuid>("owner_id")?
        || admission["credential_chain"] != json!(ctx.chain)
    {
        return Err(Error::credential());
    }
    ctx.browser_origin = serde_json::from_value(
        admission
            .get("browser_origin")
            .ok_or_else(Error::credential)?
            .clone(),
    )
    .map_err(|_| Error::credential())?;
    Ok(ctx)
}

pub(super) async fn revalidate_proof(
    tx: &Transaction<'_>,
    auth: &Auth,
    candidate: &Row,
    step: &Value,
    request_id: Uuid,
    for_dispatch: bool,
) -> Result<Revalidated> {
    let admission: Value = candidate.try_get("admission")?;
    let ctx = resume_original(tx, auth, candidate, request_id, &admission).await?;
    let step_key = string(step, "id")?;
    let mut matches = admission["steps"]
        .as_array()
        .ok_or_else(Error::denied)?
        .iter()
        .filter(|record| record["step_key"] == step_key);
    let record = matches.next().ok_or_else(Error::denied)?;
    if matches.next().is_some() {
        return Err(Error::denied());
    }
    let deployment: Option<Uuid> = candidate.try_get("provider_deployment_id")?;
    check_deployment(step, record, deployment)?;

    let workflow = Resource::new(
        "workflow",
        candidate.try_get::<_, Uuid>("workflow_id")?,
        Some(candidate.try_get("workflow_owner_id")?),
    );
    let mut targets = vec![original_target(
        "workflow.read",
        workflow,
        &admission["workflow_paths"],
    )?];
    targets.extend(step_targets(tx, step, record, deployment).await?);

    let maximum: Value = candidate.try_get("maximum_rules")?;
    let ceiling: Vec<_> = maximum
        .as_array()
        .ok_or_else(Error::denied)?
        .iter()
        .map(|rule| Path {
            rule: rule.clone(),
            sources: BTreeSet::new(),
        })
        .collect();
    let mut proofs = Vec::with_capacity(targets.len());
    for target in targets {
        let current = authority::freeze(tx, &ctx, &target.action, &target.resource).await?;
        proofs.push((target, current));
    }

    let issuer_expiry = authority::issuer_expiry(tx, &ctx).await?;
    let now = tx
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .try_get(0)?;
    if issuer_expiry <= now {
        return Err(Error::credential());
    }
    let mut until = issuer_expiry;
    let mut business_paths = Vec::new();
    for (target, current) in proofs {
        let mut policy = Policy::for_context(tx, &ctx).with_resource_owner(&target.resource);
        let matches_target = |path: &Path| {
            path.rule["action"] == target.action
                && crate::auth::selector_matches(&path.rule["resource"], &target.resource)
        };
        let maximum_until = longest_expiry(
            ceiling.iter().filter(|p| matches_target(p)),
            ctx.peer,
            &HashMap::new(),
            now,
        )
        .ok_or_else(Error::denied)?;
        let issuer_cutoffs = policy
            .live_grant_issuer_expiries(&source_ids(&current))
            .await?;
        let current_until = target_expiry(&current, ctx.peer, &issuer_cutoffs, now)?;

        // At this fixed target and peer, each original grant independently
        // needs one live expansion. Its cutoff includes every issuer in that
        // expansion, including the grant's own issuer. A missing grant only
        // withdraws the saved alternatives that required it.
        crate::policy::bounded(&target.paths)?;
        let mut source_rights = HashMap::new();
        for source in source_ids(&target.paths) {
            let mut grants = policy.grant(source, 1).await?;
            grants.retain(&matches_target);
            if grants.is_empty() {
                continue;
            }
            let sources = source_ids(&grants);
            // Commonly the current proof already named every issuer we need.
            let extra_cutoffs;
            let cutoffs = if sources.iter().all(|id| issuer_cutoffs.contains_key(id)) {
                &issuer_cutoffs
            } else {
                extra_cutoffs = policy.live_grant_issuer_expiries(&sources).await?;
                &extra_cutoffs
            };
            if let Some(cutoff) = longest_expiry(&grants, ctx.peer, cutoffs, now) {
                source_rights.insert(source, cutoff);
            }
        }
        // For each saved alternative intersect its conditions with ALL its
        // original grants, then choose the longest surviving alternative.
        let saved_until = target_expiry(&target.paths, ctx.peer, &source_rights, now)?;
        until = until.min(saved_until).min(current_until).min(maximum_until);
        if for_dispatch && target.action == "capability.invoke" {
            business_paths = policy.intersection(&target.paths, &ceiling).await?;
        }
    }
    Ok(Revalidated {
        ctx,
        business_paths,
        until,
    })
}

pub(super) async fn revalidate_schedule(
    tx: &Transaction<'_>,
    auth: &Auth,
    schedule: &Row,
    workflow: Resource,
    workflow_spec: &Value,
    request_id: Uuid,
) -> Result<(Context, DateTime<Utc>, Vec<Option<Uuid>>)> {
    let proof: Value = schedule.try_get("admission")?;
    let admission = &proof["task_admission"];
    if proof["version"] != 1 || proof["workflow_id"] != workflow.id {
        return Err(Error::denied());
    }
    let ctx = resume_original(tx, auth, schedule, request_id, admission).await?;
    let mut targets = vec![
        original_target("workflow.read", workflow, &admission["workflow_paths"])?,
        original_target(
            "task.create",
            Resource::new("task", "new", Some(ctx.subject)),
            &proof["task_create_paths"],
        )?,
    ];
    let maximum: Value = schedule.try_get("maximum_rules")?;
    let mut ceiling = ceiling(&maximum)?;
    // Task creation was frozen separately and is never carried into the child
    // Task's per-step execution ceiling.
    ceiling.extend(targets[1].paths.iter().map(|path| Path {
        rule: path.rule.clone(),
        sources: BTreeSet::new(),
    }));

    let steps = workflow_spec["steps"]
        .as_array()
        .ok_or_else(Error::denied)?;
    let records = admission["steps"].as_array().ok_or_else(Error::denied)?;
    if steps.len() != records.len() {
        return Err(Error::denied());
    }
    let mut deployments = Vec::with_capacity(steps.len());
    for (step, record) in steps.iter().zip(records) {
        if record["step_key"] != string(step, "id")? {
            return Err(Error::denied());
        }
        let deployment = stored_uuid(record, "deployment_id")?;
        check_deployment(step, record, deployment)?;
        deployments.push(deployment);
        targets.extend(step_targets(tx, step, record, deployment).await?);
    }
    let until = revalidate_schedule_targets(tx, &ctx, targets, &ceiling).await?;
    Ok((ctx, until, deployments))
}

fn ceiling(maximum: &Value) -> Result<Vec<Path>> {
    Ok(maximum
        .as_array()
        .ok_or_else(Error::denied)?
        .iter()
        .map(|rule| Path {
            rule: rule.clone(),
            sources: BTreeSet::new(),
        })
        .collect())
}

async fn revalidate_schedule_targets(
    tx: &Transaction<'_>,
    ctx: &Context,
    targets: Vec<Target>,
    ceiling: &[Path],
) -> Result<DateTime<Utc>> {
    let mut checked = Vec::with_capacity(targets.len());
    for target in targets {
        let current = if target.action == "task.create" {
            authority::freeze_task_creation(tx, ctx).await?
        } else {
            authority::freeze(tx, ctx, &target.action, &target.resource).await?
        };
        let policy = Policy::for_context(tx, ctx).with_resource_owner(&target.resource);
        crate::policy::bounded(&target.paths)?;
        let mut combined = Vec::new();
        // Current expansion has already walked the complete live issuer
        // graph and narrowed every path to this exact resource. Match its
        // source set before intersecting conditions: an independent current
        // grant must not lend authority to a detached original source.
        for original in &target.paths {
            let matching: Vec<_> = current
                .iter()
                .filter(|path| path.sources == original.sources)
                .cloned()
                .collect();
            combined.extend(
                policy
                    .intersection(std::slice::from_ref(original), &matching)
                    .await?,
            );
            crate::policy::bounded(&combined)?;
        }
        let paths = policy.intersection(&combined, ceiling).await?;
        let expires = policy
            .live_grant_issuer_expiries(&source_ids(&paths))
            .await?;
        checked.push((paths, expires));
    }
    let mut until = authority::issuer_expiry(tx, ctx).await?;
    let now = tx
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .try_get(0)?;
    if until <= now {
        return Err(Error::credential());
    }
    for (paths, expires) in checked {
        until = until.min(target_expiry(&paths, ctx.peer, &expires, now)?);
    }
    Ok(until)
}

/// Bound the issued Call by any surviving alternative that quota charging may choose.
pub(super) async fn dispatch_paths_until(
    tx: &Transaction<'_>,
    ctx: &Context,
    paths: &[Path],
) -> Result<DateTime<Utc>> {
    let cutoffs = Policy::for_context(tx, ctx)
        .live_grant_issuer_expiries(&source_ids(paths))
        .await?;
    let now = tx
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .try_get(0)?;
    paths
        .iter()
        .filter_map(|path| longest_expiry(std::iter::once(path), ctx.peer, &cutoffs, now))
        .min()
        .ok_or_else(Error::denied)
}

#[cfg(test)]
#[path = "claim_paths_tests.rs"]
mod claim_paths_tests;

fn source_ids(paths: &[Path]) -> Vec<Uuid> {
    paths
        .iter()
        .flat_map(|p| p.sources.iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn target_expiry(
    paths: &[Path],
    peer: IpAddr,
    expiries: &HashMap<Uuid, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    longest_expiry(paths, peer, expiries, now).ok_or_else(Error::denied)
}

fn longest_expiry<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    peer: IpAddr,
    expiries: &HashMap<Uuid, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    paths
        .into_iter()
        .filter_map(|path| {
            let mut until = DateTime::<Utc>::MAX_UTC;
            if let Some(conditions) = path.rule.get("conditions") {
                if !crate::auth::conditions_match(conditions, peer, now) {
                    return None;
                }
                if let Some(expiry) = conditions.get("expires_at") {
                    until = crate::policy::timestamp(expiry).ok()?;
                }
            }
            for source in &path.sources {
                until = until.min(*expiries.get(source)?);
            }
            (until > now).then_some(until)
        })
        .max()
}

fn original_target(action: &str, resource: Resource, paths: &Value) -> Result<Target> {
    let paths: Vec<Path> = serde_json::from_value(paths.clone()).map_err(|_| Error::denied())?;
    let selector = resource.json();
    // Task admission freezes business actions. None of these targets is a
    // source-less identity self-service permission.
    if paths.is_empty()
        || paths.iter().any(|p| {
            p.sources.is_empty() || p.rule["action"] != action || p.rule["resource"] != selector
        })
    {
        return Err(Error::denied());
    }
    Ok(Target {
        action: action.into(),
        resource,
        paths,
    })
}

fn bound_target(record: &Value, action: &str, resource: Resource) -> Result<Target> {
    if record["action"] != action
        || record["resource"] != resource.json()
        || record.get("owner_id") != Some(&json!(resource.owner))
    {
        return Err(Error::denied());
    }
    original_target(action, resource, &record["paths"])
}

fn stored_uuid(record: &Value, field: &str) -> Result<Option<Uuid>> {
    serde_json::from_value(record.get(field).ok_or_else(Error::denied)?.clone())
        .map_err(|_| Error::denied())
}

pub(super) fn check_deployment(
    step: &Value,
    record: &Value,
    deployment: Option<Uuid>,
) -> Result<()> {
    let explicit = step
        .get("provider_deployment_id")
        .map(|_| id(step, "provider_deployment_id"))
        .transpose()?;
    if stored_uuid(record, "deployment_id")? != deployment
        || explicit.is_some_and(|fixed| Some(fixed) != deployment)
    {
        return Err(Error::denied());
    }
    Ok(())
}

fn stored_digest<'a>(binding: &'a Value, field: &str) -> Result<&'a str> {
    binding[field]
        .as_str()
        .filter(|value| {
            value.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        })
        .ok_or_else(Error::denied)
}

// Older rows have only a version check in PostgreSQL. Check the persisted proof
// before a missing provider could mask malformed fields as a dependency wait.
pub(super) fn validate_binding(
    binding: &Value,
    step: &Value,
    skill: Option<Uuid>,
    deployment: Option<Uuid>,
) -> Result<()> {
    if let Some(skill) = skill {
        if deployment.is_none() || id(binding, "skill_id").map_err(|_| Error::denied())? != skill {
            return Err(Error::denied());
        }
        stored_digest(binding, "definition_digest")?;
        if binding["kind"] == "skill_http" {
            stored_digest(binding, "http_contract_digest")?;
            let op = binding["operation_id"].as_str().ok_or_else(Error::denied)?;
            if op.is_empty()
                || op.len() > 128
                || op.chars().any(char::is_control)
                || !matches!(
                    binding["method"].as_str(),
                    Some("GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE")
                )
            {
                return Err(Error::denied());
            }
            return crate::plugin_http::canonical_path(
                binding["path"].as_str().ok_or_else(Error::denied)?,
            )
            .map_err(|_| Error::denied());
        }
        if binding["kind"] != "skill_capability" {
            return Err(Error::denied());
        }
    } else {
        let routing = if deployment.is_some() {
            "fixed"
        } else {
            "stateless_direct"
        };
        if binding["kind"] != "capability"
            || binding["routing"] != routing
            || binding["contract"] != step["contract"]
        {
            return Err(Error::denied());
        }
    }
    crate::contracts::validate("ContractRef", &binding["contract"]).map_err(|error| {
        if error.code == "INVALID_ARGUMENT" {
            Error::denied()
        } else {
            error
        }
    })
}

async fn step_targets(
    tx: &Transaction<'_>,
    step: &Value,
    record: &Value,
    deployment: Option<Uuid>,
) -> Result<Vec<Target>> {
    let binding = &record["binding"];
    let saved = record["targets"].as_array().ok_or_else(Error::denied)?;
    let skill = step
        .get("skill_id")
        .map(|_| id(step, "skill_id"))
        .transpose()?;
    if saved.len() != if skill.is_some() { 2 } else { 1 } {
        return Err(Error::denied());
    }
    let installation = stored_uuid(record, "installation_id")?;
    validate_binding(binding, step, skill, deployment)?;

    // Admission searches for a deployment. A claim checks the selected rows
    // directly, even if additional eligible deployments have since appeared.
    let fixed = match (deployment, installation) {
        (Some(deployment), Some(installation)) => Some(tx.query_opt(
            "SELECT d.owner_id,n.manifest FROM deployments d JOIN installations n ON n.id=d.installation_id
             JOIN publishers p ON p.id=n.publisher_id WHERE d.id=$1 AND n.id=$2 AND d.archived_at IS NULL
             AND d.spec->>'enabled'='true' AND n.state='installed' AND p.status='trusted' FOR SHARE OF d,n,p",
            &[&deployment, &installation],
        ).await?.ok_or_else(provider_unavailable)?),
        (None, None) if skill.is_none() => None,
        _ => return Err(Error::denied()),
    };

    let mut targets = Vec::with_capacity(saved.len());
    let contract = if let Some(skill) = skill {
        let installation = installation.ok_or_else(Error::denied)?;
        let row = tx.query_opt(
            "SELECT definition,definition_digest,created_by FROM skills WHERE id=$1 AND installation_id=$2 AND enabled
             AND archived_at IS NULL FOR SHARE", &[&skill, &installation],
        ).await?.ok_or_else(provider_unavailable)?;
        targets.push(bound_target(
            &saved[0],
            "skill.invoke",
            Resource::new("skill", skill, Some(row.try_get("created_by")?)),
        )?);
        let definition: Value = row.try_get("definition")?;
        let digest: &str = row.try_get("definition_digest")?;
        // Verify the locked Skill against its own digest and signed candidate first.
        let binding_kind = match definition["target"]["kind"].as_str() {
            Some("http") => {
                crate::skills::check_admitted_http_binding(tx, skill, digest, installation)
                    .await
                    .map_err(binding_error)?;
                "skill_http"
            }
            Some("capability") => {
                crate::skills::check_admitted_capability_binding(tx, skill, digest, installation)
                    .await
                    .map_err(binding_error)?;
                "skill_capability"
            }
            _ => return Err(provider_unavailable()),
        };
        if binding["kind"] != binding_kind || binding["definition_digest"] != digest {
            return Err(Error::denied());
        }
        if binding_kind == "skill_http" {
            let registry = crate::http_contracts::load(tx, installation)
                .await
                .map_err(binding_error)?;
            let op = registry.bind_skill(&definition)?;
            if binding["operation_id"] != op.id
                || binding["http_contract_digest"] != op.contract_digest
                || binding["method"] != op.method.as_str()
                || binding["path"] != op.path
            {
                return Err(Error::denied());
            }
            let owner = fixed
                .as_ref()
                .ok_or_else(Error::denied)?
                .try_get("owner_id")?;
            let resource = Resource::new(
                "plugin",
                string(&definition["target"], "plugin_id")?,
                Some(owner),
            );
            targets.push(bound_target(&saved[1], &op.route.action, resource)?);
            return Ok(targets);
        }
        definition["target"]["contract"].clone()
    } else {
        step["contract"].clone()
    };

    if binding["contract"] != contract {
        // The stored binding disagrees with the immutable workflow or Skill.
        return Err(Error::denied());
    }
    crate::workflows::validate_contract(tx, &contract)
        .await
        .map_err(binding_error)?;
    if let Some(row) = fixed {
        let manifest: Value = row.try_get("manifest")?;
        if !manifest["capabilities"]
            .as_array()
            .is_some_and(|caps| caps.iter().any(|cap| cap["contract"] == contract))
            || (skill.is_none()
                && manifest
                    .get("business_entry")
                    .is_some_and(|entry| entry != "direct"))
        {
            return Err(provider_unavailable());
        }
    } else {
        let filter = json!({"capabilities":[{"contract":contract}]});
        tx.query_opt(
            "SELECT n.id FROM installations n JOIN publishers p ON p.id=n.publisher_id
             WHERE n.state='installed' AND p.status='trusted' AND n.manifest @> $1::jsonb
             AND coalesce(n.manifest->>'business_entry','direct')='direct'
             AND n.manifest->>'data_schema_version'='none' ORDER BY n.id LIMIT 1 FOR SHARE OF n,p",
            &[&filter],
        )
        .await?
        .ok_or_else(provider_unavailable)?;
    }
    let resource = Resource::new(
        "capability",
        format!(
            "{}@{}",
            string(&contract, "name")?,
            string(&contract, "version")?
        ),
        None,
    );
    targets.push(bound_target(
        &saved[targets.len()],
        "capability.invoke",
        resource,
    )?);
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_deployment_compares_uuid_values() {
        let deployment = Uuid::new_v4();
        let record = json!({"deployment_id":deployment});
        let step = json!({"provider_deployment_id":deployment.to_string().to_uppercase()});
        check_deployment(&step, &record, Some(deployment)).unwrap();
        assert!(check_deployment(&step, &record, Some(Uuid::new_v4())).is_err());
    }

    #[test]
    fn malformed_skill_bindings_pause_before_provider_lookup() {
        let skill = Uuid::new_v4();
        let deployment = Some(Uuid::new_v4());
        let step = json!({"skill_id":skill});
        let digest = crate::contracts::digest(b"admitted definition");
        let http = json!({"kind":"skill_http","skill_id":skill,"definition_digest":digest,
            "http_contract_digest":digest,"operation_id":"echo","method":"POST","path":"/plugins/org.example/echo"});
        let capability = json!({"kind":"skill_capability","skill_id":skill,"definition_digest":digest,
            "contract":{"name":"tool.echo","version":"1.0.0","schema_digest":digest}});
        for binding in [http, capability] {
            validate_binding(&binding, &step, Some(skill), deployment).unwrap();
            for field in binding.as_object().unwrap().keys() {
                let mut broken = binding.clone();
                broken.as_object_mut().unwrap().remove(field);
                assert_eq!(
                    validate_binding(&broken, &step, Some(skill), deployment)
                        .unwrap_err()
                        .code,
                    "PERMISSION_DENIED"
                );
                broken[field] = json!(17);
                assert_eq!(
                    validate_binding(&broken, &step, Some(skill), deployment)
                        .unwrap_err()
                        .code,
                    "PERMISSION_DENIED"
                );
            }
        }
    }

    #[test]
    fn source_less_business_path_cannot_be_filled_by_a_current_grant() {
        let resource = Resource::new("capability", "tool.echo@1.0.0", None);
        let rule = json!({"action":"capability.invoke","resource":resource.json()});
        let sourced = json!({"rule":rule,"sources":[Uuid::new_v4()]});
        let empty = json!({"rule":rule,"sources":[]});
        assert!(original_target("capability.invoke", resource.clone(), &json!([sourced])).is_ok());
        for paths in [json!([empty]), json!([sourced, empty])] {
            assert!(
                matches!(original_target("capability.invoke", resource.clone(), &paths),
                Err(error) if error.code == "PERMISSION_DENIED")
            );
        }
    }

    #[test]
    fn cutoff_uses_the_longest_surviving_alternative_with_all_of_its_limits() {
        let now = Utc::now();
        let t = |seconds| now + chrono::Duration::seconds(seconds);
        let peer = "127.0.0.1".parse().unwrap();
        let (a, b, missing) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let paths = serde_json::from_value::<Vec<Path>>(json!([
            {"rule":{"conditions":{}},"sources":[a,b]},
            {"rule":{"conditions":{"expires_at":t(12)}},"sources":[b]},
            {"rule":{"conditions":{}},"sources":[missing]},
            {"rule":{"conditions":{"source_cidrs":["192.0.2.0/24"]}},"sources":[b]}
        ]))
        .unwrap();
        let expiries = HashMap::from([(a, t(5)), (b, t(20))]);
        assert_eq!(target_expiry(&paths, peer, &expiries, now).unwrap(), t(12));
        assert_eq!(
            target_expiry(&paths, peer, &expiries, t(12))
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
    }

    #[tokio::test]
    async fn postgres_missing_provider_and_failed_lookup_have_distinct_codes() {
        let Ok(url) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: task provider lookup requires PostgreSQL");
            return;
        };
        let db = crate::db::Db::new(url).unwrap();
        let mut client = db.connect().await.unwrap();
        let tx = client.transaction().await.unwrap();
        tx.batch_execute(
            "CREATE TEMP TABLE deployments(id uuid,owner_id uuid,installation_id uuid,archived_at timestamptz,spec jsonb);
             CREATE TEMP TABLE installations(id uuid,publisher_id uuid,state text,manifest jsonb);
             CREATE TEMP TABLE publishers(id uuid,status text);",
        ).await.unwrap();
        let contract = json!({"name":"tool.echo","version":"1.0.0","schema_digest":crate::contracts::digest(b"contract")});
        let record = json!({"installation_id":Uuid::new_v4(),"targets":[{}],
            "binding":{"kind":"capability","routing":"fixed","contract":contract}});
        let deployment = Some(Uuid::new_v4());
        let step = json!({"id":"echo","contract":contract});
        assert!(
            matches!(step_targets(&tx, &step, &record, deployment).await,
            Err(error) if error.code == "TASK_PROVIDER_UNAVAILABLE")
        );

        tx.batch_execute("ALTER TABLE pg_temp.deployments RENAME owner_id TO missing_owner")
            .await
            .unwrap();
        assert!(
            matches!(step_targets(&tx, &step, &record, deployment).await,
            Err(error) if error.code == "NOT_READY")
        );
        tx.rollback().await.unwrap();
    }
}
