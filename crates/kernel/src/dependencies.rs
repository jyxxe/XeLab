//! Deterministic ordinary-plugin dependencies. A fixed binding is routing
//! provenance, never reusable authorization or an extra resource reservation.
use crate::{
    api::App,
    auth::{self, Context, Resource},
    contracts::{id, string, validate},
    error::{Error, Result},
    policy::{self, Policy},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub async fn dependency_lock(c: &(impl GenericClient + Sync)) -> Result<()> {
    crate::db::call_lock(c).await?;
    c.query_one("SELECT pg_advisory_xact_lock(6360564834937832451)", &[])
        .await?;
    Ok(())
}

/// Deliberately smaller than semver::VersionReq: no caret, wildcard, OR,
/// implicit equality, comma, or prerelease admitted through a broad range.
pub fn version_matches(range: &str, version: &str) -> Result<bool> {
    let parsed = semver::Version::parse(version)
        .map_err(|_| Error::invalid("Invalid dependency version"))?;
    if let Ok(exact) = semver::Version::parse(range) {
        return Ok(parsed == exact);
    }
    let parts: Vec<_> = range.split(' ').collect();
    if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
        return Err(Error::invalid(
            "Dependency range must be exact or a space-separated comparison intersection",
        ));
    }
    let mut matches = parsed.pre.is_empty();
    for part in parts {
        let (op, value) = if let Some(value) = part.strip_prefix(">=") {
            (">=", value)
        } else if let Some(value) = part.strip_prefix('<') {
            ("<", value)
        } else {
            return Err(Error::invalid("Unsupported dependency version comparison"));
        };
        let bound = semver::Version::parse(value)
            .map_err(|_| Error::invalid("Invalid dependency version bound"))?;
        if !bound.pre.is_empty() || !bound.build.is_empty() {
            return Err(Error::invalid(
                "Prerelease dependencies require an exact version",
            ));
        }
        let order = parsed.cmp_precedence(&bound);
        matches &= match op {
            ">=" => !order.is_lt(),
            "<=" => !order.is_gt(),
            ">" => order.is_gt(),
            _ => order.is_lt(),
        };
    }
    Ok(matches)
}

pub fn validate_manifest(manifest: &Value) -> Result<()> {
    let Some(dependencies) = manifest.get("dependencies") else {
        return Ok(());
    };
    let dependencies = dependencies
        .as_array()
        .ok_or_else(|| Error::invalid("Dependencies must be an array"))?;
    let mut aliases = BTreeSet::new();
    for dependency in dependencies {
        validate("DependencySpec", dependency)?;
        if !aliases.insert(string(dependency, "name")?) {
            return Err(Error::invalid("Duplicate dependency alias"));
        }
        version_matches(string(dependency, "version_range")?, "0.0.0")?;
        let features = dependency["features"].as_array().unwrap();
        if features
            .iter()
            .map(Value::to_string)
            .collect::<BTreeSet<_>>()
            .len()
            != features.len()
        {
            return Err(Error::invalid("Duplicate dependency feature"));
        }
    }
    Ok(())
}

pub fn validate_graph(graph: &BTreeMap<Uuid, Vec<Uuid>>) -> Result<()> {
    fn visit(
        node: Uuid,
        graph: &BTreeMap<Uuid, Vec<Uuid>>,
        path: &mut BTreeSet<Uuid>,
        memo: &mut BTreeMap<Uuid, usize>,
    ) -> Result<usize> {
        if path.contains(&node) {
            return Err(Error::conflict("Strong dependency cycle"));
        }
        if let Some(depth) = memo.get(&node) {
            return Ok(*depth);
        }
        if path.len() >= 16 {
            return Err(Error::conflict("Dependency graph exceeds 16 levels"));
        }
        path.insert(node);
        let mut depth = 1;
        for target in graph.get(&node).into_iter().flatten() {
            depth = depth.max(1 + visit(*target, graph, path, memo)?);
        }
        path.remove(&node);
        if depth > 16 {
            return Err(Error::conflict("Dependency graph exceeds 16 levels"));
        }
        memo.insert(node, depth);
        Ok(depth)
    }
    if graph.len() > 4096 {
        return Err(Error::exhausted());
    }
    let mut memo = BTreeMap::new();
    for node in graph.keys() {
        visit(*node, graph, &mut BTreeSet::new(), &mut memo)?;
    }
    Ok(())
}

