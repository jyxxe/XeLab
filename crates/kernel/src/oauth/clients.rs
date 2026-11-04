//! Registered public browser clients. Scope handles describe a ceiling; they
//! never grant the consenting account or its credential additional authority.
use crate::{
    api::{etag, path_id},
    auth::{self, Auth, Context, Resource},
    contracts::{string, validate},
    db,
    error::{Error, Result},
    policy::{self, Policy},
};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use tokio_postgres::{GenericClient, Row};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct Client {
    pub id: Uuid,
    pub client_id: String,
    pub created_by: Option<Uuid>,
    pub spec: Value,
    pub revision: i64,
    pub status: String,
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "createClient" | "getClient" | "listClients" | "updateClient" | "disableClient"
    )
}

fn from_row(row: &Row) -> Client {
    Client {
        id: row.get("id"),
        client_id: row.get("client_id"),
        created_by: row.get("created_by"),
        spec: row.get("spec"),
        revision: row.get("revision"),
        status: row.get("status"),
    }
}

pub(crate) async fn by_public_id(
    c: &(impl GenericClient + Sync),
    client_id: &str,
) -> Result<Client> {
    let row = c
        .query_opt(
            "SELECT * FROM clients WHERE client_id=$1 FOR SHARE",
            &[&client_id],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let client = from_row(&row);
    validate_spec(&client.spec)?;
    Ok(client)
}

pub(crate) async fn by_id(c: &(impl GenericClient + Sync), id: Uuid) -> Result<Client> {
    c.query_opt("SELECT * FROM clients WHERE id=$1", &[&id])
        .await?
        .map(|row| from_row(&row))
        .ok_or_else(Error::not_found)
}

fn value(row: &Row) -> Result<Value> {
    let mut value = db::stamp(row);
    value["client_id"] = json!(row.get::<_, String>("client_id"));
    value["spec"] = row.get("spec");
    value["status"] = json!(row.get::<_, String>("status"));
    validate("Client", &value)?;
    Ok(value)
}

async fn read(c: &(impl GenericClient + Sync), id: Uuid) -> Result<Value> {
    let row = c
        .query_opt("SELECT * FROM clients WHERE id=$1", &[&id])
        .await?
        .ok_or_else(Error::not_found)?;
    value(&row)
}

/// Public registration deliberately has no HTTP loopback exception. Such an
/// exception requires a separate trusted native-client registration profile.
fn https_url(input: &str) -> Result<Url> {
    if input.is_empty()
        || input.bytes().any(|byte| byte.is_ascii_control())
        || input.contains(['\\', '*'])
    {
        return Err(Error::invalid("Expected a canonical HTTPS URI"));
    }
    let parsed = Url::parse(input).map_err(|_| Error::invalid("Expected a canonical HTTPS URI"))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.as_str() != input
    {
        return Err(Error::invalid(
            "Expected a canonical HTTPS URI without userinfo or fragment",
        ));
    }
    Ok(parsed)
}

pub(crate) fn validate_spec(spec: &Value) -> Result<()> {
    validate("ClientSpec", spec)?;
    let mut origins = HashSet::new();
    for origin in spec["allowed_origins"].as_array().unwrap() {
        let origin = origin.as_str().unwrap();
        // Url's serialization appends '/' to an origin, whereas the origin
        // serialization is exactly scheme://host[:port]. Check that identity.
        let parsed =
            Url::parse(origin).map_err(|_| Error::invalid("Expected a canonical HTTPS origin"))?;
        if origin.bytes().any(|byte| byte.is_ascii_control())
            || origin.contains(['\\', '*'])
            || parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.origin().ascii_serialization() != origin
            || !origins.insert(origin)
        {
            return Err(Error::invalid(
                "Expected distinct canonical HTTPS origins without paths",
            ));
        }
    }
    let mut callbacks = HashSet::new();
    for callback in spec["redirect_uris"].as_array().unwrap() {
        let callback = callback.as_str().unwrap();
        let parsed = https_url(callback)?;
        if !origins.contains(parsed.origin().ascii_serialization().as_str())
            || !callbacks.insert(callback)
            || parsed.query_pairs().any(|(name, _)| {
                ["code", "state", "error", "error_description"]
                    .iter()
                    .any(|reserved| name.eq_ignore_ascii_case(reserved))
            })
        {
            return Err(Error::invalid(
                "Callback must have an allowed origin and no reserved OAuth query fields",
            ));
        }
    }
    // Every individual scope handle must have a deterministic JCS identity.
    for rule in spec["maximum_rules"].as_array().unwrap() {
        crate::skills::definition_digest(rule)?;
    }
    Ok(())
}

/// Return a normalized, registered handle set and its rules. The OAuth issuer
/// must intersect these with current user/source authority, and persist this
/// Client's current revision in its authorization request and credential family.
pub(crate) fn requested_rules(client: &Client, scope: &str) -> Result<(String, Value)> {
    if client.status != "active"
        || scope.is_empty()
        || scope.len() > 512
        || !scope.is_ascii()
        || scope.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(Error::invalid("Invalid registered OAuth scope"));
    }
    let handles: BTreeSet<_> = scope.split(' ').filter(|part| !part.is_empty()).collect();
    if handles.is_empty() {
        return Err(Error::invalid("Invalid registered OAuth scope"));
    }
    let registered: BTreeMap<_, _> = client.spec["maximum_rules"]
        .as_array()
        .ok_or_else(|| Error::invalid("Invalid Client rule ceiling"))?
        .iter()
        .map(|rule| {
            crate::skills::definition_digest(rule)
                .map(|hash| (format!("rule:{hash}"), rule.clone()))
        })
        .collect::<Result<_>>()?;
    let full = format!("client:{}", client.client_id);
    let mut selected = BTreeMap::new();
    for handle in &handles {
        if *handle == full {
            selected.extend(registered.clone());
        } else if let Some(rule) = registered.get(*handle) {
            selected.insert((*handle).to_owned(), rule.clone());
        } else {
            return Err(Error::invalid(
                "OAuth scope is not registered for this Client",
            ));
        }
    }
    Ok((
        handles.into_iter().collect::<Vec<_>>().join(" "),
        Value::Array(selected.into_values().collect()),
    ))
}

