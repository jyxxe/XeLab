//! Approved browser shells remain independent signed OCI plugins. A default
//! selection is a fixed deployment, never a publisher URL or a name lookup.
use crate::{
    api::{etag, App},
    auth::{self, Context, Resource},
    contracts::{id, string, validate},
    db,
    error::{Error, Result},
    plugin_http::{self, transport::StaticOrigin},
};
use axum::http::{HeaderMap, Method};
use serde_json::{json, Value};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub fn handles(name: &str) -> bool {
    matches!(name, "getDefaultUi" | "setDefaultUi")
}

/// Preserve historical selection without inferring any current publisher,
/// approval, client or runtime authority from a snapshot.
pub(crate) async fn validate_snapshot(c: &(impl GenericClient + Sync)) -> Result<()> {
    let mut after: Option<Uuid> = None;
    loop {
        let rows=c.query("SELECT id,manifest FROM installations WHERE manifest ? 'webui' AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 200",&[&after]).await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            after = Some(row.get("id"));
            validate_manifest(&row.get("manifest"))?;
        }
    }
    for row in c.query("SELECT * FROM default_ui", &[]).await? {
        value(&row)?;
    }
    Ok(())
}

pub(crate) fn entry_path(manifest: &Value) -> Result<String> {
    let webui = &manifest["webui"];
    let base = string(webui, "base_path")?;
    let entry = string(webui, "entry_path")?;
    plugin_http::canonical_path(entry)?;
    let path = format!("{}{}", base, &entry[1..]);
    plugin_http::canonical_path(&path)?;
    Ok(path)
}

/// The publisher declares only a path in its own static namespace. The
/// generated deployment origin is checked separately, once its UUID exists.
pub(crate) fn validate_manifest(manifest: &Value) -> Result<()> {
    let routes = manifest["routes"]
        .as_array()
        .ok_or_else(|| Error::invalid("Missing routes"))?;
    let has_routes = routes.iter().any(|route| route["kind"] == "webui");
    let Some(spec) = manifest.get("webui") else {
        return if has_routes {
            Err(Error::invalid("WebUI routes require signed WebUI metadata"))
        } else {
            Ok(())
        };
    };
    validate("WebUiSpec", spec)?;
    let base = format!("/ui/{}/", string(manifest, "plugin_id")?);
    if spec["base_path"] != base || spec["isolation"] != "dedicated_origin" {
        return Err(Error::invalid(
            "WebUI requires its own allocated origin and namespace",
        ));
    }
    if let Some(origin) = spec["origin"].as_str() {
        let url = url::Url::parse(origin).map_err(|_| Error::invalid("Invalid WebUI origin"))?;
        if url.scheme() != "https"
            || url.origin().ascii_serialization() != origin
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::invalid("WebUI origin must be an exact HTTPS origin"));
        }
    }
    let entry = entry_path(manifest)?;
    if !routes.iter().any(|route| {
        route["kind"] == "webui"
            && route["path_prefix"] == base
            && route["authentication"] == "public"
            && route["maximum_body_bytes"] == 0
            && route["methods"].as_array().is_some_and(|methods| {
                methods.len() == 2
                    && methods.contains(&json!("GET"))
                    && methods.contains(&json!("HEAD"))
            })
    }) || !entry.starts_with(&base)
    {
        return Err(Error::invalid(
            "WebUI entry requires its signed public GET/HEAD route",
        ));
    }
    Ok(())
}

pub(crate) struct Selected {
    pub manifest: Value,
    pub origin: String,
    pub entry_url: String,
}

const READY: &str = "SELECT n.id installation_id,n.manifest FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id WHERE d.id=$1 AND d.archived_at IS NULL AND d.spec->>'enabled'='true' AND d.phase='ready' AND n.state='installed' AND p.status='trusted' AND i.phase='ready' AND i.health='alive' AND i.generation=d.observed_generation AND i.last_heartbeat_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint)";

pub(crate) async fn check_deployment(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
) -> Result<Selected> {
    let rows = c.query(READY, &[&deployment]).await?;
    if rows.len() != 1 {
        return Err(Error::unavailable(
            "Selected WebUI is unavailable; use the CLI or core API",
        ));
    }
    let row = &rows[0];
    let manifest: Value = row.get("manifest");
    if manifest.get("webui").is_none() {
        return Err(Error::invalid("Selected deployment has no signed WebUI"));
    }
    validate_manifest(&manifest)?;
    let origin = StaticOrigin::for_deployment(&app.config, deployment, None)?
        .origin()
        .to_owned();
    if manifest["webui"]["origin"]
        .as_str()
        .is_some_and(|declared| declared != origin)
    {
        return Err(Error::invalid(
            "Signed WebUI origin differs from its allocated deployment",
        ));
    }
    let installation = row.get("installation_id");
    let registry = crate::http_contracts::load(c, installation).await?;
    let path = entry_path(&manifest)?;
    if registry.resolve_static(&Method::GET, &path)?.kind != "webui" {
        return Err(Error::denied());
    }
    crate::plugin_http_gateway::check_approval(c, app, deployment, &manifest).await?;
    if !registered_client(c, &manifest, &origin).await? {
        return Err(Error::unavailable(
            "Selected WebUI has no active registered browser client; use the CLI or core API",
        ));
    }
    Ok(Selected {
        manifest,
        entry_url: format!("{origin}{path}"),
        origin,
    })
}