async fn graph(c: &(impl GenericClient + Sync), consumer: Uuid, bindings: &[Value]) -> Result<()> {
    let rows = c.query("SELECT b.consumer_deployment_id,b.provider_deployment_id,b.bound_call FROM dependency_bindings b LEFT JOIN calls q ON q.id=b.bound_call WHERE b.required AND (b.bound_call IS NULL OR (b.expires_at>now() AND q.state='running')) ORDER BY b.consumer_deployment_id,b.provider_deployment_id LIMIT 4097", &[]).await?;
    if rows.len() > 4096 {
        return Err(Error::exhausted());
    }
    let mut graph: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
    for row in rows {
        let source: Uuid = row.get(0);
        if source != consumer || row.get::<_, Option<Uuid>>(2).is_some() {
            graph.entry(source).or_default().push(row.get(1));
        }
    }
    for binding in bindings {
        if binding["required"] == true {
            graph
                .entry(consumer)
                .or_default()
                .push(id(binding, "deployment_id")?);
        }
    }
    validate_graph(&graph)
}

async fn selected(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    consumer: Uuid,
    installation: Uuid,
) -> Result<(i64, Vec<Value>, Vec<String>)> {
    selected_filtered(c, Some(ctx), consumer, installation, None, None).await
}

async fn selected_filtered(
    c: &(impl GenericClient + Sync),
    ctx: Option<&Context>,
    consumer: Uuid,
    installation: Uuid,
    exact: Option<(&str, &Value, Option<Uuid>)>,
    ceiling: Option<&Value>,
) -> Result<(i64, Vec<Value>, Vec<String>)> {
    let row = c.query_opt("SELECT d.revision,n.manifest FROM deployments d JOIN installations n ON n.id=$2 JOIN publishers p ON p.id=n.publisher_id WHERE d.id=$1 AND d.archived_at IS NULL AND n.state='installed' AND p.status='trusted'", &[&consumer,&installation]).await?.ok_or_else(Error::not_found)?;
    let revision = row.get("revision");
    let manifest: Value = row.get("manifest");
    validate_manifest(&manifest)?;
    let mut bindings = Vec::new();
    let mut conflicts = Vec::new();
    for dependency in manifest["dependencies"].as_array().into_iter().flatten() {
        let alias = string(dependency, "name")?;
        if exact.is_some_and(|(requested, _, _)| requested != alias) {
            continue;
        }
        // Signed manifests/contract records are authoritative. Features come from
        // the core-validated health observation, never a caller address.
        let rows = c.query("SELECT d.id,d.installation_id,d.spec AS provider_spec,i.generation AS provider_generation,n.manifest,n.manifest_digest,n.plugin_id,n.version,i.id AS instance_id,p.name,p.version AS capability_version,p.features,k.schema_digest,k.spec,(p.availability='ready' AND p.observed_generation=i.generation AND p.observed_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint)) AS capability_ready,(i.last_heartbeat_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint)) AS plugin_ready FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers u ON u.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id LEFT JOIN provider_records p ON p.instance_id=i.id LEFT JOIN capability_contracts k ON k.name=p.name AND k.version=p.version WHERE d.spec->>'enabled'='true' AND d.archived_at IS NULL AND n.state='installed' AND u.status='trusted' AND i.phase='ready' ORDER BY d.id,p.name,p.version,i.id LIMIT 4097", &[]).await?;
        if rows.len() > 4096 {
            return Err(Error::exhausted());
        }
        let mut chosen = None;
        for candidate in rows {
            if exact.is_some_and(|(_, _, fixed)| {
                fixed.is_some_and(|provider| provider != candidate.get::<_, Uuid>("id"))
            }) {
                continue;
            }
            let plugin: String = candidate.get("plugin_id");
            let target = string(dependency, "target")?;
            let is_plugin = dependency["target_kind"] == "plugin";
            let capability: Option<String> = candidate.get("name");
            let version: Option<String> = candidate.get("capability_version");
            let cap_ready = candidate
                .get::<_, Option<bool>>("capability_ready")
                .unwrap_or(false);
            let plugin_ready = candidate
                .get::<_, Option<bool>>("plugin_ready")
                .unwrap_or(false)
                || cap_ready;
            if (is_plugin && (target != plugin || !plugin_ready))
                || (!is_plugin && (capability.as_deref() != Some(target) || !cap_ready))
            {
                continue;
            }
            let matched_version = if is_plugin {
                candidate.get::<_, String>("version")
            } else {
                version.clone().unwrap()
            };
            if !version_matches(string(dependency, "version_range")?, &matched_version)? {
                continue;
            }
            if exact.is_some() && !cap_ready {
                continue;
            }
            let contract = if is_plugin && exact.is_none() {
                Value::Null
            } else {
                json!({"name":capability.as_ref().unwrap(),"version":version.as_ref().unwrap(),"schema_digest":candidate.get::<_,Option<String>>("schema_digest").ok_or_else(||Error::conflict("Missing capability digest"))?})
            };
            if exact.is_some_and(|(requested, value, _)| requested == alias && *value != contract) {
                continue;
            }
            let source: Value = candidate.get("manifest");
            let features: Value = candidate
                .get::<_, Option<Value>>("features")
                .unwrap_or(json!([]));
            if !contract.is_null() {
                let Some(signed) = source["capabilities"]
                    .as_array()
                    .and_then(|caps| caps.iter().find(|cap| cap["contract"] == contract))
                else {
                    continue;
                };
                if Some(signed) != candidate.get::<_, Option<Value>>("spec").as_ref() {
                    continue;
                }
            }
            if dependency["features"]
                .as_array()
                .unwrap()
                .iter()
                .any(|feature| {
                    !features
                        .as_array()
                        .is_some_and(|items| items.contains(feature))
                })
            {
                continue;
            }
            let (action, resource) = if is_plugin {
                ("plugin.read", Resource::new("plugin", plugin.clone(), None))
            } else {
                (
                    "capability.discover",
                    Resource::new(
                        "capability",
                        format!(
                            "{}@{}",
                            capability.as_ref().unwrap(),
                            version.as_ref().unwrap()
                        ),
                        None,
                    ),
                )
            };
            if let Some(ctx) = ctx {
                if ceiling
                    .is_some_and(|rules| !auth::rules_allow(rules, action, &resource, ctx.peer))
                {
                    continue;
                }
                match auth::authorize(c, ctx, action, &resource).await {
                    Ok(()) => (),
                    Err(error)
                        if matches!(error.code, "PERMISSION_DENIED" | "INVALID_CREDENTIAL") =>
                    {
                        continue
                    }
                    Err(error) => return Err(error),
                }
            }
            chosen = Some(
                json!({"alias":alias,"contract":contract,"deployment_id":candidate.get::<_,Uuid>("id"),"instance_id":Value::Null,"binding_scope":dependency["binding_scope"],"required":dependency["required"],"failure_policy":dependency["failure_policy"],"provider_installation_id":candidate.get::<_,Uuid>("installation_id"),"provider_manifest_digest":candidate.get::<_,String>("manifest_digest"),"provider_plugin_id":plugin,"provider_spec":candidate.get::<_,Value>("provider_spec"),"provider_generation":candidate.get::<_,i64>("provider_generation"),"required_features":dependency["features"]}),
            );
            break;
        }
        if let Some(binding) = chosen {
            bindings.push(binding);
        } else if dependency["required"] == true {
            conflicts.push(format!("required_dependency_unavailable:{alias}"));
        }
    }
    if exact.is_none() {
        if let Err(error) = graph(c, consumer, &bindings).await {
            conflicts.push(error.message.to_owned());
        }
    }
    if conflicts.len() > 64 {
        conflicts.truncate(63);
        conflicts.push("additional_dependency_conflicts_omitted".to_owned());
    }
    Ok((revision, bindings, conflicts))
}

