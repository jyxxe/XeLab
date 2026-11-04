//! Signed UI discovery and account-owned preferences. Discovery only narrows
//! authority; the Broker independently authorizes every subsequent Skill call.
use crate::{
    api::{etag, path_id, App},
    auth::{self, Context, Resource},
    contracts::{id, strict_urlencoded, string, unsigned, validate},
    db,
    error::{Error, Result},
    plugin_http::{self, transport::StaticOrigin},
    policy::{self, Policy},
};
use axum::http::{HeaderMap, Method};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use tokio_postgres::{GenericClient, Row};
use uuid::Uuid;

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "listUiContributions" | "getUiContribution" | "getMyUiPreferences" | "setMyUiPreferences"
    )
}

/// Shared by signed artifact admission and fresh discovery. No entry URL or
/// Skill target can be supplied by a running plugin or its iframe.
pub(crate) fn validate_manifest(manifest: &Value) -> Result<()> {
    let Some(ui) = manifest.get("ui") else {
        return Ok(());
    };
    validate("PluginUiSpec", ui)?;
    let plugin = string(manifest, "plugin_id")?;
    if ui["base_path"] != format!("/ui/{plugin}/") {
        return Err(Error::invalid("UI base path differs from its plugin"));
    }
    let mut identifiers = BTreeSet::new();
    for spec in ui["contributions"].as_array().unwrap() {
        if !identifiers.insert(string(spec, "id")?) {
            return Err(Error::invalid("UI contribution identifiers must be unique"));
        }
        crate::skills::definition_digest(spec)?;
        for field in ["entry_path", "icon_path"] {
            if let Some(path) = spec.get(field) {
                let path = path
                    .as_str()
                    .ok_or_else(|| Error::invalid("Invalid UI path"))?;
                plugin_http::canonical_path(path)?;
                let fixed = format!("/ui/{plugin}{path}");
                if !manifest["routes"].as_array().is_some_and(|routes| {
                    routes.iter().any(|route| {
                        matches!(route["kind"].as_str(), Some("static" | "webui"))
                            && route["authentication"] == "public"
                            && ["GET", "HEAD"].iter().all(|method| {
                                route["methods"].as_array().is_some_and(|methods| {
                                    methods.iter().any(|value| value == method)
                                })
                            })
                            && route["path_prefix"].as_str().is_some_and(|prefix| {
                                fixed == prefix
                                    || fixed.strip_prefix(prefix).is_some_and(|tail| {
                                        prefix.ends_with('/') || tail.starts_with('/')
                                    })
                            })
                    })
                }) {
                    return Err(Error::invalid(
                        "UI entry requires a signed public GET/HEAD route",
                    ));
                }
            }
        }
        let dimensions = if spec["kind"] == "window" {
            (&spec["window"], ["width", "height"])
        } else {
            (&spec["widget"], ["columns", "rows"])
        };
        for field in dimensions.1 {
            let minimum = unsigned(&dimensions.0[format!("min_{field}")])?;
            let value = unsigned(&dimensions.0[field])?;
            let maximum = unsigned(&dimensions.0[format!("max_{field}")])?;
            if !(minimum..=maximum).contains(&value) {
                return Err(Error::invalid("UI dimensions must be ordered"));
            }
        }
        let mut aliases = BTreeSet::new();
        for binding in spec["skill_bindings"].as_array().unwrap() {
            if !aliases.insert(string(binding, "alias")?) {
                return Err(Error::invalid("UI Skill aliases must be unique"));
            }
            if !manifest["skills"].as_array().is_some_and(|skills| {
                skills.iter().any(|skill| {
                    skill["name"] == binding["name"]
                        && skill["version"] == binding["version"]
                        && skill["target"]["plugin_id"] == plugin
                })
            }) {
                return Err(Error::invalid(
                    "UI binding must name an exact signed local Skill",
                ));
            }
        }
    }
    Ok(())
}

async fn deployment(c: &(impl GenericClient + Sync), deployment: Uuid) -> Result<Row> {
    c.query_opt("SELECT d.id,d.owner_id,d.installation_id,d.spec AS deployment_spec,d.phase,d.observed_generation,d.approval,n.plugin_id,n.manifest_digest,n.manifest,n.state AS installation_state,p.status AS publisher_status FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id WHERE d.id=$1 AND d.archived_at IS NULL", &[&deployment]).await?.ok_or_else(Error::not_found)
}