async fn registered_client(
    c: &(impl GenericClient + Sync),
    manifest: &Value,
    origin: &str,
) -> Result<bool> {
    let rows=c.query("SELECT spec FROM clients WHERE status='active' AND spec->>'plugin_id'=$1 AND spec->'allowed_origins' @> $2::jsonb LIMIT 201",&[&string(manifest,"plugin_id")?,&json!([origin])]).await?;
    if rows.len() > 200 {
        return Err(Error::exhausted());
    }
    let base = string(&manifest["webui"], "base_path")?;
    for row in rows {
        let spec: Value = row.get("spec");
        crate::oauth::clients::validate_spec(&spec)?;
        if spec["redirect_uris"]
            .as_array()
            .unwrap()
            .iter()
            .any(|redirect| {
                url::Url::parse(redirect.as_str().unwrap()).is_ok_and(|url| {
                    url.origin().ascii_serialization() == origin && url.path().starts_with(base)
                })
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The platform may authorize a newly allocated official shell without a
/// configuration restart. Never derive embedding permission from arbitrary
/// Client origins: the exact origin must belong to a fresh, approved WebUI.
pub(crate) async fn frame_ancestors(
    c: &(impl GenericClient + Sync),
    app: &App,
    contribution: Uuid,
) -> Result<Vec<String>> {
    let own = StaticOrigin::for_deployment(&app.config, contribution, None)?
        .origin()
        .to_owned();
    let mut origins = app.config.value["ui_gateway"]["shell_origins"]
        .as_array()
        .ok_or_else(|| Error::unavailable("UI gateway is not configured"))?
        .iter()
        .filter_map(Value::as_str)
        .filter(|value| *value != own)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if let Some(row) = c
        .query_opt("SELECT deployment_id FROM default_ui WHERE singleton", &[])
        .await?
    {
        if let Some(deployment) = row
            .get::<_, Option<Uuid>>(0)
            .filter(|id| *id != contribution)
        {
            match check_deployment(c, app, deployment).await {
                Ok(shell) => origins.push(shell.origin),
                Err(error)
                    if matches!(
                        error.code,
                        "PERMISSION_DENIED"
                            | "INVALID_CREDENTIAL"
                            | "NOT_READY"
                            | "RESOURCE_NOT_FOUND"
                            | "INVALID_ARGUMENT"
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
    origins.sort_unstable();
    origins.dedup();
    if origins.iter().map(String::len).sum::<usize>() > 16384 {
        return Err(Error::exhausted());
    }
    Ok(origins)
}

pub async fn authorize(c: &(impl GenericClient + Sync), ctx: &Context, name: &str) -> Result<()> {
    let action = match name {
        "getDefaultUi" => "platform.read",
        "setDefaultUi" => "platform.configure",
        _ => return Err(Error::not_found()),
    };
    auth::authorize(c, ctx, action, &Resource::new("platform", "node", None)).await
}

fn value(row: &tokio_postgres::Row) -> Result<Value> {
    let mut value = db::stamp(row);
    value["deployment_id"] = json!(row.get::<_, Option<Uuid>>("deployment_id"));
    validate("DefaultUi", &value)?;
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    name: &str,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    if !query.is_empty() {
        return Err(Error::invalid("Unexpected default UI query"));
    }
    authorize(c, ctx, name).await?;
    // Only an authenticated, authorized management transaction creates this
    // singleton. Migrations and anonymous root requests must leave an empty
    // recovery target empty for the trusted COPY import protocol.
    c.execute("INSERT INTO default_ui(id,deployment_id,created_at,updated_at) VALUES($1,NULL,now(),now()) ON CONFLICT(singleton) DO NOTHING", &[&Uuid::new_v4()]).await?;
    let row = c
        .query_one("SELECT * FROM default_ui WHERE singleton FOR UPDATE", &[])
        .await?;
    if name == "getDefaultUi" {
        return Ok((value(&row)?, 200));
    }
    if name != "setDefaultUi" {
        return Err(Error::not_found());
    }
    validate("DefaultUiSpec", body)?;
    etag(headers, row.get("revision"))?;
    let deployment = if body["deployment_id"].is_null() {
        None
    } else {
        Some(id(body, "deployment_id")?)
    };
    if let Some(deployment) = deployment {
        check_deployment(c, app, deployment).await?;
    }
    let row = c.query_one("UPDATE default_ui SET deployment_id=$1,revision=revision+1,updated_at=now() WHERE singleton RETURNING *", &[&deployment]).await?;
    Ok((value(&row)?, 200))
}

/// No next/return URL is accepted. The public landing route does not create
/// configuration or credentials and remains independent of the management API.
pub async fn redirect(app: &App) -> Result<String> {
    crate::skill_streams::ensure_active(app)?;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut connection = app
            .db
            .connect_scoped(std::time::Duration::from_secs(10))
            .await?;
        let tx = connection.transaction().await?;
        db::auth_lock(&tx, false).await?;
        let deployment = tx
            .query_opt("SELECT deployment_id FROM default_ui WHERE singleton", &[])
            .await?
            .and_then(|row| row.get::<_, Option<Uuid>>(0))
            .ok_or_else(|| {
                Error::unavailable("No default WebUI is available; use the CLI or core API")
            })?;
        let selected = check_deployment(&tx, app, deployment).await?;
        tx.commit().await?;
        Ok(selected.entry_url)
    })
    .await
    .map_err(|_| Error::unavailable("Default WebUI lookup exceeded its deadline"))?
}

/// OAuth callback query parameters stay only in the browser location. They
/// are admitted at an exact active registered callback and are never forwarded
/// into the plugin's static HTTP request or exposed to its server-side logs.
pub(crate) async fn validate_callback(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    path: &str,
    query: &str,
) -> Result<()> {
    plugin_http::canonical_path(path)?;
    let selected = check_deployment(c, app, deployment).await?;
    let items = crate::contracts::strict_urlencoded(query.as_bytes(), 8192, 32)?;
    if items.contains_key(frame_policy::QUERY) {
        return Err(Error::invalid(
            "Reserved framing query is not an OAuth callback",
        ));
    }
    let success = items
        .get("code")
        .and_then(Value::as_str)
        .is_some_and(|v| !v.is_empty())
        && !items.contains_key("error");
    let denied = items
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|v| !v.is_empty())
        && !items.contains_key("code");
    if !(success || denied)
        || !items
            .get("state")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
    {
        return Err(Error::invalid("Invalid OAuth callback response"));
    }
    let rows = c.query("SELECT spec FROM clients WHERE status='active' AND spec->>'plugin_id'=$1 AND spec->'allowed_origins' @> $2::jsonb LIMIT 201", &[&string(&selected.manifest,"plugin_id")?, &json!([selected.origin])]).await?;
    if rows.len() > 200 {
        return Err(Error::exhausted());
    }
    for row in rows {
        let spec: Value = row.get("spec");
        crate::oauth::clients::validate_spec(&spec)?;
        for redirect in spec["redirect_uris"].as_array().unwrap() {
            let registered =
                url::Url::parse(redirect.as_str().unwrap()).map_err(|_| Error::denied())?;
            if registered.origin().ascii_serialization() != selected.origin
                || registered.path() != path
            {
                continue;
            }
            let expected = if let Some(query) = registered.query() {
                crate::contracts::strict_urlencoded(query.as_bytes(), 8192, 32)?
            } else {
                serde_json::Map::new()
            };
            if expected.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "code" | "state" | "error" | "error_description"
                )
            }) {
                continue;
            }
            if expected
                .iter()
                .any(|(key, value)| items.get(key) != Some(value))
            {
                continue;
            }
            if items.keys().all(|key| {
                expected.contains_key(key)
                    || if success {
                        matches!(key.as_str(), "code" | "state")
                    } else {
                        matches!(key.as_str(), "error" | "error_description" | "state")
                    }
            }) {
                return Ok(());
            }
        }
    }
    Err(Error::denied())
}

/// Only an approved WebUI shell may contact the trusted public core. Ordinary
/// contributed pages keep connect-src 'none' and communicate through the bridge.
pub(crate) async fn csp(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
) -> Result<String> {
    let selected = check_deployment(c, app, deployment).await?;
    if selected.manifest != *manifest {
        return Err(Error::conflict("WebUI binding changed"));
    }
    // A dedicated allocation namespace discloses no individual deployment.
    // Exact Host, signed static routes, current approval and Bridge authority
    // remain mandatory; this source expression grants no core API access.
    let base = app.config.text("ui_gateway", "base_domain");
    let port = app.config.value["ui_gateway"]["public_port"]
        .as_u64()
        .ok_or_else(Error::denied)?;
    let suffix = if port == 443 {
        String::new()
    } else {
        format!(":{port}")
    };
    let frames = format!("https://*.{base}{suffix}");
    Ok(format!("default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' blob:; media-src 'self' blob:; connect-src {}; frame-src {frames}; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",app.config.text("server","public_origin")))
}

#[path = "webui_frame_policy.rs"]
mod frame_policy;
pub(crate) use frame_policy::{framing_query, validate_framing_query};

#[cfg(test)]
#[path = "webui_tests.rs"]
mod tests;