pub async fn plan(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    installation: Uuid,
) -> Result<Value> {
    dependency_lock(c).await?;
    let owner = c
        .query_opt(
            "SELECT owner_id FROM deployments WHERE id=$1 AND archived_at IS NULL",
            &[&deployment],
        )
        .await?
        .ok_or_else(Error::not_found)?
        .get::<_, Uuid>(0);
    auth::authorize(
        c,
        ctx,
        "plugin.dependencies.read",
        &Resource::new("deployment", deployment, Some(owner)),
    )
    .await?;
    let (revision, bindings, conflicts) = selected(c, ctx, deployment, installation).await?;
    let public: Vec<_> = bindings.into_iter().map(|binding| json!({"alias":binding["alias"],"contract":binding["contract"],"deployment_id":binding["deployment_id"],"instance_id":binding["instance_id"],"binding_scope":binding["binding_scope"]})).collect();
    // Selected providers already exist and own their reservation. Referencing
    // them does not create instances or duplicate their CPU/memory accounting.
    let result = json!({"deployment_id":deployment,"revision":revision,"bindings":public,"required_cpu_millis":0,"required_memory_bytes":0,"conflicts":conflicts});
    validate("DependencyPlan", &result)?;
    Ok(result)
}

pub async fn clear(c: &(impl GenericClient + Sync), deployment: Uuid) -> Result<()> {
    dependency_lock(c).await?;
    c.execute("DELETE FROM dependency_bindings WHERE consumer_deployment_id=$1 AND bound_call IS NOT NULL", &[&deployment]).await?;

    c.execute(
        "DELETE FROM dependency_bindings WHERE consumer_deployment_id=$1",
        &[&deployment],
    )
    .await?;
    Ok(())
}

