//! Core-only validated authorization state; no plugin-controlled return URL.
use super::{clients, credential_error, session_context};
use crate::{
    auth::{Auth, Context},
    contracts::{openapi, strict_urlencoded, validate_business},
    db,
    error::{Error, Result},
    policy::{self, Path, Policy},
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_postgres::GenericClient;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationInput {
    pub client_id: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge: String,
    pub scope: String,
}
pub struct ConsentView {
    pub authorization_request_id: Uuid,
    pub client_id: String,
    pub client_name: String,
    pub scope: String,
    pub rules: Value,
    pub expires_at: DateTime<Utc>,
    pub csrf_token: String,
}
pub struct LoginContinuation {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
pub enum ConsentOutcome {
    Approved { location: String },
    Denied { location: String },
}

pub fn parse_query(query: &str) -> Result<AuthorizationInput> {
    let mut fields = strict_urlencoded(query.as_bytes(), 8192, 7)?;
    let op = &openapi()["paths"]["/oauth/authorize"]["get"];
    if fields.len() != 7 {
        return Err(Error::invalid("Missing or unknown authorization parameter"));
    }
    for p in op["parameters"].as_array().unwrap() {
        let name = p["name"].as_str().unwrap();
        let value = fields
            .get(name)
            .ok_or_else(|| Error::invalid("Missing authorization parameter"))?;
        validate_business(&p["schema"], value)?;
    }
    Ok(AuthorizationInput {
        client_id: fields.remove("client_id").unwrap().as_str().unwrap().into(),
        redirect_uri: fields
            .remove("redirect_uri")
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        state: fields.remove("state").unwrap().as_str().unwrap().into(),
        code_challenge: fields
            .remove("code_challenge")
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        scope: fields.remove("scope").unwrap().as_str().unwrap().into(),
    })
}
fn validate_input(input: &AuthorizationInput) -> Result<()> {
    let values = [
        ("client_id", &input.client_id),
        ("redirect_uri", &input.redirect_uri),
        ("state", &input.state),
        ("code_challenge", &input.code_challenge),
        ("scope", &input.scope),
    ];
    let op = &openapi()["paths"]["/oauth/authorize"]["get"];
    for (name, value) in values {
        let p = op["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap();
        validate_business(&p["schema"], &json!(value))?;
    }
    Ok(())
}
pub(crate) async fn validate_client(
    c: &(impl GenericClient + Sync),
    input: &AuthorizationInput,
) -> Result<(clients::Client, String, String, Value)> {
    validate_input(input)?;
    let client = clients::by_public_id(c, &input.client_id).await?;
    if client.status != "active"
        || !client.spec["redirect_uris"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(input.redirect_uri)))
    {
        return Err(Error::denied());
    }
    let uri = url::Url::parse(&input.redirect_uri)
        .map_err(|_| Error::invalid("Invalid registered callback"))?;
    if uri.scheme() != "https"
        || uri.host_str().is_none()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.fragment().is_some()
        || uri
            .query_pairs()
            .any(|(k, _)| matches!(k.as_ref(), "code" | "state" | "error" | "error_description"))
    {
        return Err(Error::invalid("Invalid registered callback"));
    }
    let origin = uri.origin().ascii_serialization();
    if !client.spec["allowed_origins"]
        .as_array()
        .is_some_and(|a| a.contains(&json!(origin)))
    {
        return Err(Error::denied());
    }
    let (scope, rules) = clients::requested_rules(&client, &input.scope)?;
    Ok((client, origin, scope, rules))
}
fn csrf(auth: &Auth, session: Uuid, request: Uuid) -> String {
    URL_SAFE_NO_PAD
        .encode(auth.token_digest(&format!("xelab.oauth.consent.v1:{session}:{request}")))
}

/// Caller must hold the authorization write fence before session authentication.
pub async fn begin_authorization(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    input: &AuthorizationInput,
) -> Result<ConsentView> {
    db::auth_lock(c, true).await?;
    let ctx = session_context(c, auth, ctx).await?;
    let (client, origin, scope, rules) = validate_client(c, input).await?;
    let mut policy = Policy::new(c, ctx.peer);
    let paths = policy
        .select(&ctx, &json!({"mode":"selected","rules":rules}))
        .await?;
    let fixed = policy::rules(&paths);
    let id = Uuid::new_v4();
    let csrf_token = csrf(auth, ctx.credential, id);
    let expiry = Utc::now() + chrono::Duration::minutes(10);
    c.execute("INSERT INTO authorization_requests(id,session_id,client_id,redirect_uri,state,code_challenge,fixed_rules,csrf_digest,expires_at,status,scope,origin,client_revision,fixed_paths,hmac_key_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending',$10,$11,$12,$13,$14)", &[&id,&ctx.credential,&client.client_id,&input.redirect_uri,&input.state,&input.code_challenge,&fixed,&auth.token_digest(&csrf_token),&expiry,&scope,&origin,&client.revision,&serde_json::to_value(&paths)?,&auth.key_id.as_str()]).await?;
    Ok(ConsentView {
        authorization_request_id: id,
        client_id: client.client_id,
        client_name: client.spec["name"].as_str().unwrap().to_owned(),
        scope,
        rules: fixed,
        expires_at: expiry,
        csrf_token,
    })
}

pub async fn load_consent(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    id: Uuid,
) -> Result<ConsentView> {
    let ctx = session_context(c, auth, ctx).await?;
    let row = c
        .query_opt(
            "SELECT * FROM authorization_requests WHERE id=$1 AND session_id=$2",
            &[&id, &ctx.credential],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    if row.get::<_, String>("status") != "pending"
        || row.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
        || row.get::<_, String>("hmac_key_id") != auth.key_id.as_str()
    {
        return Err(Error::conflict(
            "Authorization request is no longer pending",
        ));
    }
    let client = clients::by_public_id(c, &row.get::<_, String>("client_id")).await?;
    if client.status != "active" || client.revision != row.get::<_, i64>("client_revision") {
        return Err(Error::credential());
    }
    Ok(ConsentView {
        authorization_request_id: id,
        client_id: client.client_id,
        client_name: client.spec["name"].as_str().unwrap().to_owned(),
        scope: row.get("scope"),
        rules: row.get("fixed_rules"),
        expires_at: row.get("expires_at"),
        csrf_token: csrf(auth, ctx.credential, id),
    })
}

pub(crate) async fn live_paths(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    saved: &Value,
    ceiling: &Value,
) -> Result<Vec<Path>> {
    let paths: Vec<Path> =
        serde_json::from_value(saved.clone()).map_err(|_| Error::credential())?;
    let mut policy = Policy::new(c, ctx.peer);
    let live = policy.revalidate(&paths, ctx.credential).await?;
    let caps: Vec<Path> = ceiling
        .as_array()
        .ok_or_else(Error::credential)?
        .iter()
        .map(|rule| Path {
            rule: rule.clone(),
            sources: Default::default(),
        })
        .collect();
    policy.intersection(&live, &caps).await
}

pub async fn decide_consent(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    id: Uuid,
    approved: bool,
    csrf_token: &str,
) -> Result<ConsentOutcome> {
    db::auth_lock(c, true).await?;
    let ctx = session_context(c, auth, ctx).await?;
    let row = c
        .query_opt(
            "SELECT * FROM authorization_requests WHERE id=$1 AND session_id=$2 FOR UPDATE",
            &[&id, &ctx.credential],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    if row.get::<_, String>("status") != "pending"
        || row.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
        || row.get::<_, String>("hmac_key_id") != auth.key_id.as_str()
    {
        return Err(Error::conflict(
            "Authorization request is no longer pending",
        ));
    }
    if !auth.verify_mac(csrf_token, &row.get::<_, Vec<u8>>("csrf_digest")) {
        return Err(Error::denied());
    }
    let client = clients::by_public_id(c, &row.get::<_, String>("client_id")).await?;
    let input = AuthorizationInput {
        client_id: client.client_id.clone(),
        redirect_uri: row.get("redirect_uri"),
        state: row.get("state"),
        code_challenge: row.get("code_challenge"),
        scope: row.get("scope"),
    };
    let (current, _, _, _) = validate_client(c, &input).await?;
    if current.revision != row.get::<_, i64>("client_revision") {
        return Err(Error::credential());
    }
    if !approved {
        c.execute(
            "UPDATE authorization_requests SET status='denied' WHERE id=$1",
            &[&id],
        )
        .await?;
        db::audit(
            c,
            ctx.subject,
            Some(ctx.credential),
            ctx.request,
            "oauth.consent",
            &json!({"kind":"client","match":"ids","ids":[client.id]}),
            "failed",
            "OAUTH_CONSENT_DENIED",
        )
        .await?;
        return Ok(ConsentOutcome::Denied {
            location: callback(&input.redirect_uri, &input.state, "error", "access_denied")?,
        });
    }
    let live = live_paths(
        c,
        &ctx,
        &row.get::<_, Value>("fixed_paths"),
        &row.get::<_, Value>("fixed_rules"),
    )
    .await?;
    if !row
        .get::<_, Value>("fixed_rules")
        .as_array()
        .is_some_and(Vec::is_empty)
        && live.is_empty()
    {
        return Err(Error::credential());
    }
    let (code, digest) = auth.random_token();
    let expiry = Utc::now() + chrono::Duration::seconds(60);
    c.execute("UPDATE authorization_requests SET status='approved',code_digest=$2,expires_at=$3,fixed_rules=$4,fixed_paths=$5 WHERE id=$1", &[&id,&digest,&expiry,&policy::rules(&live),&serde_json::to_value(&live)?]).await?;
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        ctx.request,
        "oauth.consent",
        &json!({"kind":"client","match":"ids","ids":[client.id]}),
        "succeeded",
        "OAUTH_CODE_ISSUED",
    )
    .await?;
    Ok(ConsentOutcome::Approved {
        location: callback(&input.redirect_uri, &input.state, "code", &code)?,
    })
}
fn callback(uri: &str, state: &str, key: &str, value: &str) -> Result<String> {
    let mut uri =
        url::Url::parse(uri).map_err(|_| Error::invalid("Invalid registered callback"))?;
    uri.query_pairs_mut()
        .append_pair(key, value)
        .append_pair("state", state);
    Ok(uri.into())
}

pub async fn prepare_login(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    input: &AuthorizationInput,
    peer: std::net::IpAddr,
) -> Result<LoginContinuation> {
    db::auth_lock(c, true).await?;
    let (client, _, _, _) = validate_client(c, input).await?;
    c.execute(
        "DELETE FROM oauth_login_intents WHERE expires_at<=now() OR consumed_at IS NOT NULL",
        &[],
    )
    .await?;
    if c.query_one("SELECT count(*) FROM oauth_login_intents", &[])
        .await?
        .get::<_, i64>(0)
        >= 1024
        || c.query_one(
            "SELECT count(*) FROM oauth_login_intents WHERE peer=$1::text::inet",
            &[&peer.to_string()],
        )
        .await?
        .get::<_, i64>(0)
            >= 16
    {
        return Err(Error::new(
            "RATE_LIMITED",
            "Too many pending OAuth logins",
            StatusCode::TOO_MANY_REQUESTS,
        ));
    }
    let (token, digest) = auth.random_token();
    let expiry = Utc::now() + chrono::Duration::minutes(10);
    c.execute("INSERT INTO oauth_login_intents(id,hmac_key_id,verification_digest,client_id,client_revision,request_spec,peer,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7::text::inet,$8,now())", &[&Uuid::new_v4(),&auth.key_id.as_str(),&digest,&client.client_id,&client.revision,&serde_json::to_value(input)?,&peer.to_string(),&expiry]).await?;
    Ok(LoginContinuation {
        token,
        expires_at: expiry,
    })
}
pub async fn resume_login(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    token: &str,
) -> Result<ConsentView> {
    db::auth_lock(c, true).await?;
    session_context(c, auth, ctx).await?;
    let row=c.query_opt("SELECT * FROM oauth_login_intents WHERE hmac_key_id=$1 AND verification_digest=$2 FOR UPDATE", &[&auth.key_id.as_str(),&auth.token_digest(token)]).await?.ok_or_else(Error::credential)?;
    if !auth.verify_mac(token, &row.get::<_, Vec<u8>>("verification_digest"))
        || row.get::<_, Option<DateTime<Utc>>>("consumed_at").is_some()
        || row.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
    {
        return Err(Error::credential());
    }
    let input: AuthorizationInput =
        serde_json::from_value(row.get("request_spec")).map_err(|_| Error::credential())?;
    let client = clients::by_public_id(c, &input.client_id)
        .await
        .map_err(credential_error)?;
    if client.revision != row.get::<_, i64>("client_revision") {
        return Err(Error::credential());
    }
    let result = begin_authorization(c, auth, ctx, &input).await?;
    c.execute(
        "UPDATE oauth_login_intents SET consumed_at=now() WHERE id=$1",
        &[&row.get::<_, Uuid>("id")],
    )
    .await?;
    Ok(result)
}
