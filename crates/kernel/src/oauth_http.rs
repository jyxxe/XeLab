//! OAuth HTTP encoding and trusted native-form handling. Authorization and
//! credential issuance remain in the shared Identity/Authorizer core.
use crate::{
    api::App,
    auth::{Auth, Context},
    contracts::{self, strict_json, validate},
    db,
    error::{Error, Result},
    oauth::{self, authorization, tokens::ExchangeOutcome},
    trusted_auth,
};
use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Request, StatusCode},
    response::Response,
};
use serde_json::{json, Map, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_postgres::GenericClient;
use uuid::Uuid;

const MAXIMUM_FORM_BYTES: usize = 16 * 1024;
const CONTINUATION_COOKIE: &str = "__Host-xelab_oauth";
const AUTH_COOKIE: &str = "__Host-xelab_auth";
const CONSENT_PATH: &str = "/api/platform/v1/auth/consents";

/// Called only after the core Host has been selected. Re-checking it here also
/// protects component callers; plugin Hosts can never acquire trusted pages.
pub fn handles(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        (
            "GET",
            "/oauth/authorize" | "/auth/ui/login" | "/auth/ui/consent"
        ) | ("POST", "/oauth/token" | CONSENT_PATH)
    ) || (method == "OPTIONS" && (path == "/oauth/token" || path.starts_with("/api/platform/v1/")))
}

fn cookie(headers: &HeaderMap, name: &str) -> Result<Option<String>> {
    let mut found = None;
    for line in headers.get_all("cookie") {
        for part in line.to_str().map_err(|_| Error::credential())?.split(';') {
            let (key, value) = part.trim().split_once('=').ok_or_else(Error::credential)?;
            if key == name
                && (value.len() < 32
                    || value.len() > 512
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                    || found.replace(value.to_owned()).is_some())
            {
                return Err(Error::credential());
            }
        }
    }
    Ok(found)
}
fn origin(headers: &HeaderMap) -> Result<Option<&str>> {
    let value = one_header(headers, "origin")?;
    if let Some(value) = value {
        let parsed = crate::config::checked_origin(value).map_err(|_| Error::denied())?;
        if parsed.origin().ascii_serialization() != value {
            return Err(Error::denied());
        }
    }
    Ok(value)
}
fn trusted_host(app: &App, request: &Request<Body>) -> Result<()> {
    let public = app.config.origin()?;
    let authority = &public[url::Position::BeforeHost..url::Position::AfterPort];
    let host = one_header(request.headers(), "host")?;
    let uri_authority = request.uri().authority().map(|a| a.as_str());
    let valid = if request.version() == axum::http::Version::HTTP_2 {
        // HTTP/2 transports :authority/:scheme in the URI. Any Host field must
        // agree exactly; accepting h2 must not enable h1 absolute-form routing.
        host.is_none_or(|h| h == authority)
            && uri_authority.is_none_or(|h| h == authority)
            && (host == Some(authority) || uri_authority == Some(authority))
            && request.uri().scheme_str().is_none_or(|s| s == "https")
    } else {
        host == Some(authority) && uri_authority.is_none() && request.uri().scheme().is_none()
    };
    if !valid {
        return Err(Error::not_found());
    }
    let path = request.uri().path();
    if path.contains('%') || path.contains("//") || path.contains('\\') {
        return Err(Error::invalid("Noncanonical authentication path"));
    }
    if request.headers().contains_key("content-encoding") {
        return Err(Error::invalid(
            "Encoded authentication bodies are not accepted",
        ));
    }
    Ok(())
}
fn clear_cookie(name: &str) -> String {
    format!("{name}=; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=0")
}
fn redirect(location: &str, request: Uuid) -> Result<Response> {
    Response::builder()
        .status(302)
        .header("location", location)
        .header("cache-control", "no-store")
        .header("pragma", "no-cache")
        .header("referrer-policy", "no-referrer")
        .header("x-request-id", request.to_string())
        .body(Body::empty())
        .map_err(|_| Error::unavailable("Cannot construct authentication redirect"))
}
fn consent_location(id: Uuid) -> String {
    format!("/auth/ui/consent?authorization_request_id={id}")
}
fn append_cookie(response: &mut Response, value: String) -> Result<()> {
    response.headers_mut().append(
        "set-cookie",
        value
            .parse()
            .map_err(|_| Error::unavailable("Cannot construct authentication Cookie"))?,
    );
    Ok(())
}
async fn session(
    c: &(impl GenericClient + Sync),
    app: &App,
    headers: &HeaderMap,
    peer: SocketAddr,
    request: Uuid,
) -> Result<Context> {
    if headers.contains_key("authorization") {
        return Err(Error::credential());
    }
    let token = cookie(headers, AUTH_COOKIE)?.ok_or_else(Error::credential)?;
    let ctx = app.auth.authenticate(c, &token, peer.ip(), request).await?;
    oauth::session_context(c, &app.auth, &ctx).await
}