pub async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    _body: &Value,
) -> Result<()> {
    let result = match name {
        "listClients" => Ok(()), // Recheck client.read for every returned object.
        "createClient" => {
            auth::authorize(
                c,
                ctx,
                "client.create",
                &Resource::new("client", "new", Some(ctx.subject)),
            )
            .await
        }
        "getClient" | "updateClient" | "disableClient" => {
            let client = by_id(c, path_id(params, "client_id")?).await?;
            let action = match name {
                "getClient" => "client.read",
                "updateClient" => "client.update",
                _ => "client.disable",
            };
            auth::authorize(
                c,
                ctx,
                action,
                &Resource::new("client", client.id, client.created_by),
            )
            .await
        }
        _ => Err(Error::not_found()),
    };
    result.map_err(|error| {
        if name == "getClient" && error.code == "PERMISSION_DENIED" {
            Error::not_found()
        } else {
            error
        }
    })
}

async fn invalidate(c: &(impl GenericClient + Sync), client_id: &str) -> Result<()> {
    c.execute("UPDATE oauth_families SET status='revoked',updated_at=now() WHERE client_id=$1 AND status='active'", &[&client_id]).await?;
    c.execute("UPDATE credentials SET status='revoked',revision=revision+1,updated_at=now() WHERE client_id=$1 AND kind IN ('oauth_access','oauth_refresh') AND status<>'revoked'", &[&client_id]).await?;
    c.execute("UPDATE authorization_requests SET status='denied' WHERE client_id=$1 AND status IN ('pending','approved')", &[&client_id]).await?;
    c.execute(
        "DELETE FROM oauth_login_intents WHERE client_id=$1",
        &[&client_id],
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    if name != "listClients" && !query.is_empty() {
        return Err(Error::invalid("Unexpected Client query parameter"));
    }
    match name {
        "createClient" => {
            validate_spec(body)?;
            Policy::for_context(c, ctx)
                .require_subset(ctx, body["maximum_rules"].as_array().unwrap())
                .await?;
            let id = Uuid::new_v4();
            let public_id = format!("xelab-{}", Uuid::new_v4().simple());
            c.execute("INSERT INTO clients(id,client_id,created_by,spec,status,created_at,updated_at) VALUES($1,$2,$3,$4,'active',now(),now())", &[&id,&public_id,&ctx.subject,&body]).await?;
            Ok((read(c, id).await?, 201))
        }
        "getClient" => Ok((read(c, path_id(params, "client_id")?).await?, 200)),
        "listClients" => Ok((list(c, auth, ctx, query).await?, 200)),
        "updateClient" | "disableClient" => {
            let id = path_id(params, "client_id")?;
            let row = c
                .query_opt("SELECT * FROM clients WHERE id=$1 FOR UPDATE", &[&id])
                .await?
                .ok_or_else(Error::not_found)?;
            etag(headers, row.get("revision"))?;
            let client_id: String = row.get("client_id");
            if name == "disableClient" {
                c.execute("UPDATE clients SET status='disabled',revision=revision+1,updated_at=now() WHERE id=$1 AND status='active'", &[&id]).await?;
                invalidate(c, &client_id).await?;
                return Ok((Value::Null, 204));
            }
            if row.get::<_, String>("status") != "active" {
                return Err(Error::conflict(
                    "A disabled Client cannot be updated or reactivated",
                ));
            }
            validate_spec(body)?;
            Policy::for_context(c, ctx)
                .require_subset(ctx, body["maximum_rules"].as_array().unwrap())
                .await?;
            c.execute(
                "UPDATE clients SET spec=$2,revision=revision+1,updated_at=now() WHERE id=$1",
                &[&id, &body],
            )
            .await?;
            invalidate(c, &client_id).await?;
            Ok((read(c, id).await?, 200))
        }
        _ => Err(Error::not_found()),
    }
}

async fn list(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    query: &str,
) -> Result<Value> {
    let mut arguments = HashMap::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if arguments
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(Error::invalid("Duplicate query parameter"));
        }
    }
    let limit = arguments
        .remove("limit")
        .map(|limit| limit.parse::<usize>())
        .transpose()
        .map_err(|_| Error::invalid("Invalid page limit"))?
        .unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(Error::invalid("Invalid page limit"));
    }
    let cursor = arguments.remove("cursor");
    if !arguments.is_empty() {
        return Err(Error::invalid("Unsupported Client filter"));
    }
    let binding = json!({"subject":ctx.subject,"credential":ctx.credential,"operation":"listClients","limit":limit});
    let mut watermark = Utc::now();
    let mut last_time = DateTime::<Utc>::UNIX_EPOCH;
    let mut last_id = String::new();
    if let Some(cursor) = cursor {
        if cursor.len() > 2048 {
            return Err(Error::invalid("Invalid cursor"));
        }
        let (raw, signature) = cursor
            .split_once('.')
            .ok_or_else(|| Error::invalid("Invalid cursor"))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| Error::invalid("Invalid cursor"))?;
        if !auth.verify_mac(raw, &signature) {
            return Err(Error::invalid("Invalid cursor"));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(raw)
            .map_err(|_| Error::invalid("Invalid cursor"))?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| Error::invalid("Invalid cursor"))?;
        if value["binding"] != binding || policy::timestamp(&value["expires_at"])? <= Utc::now() {
            return Err(Error::invalid("Cursor identity, filters or expiry differ"));
        }
        watermark = policy::timestamp(&value["watermark"])?;
        last_time = policy::timestamp(&value["last_time"])?;
        last_id = string(&value, "last_id")?.to_owned();
    }
    let rows = c.query("SELECT * FROM clients WHERE created_at<=$1 AND (created_at,id::text)>($2,$3) ORDER BY created_at,id LIMIT 201", &[&watermark,&last_time,&last_id]).await?;
    let mut items = Vec::new();
    let mut processed = 0;
    for row in &rows {
        if processed == 200 || items.len() == limit {
            break;
        }
        processed += 1;
        let id: Uuid = row.get("id");
        last_id = id.to_string();
        last_time = row.get("created_at");
        match auth::authorize(
            c,
            ctx,
            "client.read",
            &Resource::new("client", id, row.get("created_by")),
        )
        .await
        {
            Ok(()) => items.push(value(row)?),
            Err(error) if error.code == "PERMISSION_DENIED" => (),
            Err(error) => return Err(error),
        }
    }
    let next = if processed < rows.len() {
        let raw = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"binding":binding,"watermark":watermark,"last_time":last_time,"last_id":last_id,"expires_at":watermark+chrono::Duration::minutes(15)}))?);
        Some(format!(
            "{raw}.{}",
            URL_SAFE_NO_PAD.encode(auth.token_digest(&raw))
        ))
    } else {
        None
    };
    let page = json!({"items":items,"next_cursor":next});
    validate("ClientPage", &page)?;
    Ok(page)
}

#[cfg(test)]
#[path = "clients_tests.rs"]
mod tests;