/// Publication fence: invalidate routing and delegations before the controller
/// physically stops dependent consumers. Their reservations stay held until
/// the existing runtime stop proof, so no resources can be prematurely reused.
pub async fn invalidate_dependents(c: &(impl GenericClient + Sync), provider: Uuid) -> Result<()> {
    dependency_lock(c).await?;
    let rows = c.query("WITH RECURSIVE affected(id) AS (SELECT consumer_deployment_id FROM dependency_bindings WHERE provider_deployment_id=$1 AND required AND bound_call IS NULL UNION SELECT b.consumer_deployment_id FROM dependency_bindings b JOIN affected a ON b.provider_deployment_id=a.id WHERE b.required AND b.bound_call IS NULL) SELECT id FROM affected LIMIT 4097", &[&provider]).await?;
    if rows.len() > 4096 {
        return Err(Error::exhausted());
    }
    let ids: Vec<Uuid> = rows.iter().map(|r| r.get(0)).collect();
    // A request can have chosen a different provider than its deployment's
    // preview/base edge. Fence that exact issued parent Call, while permitting
    // future independent Calls to resolve another healthy request provider.
    let bound: Vec<Uuid> = c.query("SELECT DISTINCT bound_call FROM dependency_bindings WHERE provider_deployment_id=$1 AND required AND bound_call IS NOT NULL AND expires_at>now()", &[&provider]).await?.iter().map(|r|r.get(0)).collect();
    c.execute(
        "UPDATE rpc_delegations SET status='revoked' WHERE call_id=ANY($1)",
        &[&bound],
    )
    .await?;
    c.execute("UPDATE execution_delegations SET status='revoked' WHERE id IN(SELECT delegation_id FROM calls WHERE id=ANY($1))", &[&bound]).await?;
    c.execute("UPDATE calls SET state='unknown',error=$2 WHERE id=ANY($1) AND state IN ('accepted','running')", &[&bound,&Error::unavailable("Issued required request dependency became unavailable; physical outcome requires verification").detail()]).await?;
    c.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=ANY($1))", &[&ids]).await?;
    c.execute("UPDATE rpc_delegations SET status='revoked' WHERE call_id IN(SELECT c.id FROM calls c JOIN instances i ON i.id=c.provider_instance_id WHERE i.deployment_id=ANY($1) AND c.state IN ('accepted','running'))", &[&ids]).await?;
    c.execute("UPDATE execution_delegations SET status='revoked' WHERE id IN(SELECT c.delegation_id FROM calls c JOIN instances i ON i.id=c.provider_instance_id WHERE i.deployment_id=ANY($1) AND c.state IN ('accepted','running'))", &[&ids]).await?;
    c.execute("UPDATE calls SET state='unknown',error=$2 WHERE provider_instance_id IN(SELECT id FROM instances WHERE deployment_id=ANY($1)) AND state IN ('accepted','running')", &[&ids,&Error::unavailable("Required dependency became unavailable; physical outcome requires verification").detail()]).await?;
    c.execute("UPDATE dependency_bindings SET failure_latched=true WHERE consumer_deployment_id=ANY($1) AND required AND failure_policy='fail'", &[&ids]).await?;
    Ok(())
}