/// The resource owner comes from this fixed deployment, not a plugin-supplied
/// field or a global owner guess for a stable plugin identifier.
async fn plugin_allowed(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    plugin: &str,
    owner: Uuid,
) -> Result<bool> {
    if !policy::registered(c, action, "plugin").await? {
        return Ok(false);
    }
    let resource = Resource::new("plugin", plugin, Some(owner));
    let paths = Policy::for_context(c, ctx)
        .with_resource_owner(&resource)
        .credential(ctx.credential, 0)
        .await?;
    Ok(auth::rules_allow(
        &policy::rules(&paths),
        action,
        &resource,
        ctx.peer,
    ))
}

async fn visible(c: &(impl GenericClient + Sync), ctx: &Context, row: &Row) -> Result<()> {
    if plugin_allowed(
        c,
        ctx,
        "plugin.ui.open",
        &row.get::<_, String>("plugin_id"),
        row.get("owner_id"),
    )
    .await?
    {
        Ok(())
    } else {
        Err(Error::not_found())
    }
}

fn contribution_spec(manifest: &Value, contribution: &str) -> Result<Value> {
    validate_manifest(manifest)?;
    manifest["ui"]["contributions"]
        .as_array()
        .and_then(|items| items.iter().find(|spec| spec["id"] == contribution))
        .cloned()
        .ok_or_else(Error::not_found)
}

pub async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    _body: &Value,
) -> Result<()> {
    match name {
        "listUiContributions" => Ok(()), // Each fixed deployment is filtered afresh.
        "getUiContribution" => {
            let row = deployment(c, path_id(params, "deployment_id")?).await?;
            visible(c, ctx, &row).await?;
            contribution_spec(
                &row.get("manifest"),
                params.get("contribution_id").ok_or_else(Error::not_found)?,
            )?;
            Ok(())
        }
        "getMyUiPreferences" | "setMyUiPreferences" => {
            auth::authorize(
                c,
                ctx,
                if name == "getMyUiPreferences" {
                    "ui.preferences.read"
                } else {
                    "ui.preferences.update"
                },
                &Resource::new("account", ctx.subject, Some(ctx.subject)),
            )
            .await
        }
        _ => Err(Error::not_found()),
    }
}

fn default_preferences() -> Value {
    json!({"theme":{"mode":"system","reduce_motion":false,"reduce_transparency":false},"dock":{"position":"bottom","auto_hide":false,"pinned":[]},"widgets":[],"restore_windows":true})
}

fn preference_value(row: &Row) -> Result<Value> {
    let mut value = db::stamp(row);
    value["owner_id"] = json!(row.get::<_, Uuid>("owner_id"));
    value["spec"] = row.get("spec");
    validate("UiPreferences", &value)?;
    Ok(value)
}

async fn preferences(c: &(impl GenericClient + Sync), ctx: &Context) -> Result<Row> {
    // This is an authenticated account resource, never schema seed data. Empty
    // migrated recovery targets must remain empty until their import commits.
    c.execute("INSERT INTO ui_preferences(id,owner_id,spec,created_at,updated_at) VALUES($1,$2,$3,now(),now()) ON CONFLICT(owner_id) DO NOTHING", &[&Uuid::new_v4(),&ctx.subject,&default_preferences()]).await?;
    Ok(c.query_one(
        "SELECT * FROM ui_preferences WHERE owner_id=$1 FOR UPDATE",
        &[&ctx.subject],
    )
    .await?)
}

async fn layout_spec(c: &(impl GenericClient + Sync), ctx: &Context, key: &Value) -> Result<Value> {
    let row = deployment(c, id(key, "deployment_id")?).await?;
    visible(c, ctx, &row).await?;
    contribution_spec(&row.get("manifest"), string(key, "contribution_id")?)
}

fn validate_preference_structure(spec: &Value) -> Result<()> {
    validate("UiPreferencesSpec", spec)?;
    let mut pinned = BTreeSet::new();
    for key in spec["dock"]["pinned"].as_array().unwrap() {
        if !pinned.insert((id(key, "deployment_id")?, string(key, "contribution_id")?)) {
            return Err(Error::invalid("A Dock contribution cannot be pinned twice"));
        }
    }
    let mut instances = BTreeSet::new();
    for placement in spec["widgets"].as_array().unwrap() {
        if !instances.insert(id(placement, "instance_id")?) {
            return Err(Error::invalid("Widget instance identifiers must be unique"));
        }
    }
    Ok(())
}