/// Authentication plus browser credential classification. B's ordinary
/// same-core Cookie/CSRF checks still apply after this helper. A foreign origin
/// can use only a direct OAuth access Bearer and never an ambient core Cookie.
pub async fn credential_context(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    headers: &HeaderMap,
    peer: SocketAddr,
    request: Uuid,
    core_origin: &str,
) -> Result<Context> {
    let browser = origin(headers)?;
    let cookie = cookie(headers, AUTH_COOKIE)?;
    if headers.contains_key("authorization") && cookie.is_some() {
        return Err(Error::credential());
    }
    let foreign = browser.is_some_and(|value| value != core_origin);
    if foreign && (!headers.contains_key("authorization") || headers.contains_key("cookie")) {
        return Err(Error::denied());
    }
    let mut ctx = auth
        .authenticate(c, crate::api::bearer(headers)?, peer.ip(), request)
        .await?;
    let kind: String = c
        .query_one(
            "SELECT kind FROM credentials WHERE id=$1",
            &[&ctx.credential],
        )
        .await?
        .get(0);
    if cookie.is_some() && kind != "session" {
        return Err(Error::credential());
    }
    if foreign && kind != "oauth_access" {
        return Err(Error::denied());
    }
    if kind == "oauth_access" {
        oauth::validate_origin(c, ctx.credential, browser).await?;
    }
    // Origin restrictions on account/grant ancestors apply to individual
    // authority alternatives in Policy, not to fixed password-session self
    // service or an independently authorized Owner alternative.
    ctx.browser_origin = browser.map(str::to_owned);
    Ok(ctx)
}

/// Also accepts the original charged proof's expanded credential sources. This
/// lets a business-call adapter enforce grant issuers without replacing a
/// frozen authorization alternative with an unrelated grant.
pub async fn validate_source_origins(
    c: &(impl GenericClient + Sync),
    sources: &[Uuid],
    browser_origin: Option<&str>,
) -> Result<()> {
    if browser_origin.is_none() {
        return Ok(());
    }
    for row in c
        .query(
            "SELECT id FROM credentials WHERE id=ANY($1) AND kind='oauth_access'",
            &[&sources],
        )
        .await?
    {
        oauth::validate_origin(c, row.get(0), browser_origin).await?;
    }
    Ok(())
}
async fn registered_origin(c: &(impl GenericClient + Sync), value: &str) -> Result<bool> {
    Ok(c.query_one("SELECT EXISTS(SELECT 1 FROM clients WHERE status='active' AND spec->'allowed_origins' ? $1)",&[&value]).await?.get(0))
}
fn cors(response: &mut Response, value: &str) -> Result<()> {
    response.headers_mut().insert(
        "access-control-allow-origin",
        value.parse().map_err(|_| Error::denied())?,
    );
    response
        .headers_mut()
        .append("vary", "Origin".parse().unwrap());
    response.headers_mut().insert(
        "access-control-expose-headers",
        "ETag, X-Request-ID".parse().unwrap(),
    );
    Ok(())
}
/// Error adapters may use this for registered origins too. It discloses no
/// credential and conveys no authority; actual requests authenticate again.
pub async fn cors_response(
    c: &(impl GenericClient + Sync),
    headers: &HeaderMap,
    response: &mut Response,
) -> Result<()> {
    if let Some(value) = origin(headers)? {
        if registered_origin(c, value).await? {
            cors(response, value)?;
        }
    }
    Ok(())
}