pub async fn bind(
    c: &(impl GenericClient + Sync),
    _app: &App,
    ctx: &Context,
    deployment: Uuid,
) -> Result<()> {
    dependency_lock(c).await?;
    let row = c
        .query_one(
            "SELECT installation_id,owner_id FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await?;
    auth::authorize(
        c,
        ctx,
        "plugin.enable",
        &Resource::new("deployment", deployment, Some(row.get("owner_id"))),
    )
    .await?;
    let installation: Uuid = row.get("installation_id");
    let (_, bindings, conflicts) = selected(c, ctx, deployment, installation).await?;
    if !conflicts.is_empty() {
        return Err(Error::conflict(
            "Dependency plan has conflicts; inspect resolveDependencies",
        ));
    }
    let paths = Policy::for_context(c, ctx)
        .credential(ctx.credential, 0)
        .await?;
    persist(c, ctx, deployment, installation, &bindings, &json!(paths)).await
}

async fn persist(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    installation: Uuid,
    bindings: &[Value],
    paths: &Value,
) -> Result<()> {
    clear(c, deployment).await?;
    for binding in bindings {
        insert_binding(c, ctx, deployment, installation, binding, paths).await?;
    }
    Ok(())
}

async fn insert_binding(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    installation: Uuid,
    binding: &Value,
    paths: &Value,
) -> Result<()> {
    c.execute("INSERT INTO dependency_bindings(id,consumer_deployment_id,alias,provider_deployment_id,contract,binding_scope,consumer_installation_id,provider_installation_id,provider_manifest_digest,required,failure_policy,issuer_credential_id,subject_id,peer,browser_origin,source_paths,provider_spec,provider_generation,required_features,provider_plugin_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14::text::inet,$15, $16,$17,$18,$19,$20)", &[&Uuid::new_v4(),&deployment,&string(binding,"alias")?,&id(binding,"deployment_id")?,&binding["contract"],&string(binding,"binding_scope")?,&installation,&id(binding,"provider_installation_id")?,&string(binding,"provider_manifest_digest")?,&binding["required"].as_bool().unwrap(),&string(binding,"failure_policy")?,&ctx.credential,&ctx.subject,&ctx.peer.to_string(),&ctx.browser_origin,paths,&binding["provider_spec"],&binding["provider_generation"].as_i64(),&binding["required_features"],&string(binding,"provider_plugin_id")?]).await?;
    Ok(())
}

pub async fn check_ready(
    c: &(impl GenericClient + Sync),
    _app: &App,
    deployment: Uuid,
) -> Result<()> {
    let mut queue = vec![(deployment, 0)];
    let mut visited = BTreeSet::new();
    while let Some((current, depth)) = queue.pop() {
        if depth >= 16 || visited.len() >= 4096 {
            return Err(Error::conflict("Dependency graph exceeds its bound"));
        }
        if !visited.insert(current) {
            continue;
        }
        let row=c.query_opt("SELECT n.manifest,d.installation_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1", &[&current]).await?.ok_or_else(Error::not_found)?;
        let manifest: Value = row.get("manifest");
        let installation: Uuid = row.get("installation_id");
        for dependency in manifest["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| d["required"] == true)
        {
            let alias = string(dependency, "name")?;
            if dependency["binding_scope"] == "request" {
                let (_, candidates, conflicts) =
                    selected_filtered(c, None, current, installation, None, None).await?;
                if !conflicts.is_empty() {
                    return Err(Error::unavailable(
                        "Required request dependency cannot resolve",
                    ));
                }
                let candidate = candidates
                    .iter()
                    .find(|candidate| candidate["alias"] == alias)
                    .ok_or_else(|| {
                        Error::unavailable("Required request dependency is unavailable")
                    })?;
                candidate_ready(c, candidate).await?;
                queue.push((id(candidate, "deployment_id")?, depth + 1));
                continue;
            }
            let binding=c.query_opt("SELECT b.*,host(b.peer) AS approved_peer FROM dependency_bindings b WHERE consumer_deployment_id=$1 AND alias=$2 AND bound_call IS NULL", &[&current,&alias]).await?.ok_or_else(||Error::unavailable("Required dependency has no fixed binding"))?;
            if binding.get::<_, Option<Uuid>>("consumer_installation_id") != Some(installation) {
                return Err(Error::conflict(
                    "Consumer installation changed; explicit dependency rebind required",
                ));
            }
            let checked = async { provider_ready(c, &binding).await }.await;
            if let Err(error) = checked {
                if binding.get::<_, String>("failure_policy") == "fail" {
                    c.execute("UPDATE dependency_bindings SET failure_latched=true WHERE id=$1 OR parent_binding_id=$1", &[&binding.get::<_,Uuid>("id")]).await?;
                }
                return Err(error);
            }
            queue.push((binding.get::<_, Uuid>("provider_deployment_id"), depth + 1));
        }
    }
    Ok(())
}

pub async fn optional_conditions(
    c: &(impl GenericClient + Sync),
    deployment: Uuid,
) -> Result<Vec<String>> {
    let row=c.query_one("SELECT n.manifest,d.installation_id FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1", &[&deployment]).await?;
    let manifest: Value = row.get("manifest");
    let installation: Uuid = row.get("installation_id");
    let optional: Vec<&Value> = manifest["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["required"] == false)
        .collect();
    let mut reasons = Vec::new();
    for dependency in optional {
        let alias = string(dependency, "name")?;
        let available=if dependency["binding_scope"]=="request"{
            selected_filtered(c,None,deployment,installation,None,None).await?.1.iter().any(|binding|binding["alias"]==alias)
        }else if let Some(binding)=c.query_opt("SELECT b.* FROM dependency_bindings b WHERE consumer_deployment_id=$1 AND alias=$2 AND bound_call IS NULL", &[&deployment,&alias]).await?{provider_ready(c,&binding).await.is_ok()}else{false};
        if !available {
            reasons.push(format!("dependency_degraded:{alias}"));
        }
    }
    Ok(reasons)
}

/// A parent Call retains the provider it actually resolved. A healthy base
/// preview or a new candidate cannot authorize completion of this older Call.
pub async fn check_call(c: &(impl GenericClient + Sync), call: Uuid) -> Result<()> {
    for binding in c.query("SELECT b.* FROM dependency_bindings b WHERE bound_call=$1 AND required AND expires_at>now()", &[&call]).await? {
        provider_ready(c, &binding).await?;
    }
    Ok(())
}

async fn provider_ready(
    c: &(impl GenericClient + Sync),
    binding: &tokio_postgres::Row,
) -> Result<()> {
    let value = json!({"deployment_id":binding.get::<_,Uuid>("provider_deployment_id"),"provider_installation_id":binding.get::<_,Option<Uuid>>("provider_installation_id"),"provider_manifest_digest":binding.get::<_,Option<String>>("provider_manifest_digest"),"provider_spec":binding.get::<_,Option<Value>>("provider_spec"),"provider_generation":binding.get::<_,Option<i64>>("provider_generation"),"required_features":binding.get::<_,Value>("required_features"),"contract":binding.get::<_,Value>("contract"),"failure_latched":binding.get::<_,bool>("failure_latched")});
    candidate_ready(c, &value).await
}

async fn candidate_ready(c: &(impl GenericClient + Sync), binding: &Value) -> Result<()> {
    let contract = &binding["contract"];
    if binding["failure_latched"].as_bool().unwrap_or(false) {
        return Err(Error::conflict(
            "Dependency failure is latched; explicit rebind required",
        ));
    }
    let live = if contract.is_null() {
        c.query_one("SELECT EXISTS(SELECT 1 FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers u ON u.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id WHERE d.id=$1 AND d.installation_id=$2 AND n.manifest_digest=$3 AND n.state='installed' AND u.status='trusted' AND d.archived_at IS NULL AND d.spec->>'enabled'='true' AND d.spec=$4 AND i.generation=$5 AND i.phase='ready' AND (i.last_heartbeat_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint) OR EXISTS(SELECT 1 FROM provider_records p WHERE p.instance_id=i.id AND p.availability='ready' AND p.observed_generation=i.generation AND p.observed_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint))) AND ($6='[]'::jsonb OR EXISTS(SELECT 1 FROM provider_records p WHERE p.instance_id=i.id AND p.availability='ready' AND p.observed_generation=i.generation AND p.features @> $6)))", &[&id(binding,"deployment_id")?,&Some(id(binding,"provider_installation_id")?),&Some(string(binding,"provider_manifest_digest")?),&Some(binding["provider_spec"].clone()),&binding["provider_generation"].as_i64(),&binding["required_features"].clone()]).await?.get::<_,bool>(0)
    } else {
        c.query_one("SELECT EXISTS(SELECT 1 FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers u ON u.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id JOIN provider_records p ON p.instance_id=i.id JOIN capability_contracts k ON k.name=p.name AND k.version=p.version WHERE d.id=$1 AND d.installation_id=$2 AND n.manifest_digest=$3 AND n.state='installed' AND u.status='trusted' AND d.archived_at IS NULL AND d.spec->>'enabled'='true' AND i.phase='ready' AND p.name=$4 AND p.version=$5 AND k.schema_digest=$6 AND d.spec=$7 AND i.generation=$8 AND p.features @> $9 AND NOT $10 AND p.availability='ready' AND p.observed_generation=i.generation AND p.observed_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint))", &[&id(binding,"deployment_id")?,&Some(id(binding,"provider_installation_id")?),&Some(string(binding,"provider_manifest_digest")?),&string(contract,"name")?,&string(contract,"version")?,&string(contract,"schema_digest")?,&Some(binding["provider_spec"].clone()),&binding["provider_generation"].as_i64(),&binding["required_features"].clone(),&binding["failure_latched"].as_bool().unwrap_or(false)]).await?.get::<_,bool>(0)
    };
    if !live {
        return Err(Error::unavailable(
            "Fixed dependency provider is unavailable; no automatic rebind",
        ));
    }
    Ok(())
}

pub async fn resolve_host(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    consumer: Uuid,
    call: Uuid,
    scope: &str,
    contract: &Value,
) -> Result<Value> {
    validate("ContractRef", contract)?;
    dependency_lock(c).await?;
    let parent=c.query_opt("SELECT deadline_at FROM calls q JOIN instances i ON i.id=q.provider_instance_id JOIN deployments d ON d.id=i.deployment_id WHERE q.id=$1 AND i.deployment_id=$2 AND q.owner_id=$3 AND q.credential_id=$4 AND q.state='running' AND q.deadline_at>now() AND d.spec->>'enabled'='true' AND d.archived_at IS NULL", &[&call,&consumer,&ctx.subject,&ctx.credential]).await?.ok_or_else(Error::denied)?;
    if scope != call.to_string() && scope != consumer.to_string() {
        return Err(Error::denied());
    }
    let installation: Uuid = c
        .query_one(
            "SELECT installation_id FROM deployments WHERE id=$1 AND archived_at IS NULL",
            &[&consumer],
        )
        .await?
        .get(0);
    let paths = json!(
        Policy::for_context(c, ctx)
            .credential(ctx.credential, 0)
            .await?
    );
    let resource = Resource::new(
        "capability",
        format!(
            "{}@{}",
            string(contract, "name")?,
            string(contract, "version")?
        ),
        None,
    );
    auth::authorize(c, ctx, "capability.discover", &resource).await?;
    let deadline: chrono::DateTime<chrono::Utc> = parent.get("deadline_at");
    if let Some(existing)=c.query_opt("SELECT b.*,host(b.peer) AS approved_peer FROM dependency_bindings b WHERE consumer_deployment_id=$1 AND bound_call=$2 AND contract=$3 AND expires_at>now() ORDER BY alias LIMIT 1", &[&consumer,&call,&contract]).await?{
        if existing.get::<_,String>("binding_scope")=="request" && scope!=call.to_string(){return Err(Error::denied());}
        provider_ready(c,&existing).await?;
        return Ok(json!({"binding_id":existing.get::<_,Uuid>("id"),"provider_deployment_id":existing.get::<_,Uuid>("provider_deployment_id"),"contract":contract,"expires_at":deadline,"expires_unix_ms":deadline.timestamp_millis()}));
    }
    let manifest: Value = c
        .query_one(
            "SELECT manifest FROM installations WHERE id=$1",
            &[&installation],
        )
        .await?
        .get(0);
    let mut chosen = None;
    let mut task_scope = false;
    for dependency in manifest["dependencies"].as_array().into_iter().flatten() {
        let alias = string(dependency, "name")?;
        if dependency["target_kind"] == "capability"
            && (dependency["target"] != contract["name"]
                || !version_matches(
                    string(dependency, "version_range")?,
                    string(contract, "version")?,
                )?)
        {
            continue;
        }
        if dependency["binding_scope"] == "task" {
            task_scope = true;
            continue;
        }
        if dependency["binding_scope"] == "request" && scope != call.to_string() {
            continue;
        }
        let existing=c.query_opt("SELECT b.* FROM dependency_bindings b WHERE consumer_deployment_id=$1 AND alias=$2 AND bound_call IS NULL", &[&consumer,&alias]).await?;
        let fixed = if dependency["binding_scope"] == "deployment" {
            existing
                .as_ref()
                .map(|base| base.get::<_, Uuid>("provider_deployment_id"))
        } else {
            None
        };
        if dependency["binding_scope"] == "deployment" && fixed.is_none() {
            continue;
        }
        let (_, candidates, _) = selected_filtered(
            c,
            Some(ctx),
            consumer,
            installation,
            Some((alias, contract, fixed)),
            None,
        )
        .await?;
        let Some(candidate) = candidates.into_iter().next() else {
            continue;
        };
        if dependency["binding_scope"] == "deployment" {
            let Some(base) = existing else {
                continue;
            };
            provider_ready(c, &base).await?;
            if base.get::<_, Uuid>("provider_deployment_id") != id(&candidate, "deployment_id")? {
                continue;
            }
            chosen = Some((candidate, base.get::<_, Uuid>("id")));
            break;
        }
        let base = if let Some(base) = existing {
            base.get::<_, Uuid>("id")
        } else {
            insert_binding(c, ctx, consumer, installation, &candidate, &paths).await?;
            c.query_one("SELECT id FROM dependency_bindings WHERE consumer_deployment_id=$1 AND alias=$2 AND bound_call IS NULL", &[&consumer,&alias]).await?.get(0)
        };
        chosen = Some((candidate, base));
        break;
    }
    let (candidate, base) = chosen.ok_or_else(|| {
        if task_scope {
            Error::conflict("Task-scoped resolution requires the TaskEngine")
        } else {
            Error::unavailable("No authorized declared dependency provider is available")
        }
    })?;
    candidate_ready(c, &candidate).await?;
    let frozen=c.query("SELECT alias,required,provider_deployment_id FROM dependency_bindings WHERE consumer_deployment_id=$1 AND bound_call IS NULL", &[&consumer]).await?;
    let mut proposed:Vec<Value>=frozen.into_iter().filter(|item|item.get::<_,String>("alias")!=string(&candidate,"alias").unwrap()).map(|item|json!({"required":item.get::<_,bool>("required"),"deployment_id":item.get::<_,Uuid>("provider_deployment_id")})).collect();
    proposed.push(candidate.clone());
    graph(c, consumer, &proposed).await?;
    let binding = Uuid::new_v4();
    let provider = id(&candidate, "deployment_id")?;
    c.execute("INSERT INTO dependency_bindings(id,consumer_deployment_id,alias,provider_deployment_id,contract,binding_scope,consumer_installation_id,provider_installation_id,provider_manifest_digest,required,failure_policy,issuer_credential_id,subject_id,peer,browser_origin,source_paths,provider_spec,provider_generation,required_features,provider_plugin_id,bound_call,parent_binding_id,expires_at) SELECT $1,consumer_deployment_id,alias,$5,$6,binding_scope,consumer_installation_id,$7,$8,required,failure_policy,issuer_credential_id,subject_id,peer,browser_origin,source_paths,$9,$10,required_features,$11,$2,id,$3 FROM dependency_bindings WHERE id=$4", &[&binding,&call,&deadline,&base,&provider,&contract,&id(&candidate,"provider_installation_id")?,&string(&candidate,"provider_manifest_digest")?,&candidate["provider_spec"],&candidate["provider_generation"].as_i64(),&string(&candidate,"provider_plugin_id")?]).await?;
    c.execute("UPDATE dependency_bindings SET issuer_credential_id=$2,subject_id=$3,peer=$4::text::inet,browser_origin=$5,source_paths=$6 WHERE id=$1", &[&binding,&ctx.credential,&ctx.subject,&ctx.peer.to_string(),&ctx.browser_origin,&paths]).await?;
    Ok(
        json!({"binding_id":binding,"provider_deployment_id":provider,"contract":contract,"expires_at":deadline,"expires_unix_ms":deadline.timestamp_millis()}),
    )
}

pub async fn check_host(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    consumer: Uuid,
    call: Uuid,
    binding: Uuid,
    contract: &Value,
) -> Result<Uuid> {
    let row = c.query_opt("SELECT b.*,host(b.peer) AS approved_peer FROM dependency_bindings b JOIN dependency_bindings p ON p.id=b.parent_binding_id JOIN deployments d ON d.id=b.consumer_deployment_id JOIN calls q ON q.id=b.bound_call JOIN instances i ON i.id=q.provider_instance_id WHERE b.id=$1 AND b.consumer_deployment_id=$2 AND b.bound_call=$3 AND b.contract=$4 AND b.expires_at>now() AND b.consumer_installation_id=d.installation_id AND q.owner_id=$5 AND q.credential_id=$6 AND q.state='running' AND q.deadline_at>now() AND i.deployment_id=d.id AND d.spec->>'enabled'='true'", &[&binding,&consumer,&call,&contract,&ctx.subject,&ctx.credential]).await?.ok_or_else(Error::denied)?;
    auth::authorize(
        c,
        ctx,
        "capability.invoke",
        &Resource::new(
            "capability",
            format!(
                "{}@{}",
                string(contract, "name")?,
                string(contract, "version")?
            ),
            None,
        ),
    )
    .await?;
    provider_ready(c, &row).await?;
    Ok(row.get("provider_deployment_id"))
}

pub async fn stage_upgrade(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    consumer: Uuid,
    target: Uuid,
    operation: Uuid,
) -> Result<()> {
    dependency_lock(c).await?;
    let (_, bindings, conflicts) = selected(c, ctx, consumer, target).await?;
    if !conflicts.is_empty() {
        return Err(Error::conflict("Upgrade dependency plan has conflicts"));
    }
    let paths = Policy::for_context(c, ctx)
        .credential(ctx.credential, 0)
        .await?;
    c.execute("INSERT INTO dependency_upgrade_plans(operation_id,consumer_deployment_id,target_installation_id,bindings,source_paths) VALUES($1,$2,$3,$4,$5)", &[&operation,&consumer,&target,&json!(bindings),&json!(paths)]).await?;
    invalidate_dependents(c, consumer).await?;
    Ok(())
}

pub async fn check_upgrade(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    operation: Uuid,
) -> Result<()> {
    let Some(row) = c
        .query_opt(
            "SELECT * FROM dependency_upgrade_plans WHERE operation_id=$1",
            &[&operation],
        )
        .await?
    else {
        return Ok(());
    };
    let saved: Vec<policy::Path> =
        serde_json::from_value(row.get("source_paths")).map_err(|_| Error::denied())?;
    let live = policy::rules(
        &Policy::for_context(c, ctx)
            .revalidate(&saved, ctx.credential)
            .await?,
    );
    let bindings: Value = row.get("bindings");
    for binding in bindings.as_array().ok_or_else(Error::denied)? {
        let (action, resource) = if binding["contract"].is_null() {
            (
                "plugin.read",
                Resource::new("plugin", string(binding, "provider_plugin_id")?, None),
            )
        } else {
            (
                "capability.discover",
                Resource::new(
                    "capability",
                    format!(
                        "{}@{}",
                        string(&binding["contract"], "name")?,
                        string(&binding["contract"], "version")?
                    ),
                    None,
                ),
            )
        };
        if !auth::rules_allow(&live, action, &resource, ctx.peer) {
            return Err(Error::denied());
        }
        if binding["required"] == true {
            candidate_ready(c, binding).await?;
            check_ready(c, app, id(binding, "deployment_id")?).await?;
        }
    }
    graph(
        c,
        row.get("consumer_deployment_id"),
        bindings.as_array().unwrap(),
    )
    .await
}

pub async fn publish_upgrade(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    consumer: Uuid,
    operation: Uuid,
) -> Result<()> {
    dependency_lock(c).await?;
    check_upgrade(c, app, ctx, operation).await?;
    let Some(row) = c
        .query_opt(
            "SELECT * FROM dependency_upgrade_plans WHERE operation_id=$1",
            &[&operation],
        )
        .await?
    else {
        clear(c, consumer).await?;
        return Ok(());
    };
    let bindings: Value = row.get("bindings");
    persist(
        c,
        ctx,
        consumer,
        row.get("target_installation_id"),
        bindings.as_array().ok_or_else(Error::denied)?,
        &row.get::<_, Value>("source_paths"),
    )
    .await
}

#[cfg(test)]
#[path = "dependencies_tests.rs"]
mod tests;