/// Validate data before offline export/import without reviving historical
/// authority. An unavailable, removed or resized contribution remains an inert
/// layout reference; no current publisher, Client, grant or instance is trusted.
pub(crate) async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    let mut after: Option<Uuid> = None;
    loop {
        let rows = c.query("SELECT * FROM ui_preferences WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200", &[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            preference_value(&row)?;
            validate_preference_structure(&row.get("spec"))?;
        }
    }
    after = None;
    loop {
        let rows = c.query("SELECT id,manifest FROM installations WHERE manifest ? 'ui' AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200", &[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            validate_manifest(&row.get("manifest"))?;
        }
    }
    Ok(())
}

async fn validate_preferences(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    previous: &Value,
    wanted: &Value,
) -> Result<()> {
    validate_preference_structure(wanted)?;
    for key in wanted["dock"]["pinned"].as_array().unwrap() {
        let retained = previous["dock"]["pinned"]
            .as_array()
            .is_some_and(|old| old.contains(key));
        match layout_spec(c, ctx, key).await {
            Ok(spec) if spec["kind"] == "window" => (),
            Ok(_) if retained => (),
            Err(error) if error.code == "RESOURCE_NOT_FOUND" && retained => (),
            Ok(_) => return Err(Error::invalid("Only window contributions can be pinned")),
            Err(error) => return Err(error),
        }
    }
    for placement in wanted["widgets"].as_array().unwrap() {
        let retained = previous["widgets"]
            .as_array()
            .is_some_and(|old| old.contains(placement));
        let spec = match layout_spec(c, ctx, &placement["contribution"]).await {
            Ok(spec) => spec,
            Err(error) if error.code == "RESOURCE_NOT_FOUND" && retained => continue,
            Err(error) => return Err(error),
        };
        if spec["kind"] != "widget" {
            if retained {
                continue;
            }
            return Err(Error::invalid(
                "Widget placements require a widget contribution",
            ));
        }
        let mut fits = true;
        for field in ["columns", "rows"] {
            let minimum = unsigned(&spec["widget"][format!("min_{field}")])?;
            let maximum = unsigned(&spec["widget"][format!("max_{field}")])?;
            fits &= (minimum..=maximum).contains(&unsigned(&placement[field])?);
        }
        if !fits && !retained {
            return Err(Error::invalid(
                "Widget size exceeds its signed contribution limits",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    if name != "listUiContributions" && !query.is_empty() {
        return Err(Error::invalid("Unexpected UI query parameter"));
    }
    authorize(c, ctx, name, params, body).await?;
    match name {
        "listUiContributions" => Ok((list(c, app, ctx, query).await?, 200)),
        "getUiContribution" => {
            let row = deployment(c, path_id(params, "deployment_id")?).await?;
            let spec = contribution_spec(
                &row.get("manifest"),
                params.get("contribution_id").ok_or_else(Error::not_found)?,
            )?;
            Ok((contribution(c, app, ctx, &row, &spec).await?, 200))
        }
        "getMyUiPreferences" => Ok((preference_value(&preferences(c, ctx).await?)?, 200)),
        "setMyUiPreferences" => {
            let row = preferences(c, ctx).await?;
            etag(headers, row.get("revision"))?;
            validate_preferences(c, ctx, &row.get("spec"), body).await?;
            let row = c.query_one("UPDATE ui_preferences SET spec=$2,revision=revision+1,updated_at=now() WHERE owner_id=$1 RETURNING *", &[&ctx.subject,&body]).await?;
            Ok((preference_value(&row)?, 200))
        }
        _ => Err(Error::not_found()),
    }
}

fn unavailable(error: &Error) -> bool {
    matches!(
        error.code,
        "PERMISSION_DENIED"
            | "RESOURCE_NOT_FOUND"
            | "INVALID_CREDENTIAL"
            | "STATE_CONFLICT"
            | "CONTRACT_CONFLICT"
            | "INVALID_ARGUMENT"
    )
}

async fn contribution(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    row: &Row,
    spec: &Value,
) -> Result<Value> {
    let deployment: Uuid = row.get("id");
    let installation: Uuid = row.get("installation_id");
    let plugin: String = row.get("plugin_id");
    let manifest: Value = row.get("manifest");
    let mut reasons = BTreeSet::new();
    if row.get::<_, String>("installation_state") != "installed"
        || row.get::<_, String>("publisher_status") != "trusted"
    {
        reasons.insert("INSTALLATION_UNAVAILABLE");
    }
    if row.get::<_, Value>("deployment_spec")["enabled"] != true
        || row.get::<_, String>("phase") != "ready"
    {
        reasons.insert("DEPLOYMENT_UNAVAILABLE");
    }
    let origin = match StaticOrigin::for_deployment(&app.config, deployment, None) {
        Ok(origin) if ctx.browser_origin.as_deref() != Some(origin.origin()) => Some(origin),
        Ok(_) => {
            reasons.insert("ISOLATED_ORIGIN_UNAVAILABLE");
            None
        }
        Err(error) if error.code == "NOT_READY" || unavailable(&error) => {
            reasons.insert("ISOLATED_ORIGIN_UNAVAILABLE");
            None
        }
        Err(error) => return Err(error),
    };
    let registry = crate::http_contracts::load(c, installation).await?;
    for field in ["entry_path", "icon_path"] {
        if let Some(path) = spec.get(field).and_then(Value::as_str) {
            let path = format!("/ui/{plugin}{path}");
            for method in [Method::GET, Method::HEAD] {
                match registry.resolve_static(&method, &path) {
                    Ok(route) if route.public => (),
                    Ok(_) => {
                        reasons.insert("STATIC_ROUTE_UNAVAILABLE");
                    }
                    Err(error) if unavailable(&error) => {
                        reasons.insert("STATIC_ROUTE_UNAVAILABLE");
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    match crate::plugin_http_gateway::check_approval(c, app, deployment, &manifest).await {
        Ok(()) => (),
        Err(error) if unavailable(&error) => {
            reasons.insert("APPROVAL_UNAVAILABLE");
        }
        Err(error) => return Err(error),
    }
    let healthy:i64=c.query_one("SELECT count(*) FROM instances i WHERE i.deployment_id=$1 AND i.phase='ready' AND i.health='alive' AND i.generation=$2 AND i.last_heartbeat_at>now()-interval '1 millisecond'*($3::bigint*$4::bigint+$5::bigint)", &[&deployment,&row.get::<_,i64>("observed_generation"),&(unsigned(&manifest["health"]["interval_ms"])? as i64),&(unsigned(&manifest["health"]["failure_threshold"])? as i64),&(unsigned(&manifest["health"]["timeout_ms"])? as i64)]).await?.get(0);
    if healthy != 1 {
        reasons.insert("INSTANCE_UNAVAILABLE");
    }
    let mut bindings = Vec::new();
    for binding in spec["skill_bindings"].as_array().unwrap() {
        let saved=c.query_opt("SELECT id,enabled,archived_at FROM skills WHERE name=$1 AND version=$2 AND installation_id=$3", &[&string(binding,"name")?,&string(binding,"version")?,&installation]).await?;
        let Some(saved) = saved else {
            reasons.insert("SKILL_BINDING_UNAVAILABLE");
            continue;
        };
        if !saved.get::<_, bool>("enabled")
            || saved
                .get::<_, Option<DateTime<Utc>>>("archived_at")
                .is_some()
        {
            reasons.insert("SKILL_BINDING_UNAVAILABLE");
            continue;
        }
        let resolved = match crate::skills::resolve_for_invoke(c, ctx, saved.get("id")).await {
            Ok(resolved) => resolved,
            Err(error) if error.code == "PERMISSION_DENIED" => continue,
            Err(error)
                if matches!(
                    error.code,
                    "NOT_READY" | "CONTRACT_CONFLICT" | "RESOURCE_NOT_FOUND"
                ) =>
            {
                reasons.insert("SKILL_BINDING_UNAVAILABLE");
                continue;
            }
            Err(error) => return Err(error),
        };
        if resolved.installation_id != installation
            || resolved.definition["name"] != binding["name"]
            || resolved.definition["version"] != binding["version"]
        {
            return Err(Error::conflict(
                "UI Skill binding differs from its signed installation",
            ));
        }
        let allowed = if resolved.definition["target"]["kind"] == "http" {
            let operation = registry.bind_skill(&resolved.definition)?;
            plugin_allowed(
                c,
                ctx,
                &operation.route.action,
                &plugin,
                row.get("owner_id"),
            )
            .await?
        } else {
            let contract = &resolved.definition["target"]["contract"];
            policy::allows(
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
            .await?
        };
        // BatchEngine is a later milestone. Preserve its signed declaration,
        // while the executable M3 allowlist advertises only implemented call.
        if allowed
            && binding["modes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|mode| mode == "call")
        {
            bindings.push(json!({"alias":binding["alias"],"skill_id":resolved.id,"definition_digest":resolved.definition_digest,"modes":["call"]}));
        }
    }
    let frame_origin = origin.as_ref().map(StaticOrigin::origin);
    let framing_query = if origin.is_some() && reasons.is_empty() {
        match crate::webui::framing_query(c, app, ctx, deployment, spec).await {
            Ok(query) => query,
            Err(error) if unavailable(&error) || error.code == "NOT_READY" => {
                reasons.insert("PARENT_ORIGIN_UNAVAILABLE");
                None
            }
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    let frame_url = frame_origin.map(|origin| {
        format!(
            "{origin}/ui/{plugin}{}{}",
            spec["entry_path"].as_str().unwrap(),
            framing_query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
    });
    let value = json!({"deployment_id":deployment,"installation_id":installation,"plugin_id":plugin,"manifest_digest":row.get::<_,String>("manifest_digest"),"contribution_digest":crate::skills::definition_digest(spec)?,"spec":spec,"frame_url":frame_url,"frame_origin":frame_origin,"availability":if reasons.is_empty(){"ready"}else{"unavailable"},"reason_codes":reasons,"bindings":bindings});
    validate("UiContribution", &value)?;
    Ok(value)
}

const UI_CURSOR_DOMAIN: &str = "xelab-ui-contribution-cursor-v1";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UiCursor {
    binding: Value,
    watermark: DateTime<Utc>,
    last_plugin: String,
    last_deployment: String,
    last_contribution: String,
    expires_at: DateTime<Utc>,
}

async fn list(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    query: &str,
) -> Result<Value> {
    let mut args = if query.is_empty() {
        serde_json::Map::new()
    } else {
        strict_urlencoded(query.as_bytes(), 4096, 3)?
    };
    let limit = args
        .remove("limit")
        .map(|v| v.as_str().unwrap().parse::<usize>())
        .transpose()
        .map_err(|_| Error::invalid("Invalid UI page limit"))?
        .unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(Error::invalid("Invalid UI page limit"));
    }
    let kind = args.remove("kind").map(|v| v.as_str().unwrap().to_owned());
    if kind
        .as_deref()
        .is_some_and(|kind| kind != "window" && kind != "widget")
    {
        return Err(Error::invalid("Invalid UI contribution kind"));
    }
    let cursor = args.remove("cursor");
    if !args.is_empty() {
        return Err(Error::invalid("Unsupported UI filter"));
    }
    if !policy::registered(c, "plugin.ui.open", "plugin").await? {
        return Err(Error::denied());
    }
    let candidates = policy::PluginOwnerCandidates::default();
    let paths = Policy::for_context(c, ctx)
        .with_plugin_owner_candidates(&candidates)
        .credential(ctx.credential, 0)
        .await?;
    let ui_paths: Vec<_> = paths
        .into_iter()
        .filter(|path| {
            path.rule["action"] == "plugin.ui.open" && path.rule["resource"]["kind"] == "plugin"
        })
        .collect();
    let mut authority_bytes = 0usize;
    for path in &ui_paths {
        let bytes = serde_jcs::to_vec(path)?.len();
        crate::plugin_catalog::reserve_authority_bytes(&mut authority_bytes, bytes)?;
    }
    let mut authority_paths = ui_paths.clone();
    let mut fixed_pairs = Vec::new();
    for (plugin, owner) in candidates.pairs()? {
        let resource = Resource::new("plugin", &plugin, Some(owner));
        let paths = Policy::for_context(c, ctx)
            .with_resource_owner(&resource)
            .credential(ctx.credential, 0)
            .await?;
        let mut allowed = false;
        for path in paths {
            if auth::rules_allow(&json!([path.rule]), "plugin.ui.open", &resource, ctx.peer) {
                let bytes = serde_jcs::to_vec(&path)?.len();
                crate::plugin_catalog::reserve_authority_bytes(&mut authority_bytes, bytes)?;
                // Fixed-owner paths are ONLY an authority stamp input, never
                // global ID selectors; reject before growing beyond its cap.
                authority_paths.push(path);
                allowed = true;
            }
        }
        if allowed {
            fixed_pairs.push((plugin, owner));
        }
    }
    let authority = crate::plugin_catalog::authority_binding(c, ctx, &authority_paths).await?;
    let binding = json!({"subject":ctx.subject,"credential":ctx.credential,"chain":crate::contracts::digest(&serde_jcs::to_vec(&ctx.chain)?),"origin":ctx.browser_origin,"peer":ctx.peer,"operation":"listUiContributions","limit":limit,"kind":kind,"authority":authority,"owner_pairs":crate::contracts::digest(&serde_jcs::to_vec(&fixed_pairs)?)});
    let now = Utc::now();
    let mut position = if let Some(cursor) = cursor {
        crate::plugin_catalog::decode_opaque_cursor::<UiCursor>(
            &app.auth,
            UI_CURSOR_DOMAIN,
            "u1",
            cursor.as_str().unwrap(),
        )?
    } else {
        UiCursor {
            binding: binding.clone(),
            watermark: now,
            last_plugin: String::new(),
            last_deployment: String::new(),
            last_contribution: String::new(),
            expires_at: now + chrono::Duration::minutes(15),
        }
    };
    if position.binding != binding
        || position.expires_at <= now
        || position.watermark > now
        || position.expires_at != position.watermark + chrono::Duration::minutes(15)
    {
        return Err(Error::invalid(
            "UI cursor authority, filters or expiry changed",
        ));
    }
    let watermark = position.watermark;
    let mut last_plugin = position.last_plugin.clone();
    let mut last_deployment = position.last_deployment.clone();
    let mut last_contribution = position.last_contribution.clone();
    let selected = crate::plugin_catalog::selectors(&ui_paths, "plugin.ui.open", "plugin", ctx)?;
    let ids: Vec<_> = selected.ids.into_iter().collect();
    let owners: Vec<_> = selected.owners.into_iter().collect();
    let (pair_plugins, pair_owners): (Vec<_>, Vec<_>) = fixed_pairs.into_iter().unzip();
    let rows=c.query("SELECT d.id,d.owner_id,d.installation_id,d.spec AS deployment_spec,d.phase,d.observed_generation,d.approval,n.plugin_id,n.manifest_digest,n.manifest,n.state AS installation_state,p.status AS publisher_status,x.spec AS contribution FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id CROSS JOIN LATERAL jsonb_array_elements(n.manifest->'ui'->'contributions') x(spec) WHERE d.archived_at IS NULL AND d.created_at<=$1 AND (n.plugin_id,d.id::text,x.spec->>'id')>($2,$3,$4) AND ($5::text IS NULL OR x.spec->>'kind'=$5) AND ($6 OR n.plugin_id=ANY($7::text[]) OR d.owner_id=ANY($8::uuid[]) OR EXISTS(SELECT 1 FROM unnest($9::text[],$10::uuid[]) AS fixed(plugin_id,owner_id) WHERE fixed.plugin_id=n.plugin_id AND fixed.owner_id=d.owner_id)) ORDER BY n.plugin_id,d.id::text,x.spec->>'id' LIMIT 201", &[&watermark,&last_plugin,&last_deployment,&last_contribution,&kind,&selected.all,&ids,&owners,&pair_plugins,&pair_owners]).await?;
    let mut items = Vec::new();
    let mut processed = 0;
    for row in &rows {
        if processed == 200 || items.len() == limit {
            break;
        }
        processed += 1;
        last_plugin = row.get("plugin_id");
        last_deployment = row.get::<_, Uuid>("id").to_string();
        let spec: Value = row.get("contribution");
        last_contribution = string(&spec, "id")?.to_owned();
        match visible(c, ctx, row).await {
            Ok(()) => {
                validate_manifest(&row.get("manifest"))?;
                items.push(contribution(c, app, ctx, row, &spec).await?);
            }
            Err(error) if error.code == "RESOURCE_NOT_FOUND" => (),
            Err(error) => return Err(error),
        }
    }
    let next = if processed < rows.len() {
        position.last_plugin = last_plugin;
        position.last_deployment = last_deployment;
        position.last_contribution = last_contribution;
        Some(crate::plugin_catalog::encode_opaque_cursor(
            &app.auth,
            UI_CURSOR_DOMAIN,
            "u1",
            &position,
        )?)
    } else {
        None
    };
    let value = json!({"items":items,"next_cursor":next});
    validate("UiContributionPage", &value)?;
    Ok(value)
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