pub async fn handle(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
) -> Response {
    let token = request.uri().path() == "/oauth/token";
    let login = request.uri().path() == "/auth/ui/login";
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        dispatch(&app, peer, request_id, request),
    )
    .await
    .unwrap_or_else(|_| {
        Err(Error::unavailable(
            "Authentication request deadline elapsed",
        ))
    });
    match result {
        Ok(response) => response,
        Err(error) => {
            // OAuth's declared infrastructure 5xx responses use ErrorEnvelope;
            // validation/client/grant 4xx use OAuthError, never that envelope.
            let mut response = if token && error.status.is_client_error() {
                crate::api::response(json!({"error":"invalid_request"}), 400, request_id)
            } else {
                crate::api::response(
                    json!({"error":error.detail(),"request_id":request_id}),
                    error.status.as_u16(),
                    request_id,
                )
            };
            if login {
                let _ = append_cookie(&mut response, clear_cookie(CONTINUATION_COOKIE));
            }
            response
        }
    }
}

async fn dispatch(
    app: &App,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
) -> Result<Response> {
    trusted_host(app, &request)?;
    if let Some(value) = origin(request.headers())? {
        if request.uri().path() != "/oauth/token"
            && request.method() != "OPTIONS"
            && value != app.config.text("server", "public_origin")
        {
            return Err(Error::denied());
        }
    }
    crate::skill_streams::ensure_active(app)?;
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if !handles(&method, &path) {
        return Err(Error::not_found());
    }
    let query = request.uri().query().map(str::to_owned);
    let headers = request.headers().clone();
    let bytes = to_bytes(request.into_body(), MAXIMUM_FORM_BYTES)
        .await
        .map_err(|_| {
            Error::new(
                "PAYLOAD_TOO_LARGE",
                "Authentication body exceeds limit",
                StatusCode::PAYLOAD_TOO_LARGE,
            )
        })?;
    if (method == "GET" || method == "OPTIONS") && !bytes.is_empty() {
        return Err(Error::invalid("Authentication GET requires an empty body"));
    }
    if path != "/oauth/authorize" && path != "/auth/ui/consent" && query.is_some() {
        return Err(Error::invalid("Unexpected authentication query"));
    }
    if method == "OPTIONS" {
        return preflight(app, &headers, &path, request_id).await;
    }
    if path == "/oauth/token" {
        return token_exchange(app, peer, request_id, &headers, &bytes).await;
    }
    // Malformed or duplicate protected Cookies are transport errors, distinct
    // from a well-formed expired session that may recover through login.
    cookie(&headers, AUTH_COOKIE)?;
    cookie(&headers, CONTINUATION_COOKIE)?;
    let mut connection = app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = connection.transaction().await?;
    // Every route here may create/consume authority, so the write fence must
    // precede authenticate's read fence. A shared-to-exclusive upgrade deadlocks.
    db::auth_lock(&tx, true).await?;
    let result = match path.as_str() {
        "/oauth/authorize" => {
            if headers.contains_key("authorization") {
                return Err(Error::credential());
            }
            let input = authorization::parse_query(query.as_deref().unwrap_or(""))?;
            match session(&tx, app, &headers, peer, request_id).await {
                Ok(ctx) => {
                    let view =
                        authorization::begin_authorization(&tx, &app.auth, &ctx, &input).await?;
                    redirect(&consent_location(view.authorization_request_id), request_id)?
                }
                Err(error) if error.code == "INVALID_CREDENTIAL" => {
                    let continuation =
                        authorization::prepare_login(&tx, &app.auth, &input, peer.ip()).await?;
                    let mut response = redirect("/auth/ui/login", request_id)?;
                    append_cookie(&mut response,format!("{CONTINUATION_COOKIE}={}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age=600",continuation.token))?;
                    append_cookie(&mut response, clear_cookie(AUTH_COOKIE))?;
                    response
                }
                Err(error) => return Err(error),
            }
        }
        "/auth/ui/login" => {
            if headers.contains_key("authorization") {
                return Err(Error::credential());
            }
            let continuation = cookie(&headers, CONTINUATION_COOKIE)?;
            if let Some(token) = continuation.as_deref() {
                match session(&tx, app, &headers, peer, request_id).await {
                    Ok(ctx) => {
                        let view = authorization::resume_login(&tx, &app.auth, &ctx, token).await?;
                        let mut response =
                            redirect(&consent_location(view.authorization_request_id), request_id)?;
                        append_cookie(&mut response, clear_cookie(CONTINUATION_COOKIE))?;
                        response
                    }
                    Err(error) if error.code == "INVALID_CREDENTIAL" => {
                        trusted_auth::login(Some("/auth/ui/login"))?.into_response(request_id)?
                    }
                    Err(error) => return Err(error),
                }
            } else {
                trusted_auth::login(None)?.into_response(request_id)?
            }
        }
        "/auth/ui/consent" => {
            let fields =
                contracts::strict_urlencoded(query.as_deref().unwrap_or("").as_bytes(), 256, 1)?;
            if fields.len() != 1 || !fields.contains_key("authorization_request_id") {
                return Err(Error::invalid("Unexpected consent query"));
            }
            let id = contracts::id(&Value::Object(fields), "authorization_request_id")?;
            let ctx = session(&tx, app, &headers, peer, request_id).await?;
            let view = authorization::load_consent(&tx, &app.auth, &ctx, id).await?;
            let row=tx.query_one("SELECT a.display_name,r.redirect_uri,c.spec FROM authorization_requests r JOIN credentials k ON k.id=r.session_id JOIN accounts a ON a.id=k.owner_id JOIN clients c ON c.client_id=r.client_id WHERE r.id=$1",&[&id]).await?;
            let spec: Value = row.get("spec");
            trusted_auth::consent(trusted_auth::Consent {
                authorization_request: id,
                idempotency_key: Uuid::new_v4(),
                csrf_token: &view.csrf_token,
                account_name: &row.get::<_, String>("display_name"),
                client_name: &view.client_name,
                plugin_id: contracts::string(&spec, "plugin_id")?,
                redirect_uri: &row.get::<_, String>("redirect_uri"),
                expires_at: view.expires_at,
                rules: &view.rules,
            })?
            .into_response(request_id)?
        }
        CONSENT_PATH => {
            if query.is_some()
                || origin(&headers)? != Some(app.config.text("server", "public_origin"))
            {
                return Err(Error::denied());
            }
            let submission = consent_request(&headers, &bytes)?;
            let ctx = session(&tx, app, &headers, peer, request_id).await?;
            consent(&tx, app, &ctx, request_id, submission).await?
        }
        _ => return Err(Error::not_found()),
    };
    crate::skill_streams::ensure_active(app)?;
    tx.commit().await?;
    crate::skill_streams::ensure_active(app)?;
    Ok(result)
}

async fn consent(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    request: Uuid,
    submission: ConsentSubmission,
) -> Result<Response> {
    let id = contracts::id(&submission.decision, "authorization_request_id")?;
    let fingerprint = contracts::digest(&app.auth.token_digest(&serde_json::to_string(
        &json!({"decision":submission.decision,"csrf":submission.csrf_token}),
    )?));
    let key = submission.idempotency_key;
    if let Some(row)=c.query_opt("SELECT fingerprint,resource_id FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$3 AND key=$4 AND expires_at>now()",&[&ctx.subject,&ctx.credential,&CONSENT_PATH,&key]).await? {
        if row.get::<_,String>("fingerprint")!=fingerprint {return Err(Error::new("IDEMPOTENCY_CONFLICT","Idempotency key fingerprint differs",StatusCode::CONFLICT));}
        return Err(Error::new("ONE_TIME_RESPONSE_UNAVAILABLE","The one-time authorization result cannot be retrieved again",StatusCode::CONFLICT).with_resource(row.get::<_,String>("resource_id")));
    }
    let outcome = authorization::decide_consent(
        c,
        &app.auth,
        ctx,
        id,
        submission.decision["approved"].as_bool().unwrap(),
        &submission.csrf_token,
    )
    .await?;
    c.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path=$3 AND key=$4 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&CONSENT_PATH,&key]).await?;
    c.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,response_status,safe_response,sensitive,resource_id,expires_at,created_at) VALUES($1,$2,'POST',$3,$4,$5,302,NULL,true,$6,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&CONSENT_PATH,&key,&fingerprint,&id.to_string()]).await?;
    match outcome {
        authorization::ConsentOutcome::Approved { location }
        | authorization::ConsentOutcome::Denied { location } => redirect(&location, request),
    }
}

async fn token_exchange(
    app: &App,
    peer: SocketAddr,
    request: Uuid,
    headers: &HeaderMap,
    bytes: &[u8],
) -> Result<Response> {
    if headers.contains_key("authorization") || headers.contains_key("cookie") {
        return Err(Error::invalid("Token proof cannot use ambient credentials"));
    }
    let browser = origin(headers)?;
    let fields = if media_type(headers)?.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        form(bytes)?
    } else {
        return Err(Error::invalid("Token endpoint requires URL encoded form"));
    };
    if fields
        .get("grant_type")
        .and_then(Value::as_str)
        .is_some_and(|s| s != "authorization_code" && s != "refresh_token")
    {
        return Ok(crate::api::response(
            json!({"error":"unsupported_grant_type"}),
            400,
            request,
        ));
    }
    let input = token_request(headers, bytes)?;
    let allowed = if let Some(value) = browser {
        let c = app.db.connect_scoped(Duration::from_secs(10)).await?;
        registered_origin(&*c, value).await?
    } else {
        false
    };
    let outcome =
        oauth::tokens::exchange(&app.db, &app.auth, peer.ip(), request, &input, browser).await?;
    crate::skill_streams::ensure_active(app)?;
    let mut response = match outcome {
        ExchangeOutcome::Token(v) => crate::api::response(v, 200, request),
        ExchangeOutcome::Rejected { status, body } => crate::api::response(body, status, request),
    };
    if allowed {
        cors(&mut response, browser.unwrap())?;
    }
    response
        .headers_mut()
        .insert("pragma", "no-cache".parse().unwrap());
    Ok(response)
}

async fn preflight(app: &App, headers: &HeaderMap, path: &str, request: Uuid) -> Result<Response> {
    if headers.contains_key("cookie") || headers.contains_key("authorization") {
        return Err(Error::denied());
    }
    let browser = origin(headers)?.ok_or_else(Error::denied)?;
    let method = one_header(headers, "access-control-request-method")?.ok_or_else(Error::denied)?;
    if path == "/oauth/token" {
        if method != "POST" {
            return Err(Error::denied());
        }
    } else {
        let (operation, _) = crate::api::operation(method, path).ok_or_else(Error::not_found)?;
        if !path.starts_with("/api/platform/v1/")
            || matches!(
                operation["operationId"].as_str(),
                Some("login" | "activateAccount" | "submitConsent")
            )
        {
            return Err(Error::denied());
        }
    }
    let requested = one_header(headers, "access-control-request-headers")?.unwrap_or("");
    let mut names = std::collections::BTreeSet::new();
    if !requested.is_empty() {
        for name in requested.split(',') {
            let name = name.trim().to_ascii_lowercase();
            if !matches!(
                name.as_str(),
                "authorization"
                    | "content-type"
                    | "idempotency-key"
                    | "if-match"
                    | "accept"
                    | "range"
                    | "x-request-id"
            ) || !names.insert(name)
            {
                return Err(Error::denied());
            }
        }
    }
    let c = app.db.connect_scoped(Duration::from_secs(10)).await?;
    if !registered_origin(&*c, browser).await? {
        return Err(Error::denied());
    }
    let mut response = crate::api::response(Value::Null, 204, request);
    cors(&mut response, browser)?;
    response.headers_mut().append(
        "vary",
        "Access-Control-Request-Method, Access-Control-Request-Headers"
            .parse()
            .unwrap(),
    );
    response.headers_mut().insert(
        "access-control-allow-methods",
        method.parse().map_err(|_| Error::denied())?,
    );
    if !names.is_empty() {
        response.headers_mut().insert(
            "access-control-allow-headers",
            names
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
                .parse()
                .map_err(|_| Error::denied())?,
        );
    }
    Ok(response)
}

pub(crate) struct ConsentSubmission {
    pub decision: Value,
    pub csrf_token: String,
    pub idempotency_key: String,
}

fn one_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>> {
    if headers.get_all(name).iter().count() > 1 {
        return Err(Error::invalid("Repeated authentication header"));
    }
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Error::invalid("Invalid authentication header"))
        })
        .transpose()
}

fn media_type(headers: &HeaderMap) -> Result<&str> {
    if headers.contains_key("content-encoding") {
        return Err(Error::invalid(
            "Encoded authentication bodies are not accepted",
        ));
    }
    let value = one_header(headers, "content-type")?
        .ok_or_else(|| Error::invalid("Authentication Content-Type is required"))?;
    let mut parts = value.split(';');
    let media = parts.next().unwrap_or("").trim();
    let mut charset = false;
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            return Err(Error::invalid("Invalid authentication Content-Type"));
        };
        if charset
            || !name.trim().eq_ignore_ascii_case("charset")
            || !value.trim().eq_ignore_ascii_case("utf-8")
        {
            return Err(Error::invalid("Unsupported authentication body encoding"));
        }
        charset = true;
    }
    Ok(media)
}

fn form(bytes: &[u8]) -> Result<Map<String, Value>> {
    crate::contracts::strict_urlencoded(bytes, MAXIMUM_FORM_BYTES, 16)
}

pub(crate) fn token_request(headers: &HeaderMap, bytes: &[u8]) -> Result<Value> {
    if !media_type(headers)?.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
        return Err(Error::invalid(
            "OAuth token endpoint requires a URL encoded form",
        ));
    }
    let value = Value::Object(form(bytes)?);
    validate("OAuthTokenRequest", &value)?;
    Ok(value)
}

fn metadata(
    headers: &HeaderMap,
    fields: &mut Map<String, Value>,
    name: &str,
    minimum: usize,
    maximum: usize,
) -> Result<String> {
    let header = one_header(headers, name)?;
    let field = fields.remove(name);
    let value = match (header, field) {
        (Some(value), None) => value.to_owned(),
        (None, Some(Value::String(value))) => value,
        _ => {
            return Err(Error::invalid(
                "Authentication metadata must have one source",
            ))
        }
    };
    if !(minimum..=maximum).contains(&value.chars().count()) {
        return Err(Error::invalid("Authentication metadata size is invalid"));
    }
    if !axum::http::HeaderValue::from_str(&value)
        .ok()
        .is_some_and(|header| header.to_str().is_ok())
    {
        return Err(Error::invalid(
            "Authentication metadata is not a valid header value",
        ));
    }
    Ok(value)
}

pub(crate) fn consent_request(headers: &HeaderMap, bytes: &[u8]) -> Result<ConsentSubmission> {
    let media = media_type(headers)?;
    let native = media.eq_ignore_ascii_case("application/x-www-form-urlencoded");
    let mut fields = if native {
        let fields = form(bytes)?;
        validate("ConsentForm", &Value::Object(fields.clone()))?;
        fields
    } else if media.eq_ignore_ascii_case("application/json") {
        let value = strict_json(bytes, MAXIMUM_FORM_BYTES)?;
        validate("ConsentDecision", &value)?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| Error::invalid("Consent decision must be an object"))?
    } else {
        return Err(Error::invalid("Unsupported consent Content-Type"));
    };
    let csrf_token = metadata(headers, &mut fields, "X-CSRF-Token", 43, 128)?;
    let idempotency_key = metadata(headers, &mut fields, "Idempotency-Key", 1, 128)?;
    if native {
        let approved = match fields.remove("approved") {
            Some(Value::String(value)) if value == "true" => true,
            Some(Value::String(value)) if value == "false" => false,
            _ => return Err(Error::invalid("Consent approved must be true or false")),
        };
        fields.insert("approved".into(), Value::Bool(approved));
    }
    let decision = Value::Object(fields);
    validate("ConsentDecision", &decision)?;
    Ok(ConsentSubmission {
        decision,
        csrf_token,
        idempotency_key,
    })
}

#[cfg(test)]
#[path = "oauth_http_tests.rs"]
mod http_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(media: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_str(media).unwrap());
        headers
    }

    #[test]
    fn token_form_rejects_duplicates_aliases_bad_utf8_and_mixed_grants() {
        let headers = headers("application/x-www-form-urlencoded; charset=UTF-8");
        let valid = b"grant_type=refresh_token&client_id=client&refresh_token=credential";
        assert_eq!(
            token_request(&headers, valid).unwrap()["grant_type"],
            "refresh_token"
        );
        for body in [
            b"grant_type=refresh_token&%67rant_type=authorization_code&client_id=x&refresh_token=x"
                .as_slice(),
            b"grant_type=refresh_token&client_id=x&refresh_token=%ff",
            b"grant_type=refresh_token&client_id=x&refresh_token=%G1",
            b"grant_type=refresh_token&client_id=x&refresh_token=%",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&code=also-code",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&account_id=forged",
            b"grant_type=refresh_token&client_id=x&refresh_token=x&",
        ] {
            assert!(token_request(&headers, body).is_err(), "{body:?}");
        }
        assert!(form(&vec![b'a'; MAXIMUM_FORM_BYTES + 1]).is_err());
        assert!(token_request(&self::headers("application/json"), valid).is_err());
    }

    #[test]
    fn native_consent_metadata_is_exclusive_and_decision_stays_unchanged() {
        let mut headers = headers("application/x-www-form-urlencoded");
        let csrf = "a".repeat(43);
        let body = format!("authorization_request_id=00000000-0000-4000-8000-000000000001&approved=false&X-CSRF-Token={csrf}&Idempotency-Key=one-time");
        let parsed = consent_request(&headers, body.as_bytes()).unwrap();
        assert_eq!(parsed.decision["approved"], false);
        assert_eq!(parsed.decision.as_object().unwrap().len(), 2);
        assert_eq!(parsed.csrf_token, csrf);
        assert_eq!(parsed.idempotency_key, "one-time");
        headers.insert("x-csrf-token", HeaderValue::from_str(&csrf).unwrap());
        assert!(consent_request(&headers, body.as_bytes()).is_err());
        headers.remove("x-csrf-token");
        headers.insert("idempotency-key", HeaderValue::from_static("different"));
        assert!(consent_request(&headers, body.as_bytes()).is_err());
        headers.remove("idempotency-key");
        for value in ["1", "yes", "True", "false&approved=true"] {
            assert!(consent_request(
                &headers,
                body.replace("approved=false", &format!("approved={value}"))
                    .as_bytes()
            )
            .is_err());
        }
        for invalid in ["%00", "%0D%0Aforged", "%E4%B8%AD"] {
            assert!(
                consent_request(
                    &headers,
                    body.replace(
                        "Idempotency-Key=one-time",
                        &format!("Idempotency-Key={invalid}")
                    )
                    .as_bytes()
                )
                .is_err(),
                "native metadata must retain HTTP header validity"
            );
        }
    }

    #[test]
    fn json_consent_does_not_accept_form_metadata_or_duplicate_headers() {
        let mut headers = headers("application/json");
        headers.insert(
            "x-csrf-token",
            HeaderValue::from_str(&"b".repeat(43)).unwrap(),
        );
        headers.insert("idempotency-key", HeaderValue::from_static("request-one"));
        let value = serde_json::json!({"authorization_request_id":"00000000-0000-4000-8000-000000000001","approved":true});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(consent_request(&headers, &bytes).unwrap().decision["approved"] == true);
        let mut with_metadata = value;
        with_metadata["X-CSRF-Token"] = Value::String("b".repeat(43));
        assert!(consent_request(&headers, &serde_json::to_vec(&with_metadata).unwrap()).is_err());
        headers.append("idempotency-key", HeaderValue::from_static("request-one"));
        assert!(consent_request(&headers, &bytes).is_err());
    }
}
