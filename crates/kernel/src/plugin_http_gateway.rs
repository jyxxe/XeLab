//! The public HTTP boundary. Host allocation precedes every trusted endpoint;
//! business admission stays in Broker and static files retain only read access.
use crate::{
    api::{self, App},
    auth::{self, Context, Resource},
    config::Config,
    contracts::{digest, string},
    db,
    error::{Error, Result},
    plugin_http::{
        self,
        transport::{InstanceHttp, StaticOrigin},
        Route,
    },
    policy::{self, Path, Policy},
};
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::Response,
};
use serde_json::{json, Value};
use std::{
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::GenericClient;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Core,
    Deployment(Uuid),
}

fn authority(origin: &url::Url) -> String {
    let host = match origin.host() {
        Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
        Some(host) => host.to_string(),
        None => String::new(),
    };
    origin
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or(host)
}
pub(crate) fn core_authority(config: &Config) -> Result<String> {
    Ok(authority(&config.origin()?))
}

/// Host never comes from forwarding headers. An upstream proxy must preserve
/// the exact externally allocated Host on its authenticated core connection.
pub(crate) fn host(config: &Config, request: &Request<Body>) -> Result<Host> {
    let headers = request.headers();
    if headers.contains_key("forwarded")
        || headers
            .keys()
            .any(|name| name.as_str().starts_with("x-forwarded-"))
    {
        return Err(Error::invalid("Forwarded origin headers are not accepted"));
    }
    let uri = request.uri();
    if uri.scheme_str().is_some_and(|scheme| scheme != "https") {
        return Err(Error::invalid("Invalid request origin scheme"));
    }
    let host = if headers.contains_key("host") {
        let host = api::header(headers, "host")?;
        if uri.authority().is_some_and(|a| a.as_str() != host) {
            return Err(Error::invalid("Conflicting HTTP authorities"));
        }
        host
    } else {
        uri.authority()
            .map(|a| a.as_str())
            .ok_or_else(|| Error::invalid("An explicit HTTP authority is required"))?
    };
    if host == core_authority(config)? {
        return Ok(Host::Core);
    }
    let gateway = &config.value["ui_gateway"];
    if !gateway.is_null() {
        let base = string(gateway, "base_domain")?;
        let port = gateway["public_port"]
            .as_u64()
            .ok_or_else(|| Error::invalid("Invalid UI gateway port"))?;
        let suffix = if port == 443 {
            format!(".{base}")
        } else {
            format!(".{base}:{port}")
        };
        if let Some(id) = host
            .strip_prefix("d-")
            .and_then(|s| s.strip_suffix(&suffix))
        {
            if id.len() == 32
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                let deployment = Uuid::parse_str(id).map_err(|_| Error::not_found())?;
                return Ok(Host::Deployment(deployment));
            }
        }
    }
    Err(Error::not_found())
}

fn cookie(headers: &HeaderMap) -> Result<Option<&str>> {
    let mut found = None;
    for value in headers.get_all("cookie") {
        for pair in value.to_str().map_err(|_| Error::credential())?.split(';') {
            if let Some(token) = pair.trim().strip_prefix("__Host-xelab_auth=") {
                if !(32..=512).contains(&token.len()) || found.replace(token).is_some() {
                    return Err(Error::invalid("Ambiguous login Cookie"));
                }
            }
        }
    }
    Ok(found)
}

/// Shared by the existing core API and the business proxy. Origin is exact,
/// Cookie mutations require a session-bound CSRF proof, and a second credential
/// cannot silently select a different account.
pub(crate) async fn request_security(
    app: &App,
    peer: IpAddr,
    request: Uuid,
    method: &Method,
    headers: &HeaderMap,
) -> Result<()> {
    tokio::time::timeout(
        Duration::from_secs(10),
        request_security_inner(app, peer, request, method, headers),
    )
    .await
    .map_err(|_| deadline())?
}
async fn request_security_inner(
    app: &App,
    peer: IpAddr,
    request: Uuid,
    method: &Method,
    headers: &HeaderMap,
) -> Result<()> {
    let trusted = app.config.text("server", "public_origin");
    if crate::oauth_http::request_origin(headers)?.is_some_and(|origin| origin != trusted) {
        // Foreign browser requests have no ambient core credential. This
        // preliminary classification does not replace action/resource policy
        // checks in the actual request transaction or Broker actor.
        let mut c = app.db.connect_scoped(Duration::from_secs(10)).await?;
        let tx = c.transaction().await?;
        crate::oauth_http::credential_context(
            &tx,
            &app.auth,
            headers,
            SocketAddr::new(peer, 0),
            request,
            trusted,
        )
        .await?;
        tx.commit().await?;
        return Ok(());
    }
    let login_cookie = cookie(headers)?;
    if matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        if let Some(token) = login_cookie {
            if headers.get("origin").and_then(|value| value.to_str().ok()) != Some(trusted) {
                return Err(Error::denied());
            }
            app.auth.verify_csrf(
                token,
                api::header(headers, "x-csrf-token").map_err(|_| Error::denied())?,
            )?;
        }
    }
    if let Some(cookie) = login_cookie {
        let mut connection = app.db.connect_scoped(Duration::from_secs(5)).await?;
        let transaction = connection.transaction().await?;
        db::auth_lock(&transaction, false).await?;
        let second = app
            .auth
            .authenticate(&transaction, cookie, peer, request)
            .await?;
        let kind: String = transaction
            .query_one(
                "SELECT kind FROM credentials WHERE id=$1",
                &[&second.credential],
            )
            .await?
            .get(0);
        if kind != "session" {
            return Err(Error::credential());
        }
        if headers.contains_key("authorization") {
            let first = app
                .auth
                .authenticate(&transaction, api::bearer(headers)?, peer, request)
                .await?;
            if first.subject != second.subject {
                return Err(Error::invalid("Bearer and Cookie subjects differ"));
            }
        }
        transaction.commit().await?;
    }
    Ok(())
}
/// A fresh password/activation credential does not inherit an old Cookie's
/// authority. Browser requests still require the exact trusted origin; an old
/// expired session must not prevent the user from authenticating again.
pub(crate) fn fresh_credential_security(app: &App, headers: &HeaderMap) -> Result<()> {
    if headers.get_all("origin").iter().count() > 1 {
        return Err(Error::invalid("Duplicate Origin"));
    }
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    let trusted = app.config.text("server", "public_origin");
    if (headers.contains_key("origin") && origin != Some(trusted))
        || (headers.contains_key("cookie") && origin != Some(trusted))
    {
        return Err(Error::denied());
    }
    Ok(())
}

pub(crate) async fn approve(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    manifest: &Value,
    approval: &Value,
    owner: Uuid,
) -> Result<()> {
    if !manifest["routes"]
        .as_array()
        .is_some_and(|routes| !routes.is_empty())
    {
        c.execute(
            "DELETE FROM plugin_http_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await?;
        return Ok(());
    }
    crate::http_contracts::validate_actions(c, manifest).await?;
    let resource = Resource::new("deployment", deployment, Some(owner));
    auth::authorize(c, ctx, "plugin.permission.grant", &resource).await?;
    let current = Policy::for_context(c, ctx)
        .credential(ctx.credential, 0)
        .await?;
    let paths: Vec<_> = current
        .into_iter()
        .filter(|path| {
            auth::rules_allow(
                &json!([path.rule]),
                "plugin.permission.grant",
                &resource,
                ctx.peer,
            )
        })
        .collect();
    if paths.is_empty() {
        return Err(Error::denied());
    }
    let installation: Uuid = c
        .query_one(
            "SELECT installation_id FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await?
        .get(0);
    let fingerprint = approval_fingerprint(approval, installation, manifest)?;
    c.execute("INSERT INTO plugin_http_approvals(deployment_id,credential_id,peer,approval_digest,source_paths,browser_origin,created_at) VALUES($1,$2,$3::text::inet,$4,$5,$6,now()) ON CONFLICT(deployment_id) DO UPDATE SET credential_id=$2,peer=$3::text::inet,approval_digest=$4,source_paths=$5,browser_origin=$6,created_at=now()",&[&deployment,&ctx.credential,&ctx.peer.to_string(),&fingerprint,&json!(paths),&ctx.browser_origin]).await?;
    Ok(())
}

pub(crate) async fn check_approval(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
) -> Result<()> {
    check_approval_inner(c, app, deployment, manifest, None).await
}

pub(crate) async fn check_approval_prepared(
    transaction: &tokio_postgres::Transaction<'_>,
    app: &App,
    deployment: Uuid,
    manifest: &Value,
    context: &Context,
    authority: &auth::CurrentAuthority<'_, '_>,
) -> Result<()> {
    authority.verify(transaction, context)?;
    check_approval_inner(
        transaction,
        app,
        deployment,
        manifest,
        Some((transaction, context, authority)),
    )
    .await
}

async fn check_approval_inner(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
    prepared: Option<(
        &tokio_postgres::Transaction<'_>,
        &Context,
        &auth::CurrentAuthority<'_, '_>,
    )>,
) -> Result<()> {
    if !manifest["routes"]
        .as_array()
        .is_some_and(|routes| !routes.is_empty())
    {
        return Ok(());
    }
    crate::http_contracts::validate_actions(c, manifest).await?;
    let row=c.query_opt("SELECT d.approval,d.owner_id,d.installation_id,a.credential_id,host(a.peer) peer,a.approval_digest,a.source_paths,a.browser_origin FROM deployments d JOIN plugin_http_approvals a ON a.deployment_id=d.id WHERE d.id=$1",&[&deployment]).await?.ok_or_else(Error::denied)?;
    let approval: Value = row.get("approval");
    if approval_fingerprint(&approval, row.get("installation_id"), manifest)?
        != row.get::<_, String>("approval_digest")
    {
        return Err(Error::denied());
    }
    let peer: IpAddr = row
        .get::<_, String>("peer")
        .parse()
        .map_err(|_| Error::denied())?;
    let mut ctx = app
        .auth
        .resume(
            c,
            row.get("credential_id"),
            peer,
            prepared
                .map(|(_, ctx, _)| ctx.request)
                .unwrap_or_else(Uuid::new_v4),
        )
        .await?;
    ctx.browser_origin = row.get("browser_origin");
    let saved: Vec<Path> =
        serde_json::from_value(row.get("source_paths")).map_err(|_| Error::denied())?;
    let current = if let Some((transaction, caller, authority)) =
        prepared.filter(|(_, caller, _)| {
            caller.credential == ctx.credential
                && caller.peer == ctx.peer
                && caller.subject == ctx.subject
                && caller.browser_origin == ctx.browser_origin
        }) {
        authority.verify(transaction, caller)?;
        authority.revalidate(transaction, &ctx, &saved).await?
    } else {
        Policy::for_context(c, &ctx)
            .revalidate(&saved, ctx.credential)
            .await?
    };
    let resource = Resource::new("deployment", deployment, Some(row.get("owner_id")));
    if !auth::rules_allow(
        &policy::rules(&current),
        "plugin.permission.grant",
        &resource,
        peer,
    ) {
        return Err(Error::denied());
    }
    // revalidate already intersects the original proof with this credential's
    // current rights; evaluating the same graph again conveys no extra proof.
    Ok(())
}
fn approval_fingerprint(approval: &Value, installation: Uuid, manifest: &Value) -> Result<String> {
    Ok(digest(&serde_json::to_vec(
        &json!({"approval":approval,"installation_id":installation,"manifest":manifest}),
    )?))
}

pub(crate) async fn handle(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    host: Host,
    request: Request<Body>,
) -> Result<Response> {
    handle_with_ingress(app, peer, request_id, host, request, ingress_pool()).await
}
fn ingress_pool() -> Arc<Semaphore> {
    static POOL: OnceLock<Arc<Semaphore>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Semaphore::new(32))).clone()
}
async fn handle_with_ingress(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    host: Host,
    request: Request<Body>,
    ingress: Arc<Semaphore>,
) -> Result<Response> {
    let started = tokio::time::Instant::now();
    crate::skill_streams::ensure_active(&app)?;
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    plugin_http::canonical_path(path)?;
    plugin_http::reject_upgrade(&parts.headers)?;
    if !path.starts_with("/plugins/") && !path.starts_with("/ui/") {
        return Err(Error::not_found());
    }
    // Reserve bounded metadata work before either public query profile can
    // open a connection or validate a current deployment/client binding.
    let _ingress = ingress
        .try_acquire_owned()
        .map_err(|_| Error::exhausted())?;
    let mut framing = None;
    if let Some(query) = parts.uri.query() {
        let deployment = match host {
            Host::Deployment(id) if matches!(parts.method.as_str(), "GET" | "HEAD") => id,
            _ => return Err(Error::invalid("Undeclared plugin query parameters")),
        };
        // Both reserved query profiles terminate at the core. The backend
        // receives the fixed signed path alone, including for public pages.
        framing = tokio::time::timeout_at(started + Duration::from_secs(10), async {
            let mut c = app.db.connect_scoped(Duration::from_secs(10)).await?;
            let tx = c.transaction().await?;
            db::auth_lock(&tx, false).await?;
            let framing = if query.starts_with("xelab_frame=") {
                Some(FramingPolicy {
                    query: query.to_owned(),
                    parent: crate::webui::validate_framing_query(
                        &tx, &app, deployment, path, query,
                    )
                    .await?,
                })
            } else {
                crate::webui::validate_callback(&tx, &app, deployment, path, query).await?;
                None
            };
            tx.commit().await?;
            Ok::<_, Error>(framing)
        })
        .await
        .map_err(|_| deadline())??;
    }
    let fixed = match host {
        Host::Core => None,
        Host::Deployment(id) => Some(id),
    };
    if let Some(deployment) = fixed.filter(|_| {
        path.starts_with("/plugins/")
            && (parts.method == Method::GET || parts.method == Method::HEAD)
    }) {
        if tokio::time::timeout_at(
            started + Duration::from_secs(10),
            declared_static(&app, deployment, &parts.method, path),
        )
        .await
        .map_err(|_| deadline())??
        {
            let bytes =
                tokio::time::timeout_at(started + Duration::from_secs(10), to_bytes(body, 0))
                    .await
                    .map_err(|_| deadline())?
                    .map_err(|_| plugin_http::too_large())?;
            if !bytes.is_empty() {
                return Err(Error::invalid("Static requests must be bodyless"));
            }
            return static_page(
                app,
                peer.ip(),
                request_id,
                deployment,
                &parts.method,
                path,
                &parts.headers,
                started,
                framing,
            )
            .await;
        }
    }
    if path.starts_with("/plugins/") {
        tokio::time::timeout_at(
            started + Duration::from_secs(10),
            request_security(&app, peer.ip(), request_id, &parts.method, &parts.headers),
        )
        .await
        .map_err(|_| deadline())??;
        let token = api::bearer(&parts.headers)?.to_owned();
        let key = if matches!(parts.method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
            Some(api::header(&parts.headers, "idempotency-key")?)
        } else {
            None
        };
        let bytes = tokio::time::timeout_at(
            started + Duration::from_secs(10),
            to_bytes(body, 16 * 1024 * 1024),
        )
        .await
        .map_err(|_| deadline())?
        .map_err(|_| plugin_http::too_large())?;
        return crate::broker::invoke_http(
            app,
            &token,
            peer.ip(),
            request_id,
            &parts.method,
            path,
            &parts.headers,
            &bytes,
            key,
            fixed,
            started,
        )
        .await;
    }
    let deployment = fixed.ok_or_else(Error::not_found)?;
    if !path.starts_with("/ui/") {
        return Err(Error::not_found());
    }
    let bytes = tokio::time::timeout_at(started + Duration::from_secs(10), to_bytes(body, 0))
        .await
        .map_err(|_| deadline())?
        .map_err(|_| plugin_http::too_large())?;
    if !bytes.is_empty() {
        return Err(Error::invalid("Static requests must be bodyless"));
    }
    static_page(
        app,
        peer.ip(),
        request_id,
        deployment,
        &parts.method,
        path,
        &parts.headers,
        started,
        framing,
    )
    .await
}
async fn declared_static(app: &App, deployment: Uuid, method: &Method, path: &str) -> Result<bool> {
    let mut connection = app.db.connect_scoped(Duration::from_secs(3)).await?;
    let transaction = connection.transaction().await?;
    db::auth_lock(&transaction, false).await?;
    let installation:Uuid=transaction.query_opt("SELECT d.installation_id FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id WHERE d.id=$1 AND d.archived_at IS NULL AND n.state='installed' AND p.status='trusted'",&[&deployment]).await?.ok_or_else(Error::not_found)?.get(0);
    let registry = crate::http_contracts::load(&transaction, installation).await?;
    let result = match registry.resolve_static(method, path) {
        Ok(_) => true,
        Err(error) if error.code == "RESOURCE_NOT_FOUND" => false,
        Err(error) => return Err(error),
    };
    transaction.commit().await?;
    Ok(result)
}

fn pool() -> Arc<Semaphore> {
    static POOL: OnceLock<Arc<Semaphore>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(Semaphore::new(32))).clone()
}
fn retain(response: Response, permit: OwnedSemaphorePermit) -> Response {
    let (parts, body) = response.into_parts();
    let mut source = body.into_data_stream();
    Response::from_parts(
        parts,
        Body::from_stream(futures_util::stream::poll_fn(move |cx| {
            let _held = &permit;
            futures_util::Stream::poll_next(Pin::new(&mut source), cx)
        })),
    )
}

struct FramingPolicy {
    query: String,
    parent: String,
}
#[derive(Clone)]
struct StaticBinding {
    installation: Uuid,
    deployment: Uuid,
    instance: Uuid,
    generation: i64,
    owner: Uuid,
    manifest: Value,
    approval: Value,
    route: Route,
    ctx: Option<Context>,
    paths: Vec<Path>,
    deadline: tokio::time::Instant,
    lease: Uuid,
}
struct StaticLease {
    app: Arc<App>,
    id: Uuid,
    settled: bool,
    deployment: Uuid,
    started: tokio::time::Instant,
    phase_started: tokio::time::Instant,
    phase: &'static str,
}
impl StaticLease {
    fn new(app: Arc<App>, id: Uuid, deployment: Uuid, started: tokio::time::Instant) -> Self {
        Self {
            app,
            id,
            settled: false,
            deployment,
            started,
            phase_started: tokio::time::Instant::now(),
            phase: "transport_setup",
        }
    }
    fn phase(&mut self, phase: &'static str) {
        self.phase = phase;
        self.phase_started = tokio::time::Instant::now();
    }
    async fn complete(&mut self) -> Result<()> {
        let mut connection = self.app.db.connect_scoped(Duration::from_secs(3)).await?;
        let transaction = connection.transaction().await?;
        db::call_lock(&transaction).await?;
        let deleted = transaction
            .execute(
                "DELETE FROM plugin_http_requests WHERE id=$1 AND state='active'",
                &[&self.id],
            )
            .await?;
        if deleted == 0
            && transaction
                .query_one(
                    "SELECT EXISTS(SELECT 1 FROM plugin_http_requests WHERE id=$1)",
                    &[&self.id],
                )
                .await?
                .get::<_, bool>(0)
        {
            return Err(Error::conflict(
                "An unknown static request requires a physical fence",
            ));
        }
        transaction.commit().await?;
        self.settled = true;
        Ok(())
    }
}
impl Drop for StaticLease {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        tracing::info!(target:"xelab.static_transition",code="STATIC_UNSETTLED",stage=self.phase,deployment_id=%self.deployment,elapsed_ms=self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,phase_ms=self.phase_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        let app = self.app.clone();
        let id = self.id;
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(3), async {
                    let mut connection = app.db.connect_scoped(Duration::from_secs(3)).await?;
                    let transaction = connection.transaction().await?;
                    db::call_lock(&transaction).await?;
                    transaction
                        .execute(
                            "UPDATE plugin_http_requests SET state='unknown' WHERE id=$1",
                            &[&id],
                        )
                        .await?;
                    transaction.commit().await?;
                    Ok::<_, Error>(())
                })
                .await;
            });
        }
        // If shutdown prevents the marker, the durable expiry still forces
        // physical process fencing before this instance slot can be reused.
    }
}
const STATIC_INSTANCE:&str="SELECT i.id,i.generation,n.id installation_id,n.manifest,d.approval,d.owner_id FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id WHERE d.id=$1 AND d.archived_at IS NULL AND d.spec->>'enabled'='true' AND d.phase='ready' AND n.state='installed' AND p.status='trusted' AND i.phase='ready' AND i.health='alive' AND i.generation=d.observed_generation AND i.last_heartbeat_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint) ORDER BY i.id";

#[allow(clippy::too_many_arguments)] // Host, request clock and incoming headers remain separate trust inputs.
async fn static_page(
    app: Arc<App>,
    peer: IpAddr,
    request: Uuid,
    deployment: Uuid,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    started: tokio::time::Instant,
    framing: Option<FramingPolicy>,
) -> Result<Response> {
    let permit = pool().try_acquire_owned().map_err(|_| Error::exhausted())?;
    if headers.contains_key("range") {
        return Err(Error::invalid("Static byte ranges are not supported"));
    }
    for name in ["accept", "if-none-match"] {
        if headers.get_all(name).iter().count() > 1 {
            return Err(Error::invalid("Duplicate static request header"));
        }
    }
    if headers.get_all("origin").iter().count() > 1 {
        return Err(Error::invalid("Duplicate Origin"));
    }
    let origin = StaticOrigin::for_deployment(&app.config, deployment, None)?;
    if let Some(value) = headers.get("origin") {
        let value = value.to_str().map_err(|_| Error::denied())?;
        if value != origin.origin()
            && value != app.config.text("server", "public_origin")
            && !framing
                .as_ref()
                .is_some_and(|policy| policy.parent == value)
            && !app.config.value["ui_gateway"]["shell_origins"]
                .as_array()
                .is_some_and(|origins| origins.iter().any(|origin| origin == value))
        {
            return Err(Error::denied());
        }
    }
    let binding = tokio::time::timeout_at(started + Duration::from_secs(10), async {
        let mut connection = app.db.connect_scoped(Duration::from_secs(10)).await?;
        let transaction = connection.transaction().await?;
        db::call_lock(&transaction).await?;
        let rows = transaction.query(STATIC_INSTANCE, &[&deployment]).await?;
        if rows.len() != 1 {
            return Err(Error::unavailable(
                "A fixed healthy static instance is unavailable",
            ));
        }
        let row = &rows[0];
        let instance:Uuid=row.get("id");
        transaction.query_one("SELECT id FROM instances WHERE id=$1 FOR UPDATE",&[&instance]).await?;
        let installation: Uuid = row.get("installation_id");
        let registry = crate::http_contracts::load(&transaction, installation).await?;
        let route = registry.resolve_static(method, path)?.clone();
        let manifest: Value = row.get("manifest");
        check_approval(&transaction, &app, deployment, &manifest).await?;
        let (ctx, paths) = if route.public {
            (None, Vec::new())
        } else {
            request_security(&app, peer, request, method, headers).await?;
            let mut ctx = app
                .auth
                .authenticate(&transaction, api::bearer(headers)?, peer, request)
                .await?;
            ctx.browser_origin=crate::oauth_http::request_origin(headers)?.map(str::to_owned);
            let resource = Resource::new("plugin", string(&manifest, "plugin_id")?, Some(row.get("owner_id")));
            auth::authorize(&transaction, &ctx, &route.action, &resource).await?;
            let paths = Policy::for_context(&transaction, &ctx)
                .credential(ctx.credential, 0)
                .await?
                .into_iter()
                .filter(|path| {
                    auth::rules_allow(&json!([path.rule]), &route.action, &resource, peer)
                })
                .collect();
            (Some(ctx), paths)
        };
        let deadline_at=started+route.timeout;
        ensure_deadline(deadline_at)?;
        let active:i64=transaction.query_one("SELECT (SELECT count(*) FROM calls c WHERE provider_instance_id=$1 AND (state IN ('accepted','running') OR EXISTS(SELECT 1 FROM reservations r WHERE r.id=c.id AND r.kind='call' AND r.status<>'released')))+(SELECT count(*) FROM plugin_http_requests WHERE instance_id=$1)",&[&instance]).await?.get(0);
        if active>=crate::contracts::unsigned(&manifest["concurrency"]["max_active_requests"])? as i64 {return Err(Error::exhausted());}
        let lease=Uuid::new_v4();
        let expires=chrono::Utc::now()+chrono::Duration::from_std(deadline_at.saturating_duration_since(tokio::time::Instant::now())).map_err(|_|deadline())?;
        transaction.execute("INSERT INTO plugin_http_requests(id,instance_id,generation,expires_at,state,created_at) VALUES($1,$2,$3,$4,'active',now())",&[&lease,&instance,&row.get::<_,i64>("generation"),&expires]).await?;
        let binding = StaticBinding {
            installation,
            deployment,
            instance: row.get("id"),
            generation: row.get("generation"),
            owner:row.get("owner_id"),
            manifest,
            approval: row.get("approval"),
            deadline: deadline_at,
            route,
            ctx,
            paths,
            lease,
        };
        transaction.commit().await?;
        Ok::<_, Error>(binding)
    })
    .await
    .map_err(|_| deadline())??;
    let mut lease = StaticLease::new(app.clone(), binding.lease, binding.deployment, started);
    ensure_deadline(binding.deadline)?;
    let client = InstanceHttp::from_runtime(
        &app.config,
        &app.runtime,
        binding.instance,
        binding.generation,
    )?;
    let fetch = client.static_page(
        &binding.route,
        method,
        path,
        headers,
        &origin,
        binding
            .deadline
            .saturating_duration_since(tokio::time::Instant::now()),
    );
    tokio::pin!(fetch);
    lease.phase("fetch");
    let fetched = {
        // Keep polling the transport and its deadline while the complete
        // authority check waits on PostgreSQL. Awaiting it inside a selected
        // timer branch would suspend TLS/HTTP progress for that entire wait.
        let authority = async {
            let mut monitor = tokio::time::interval(Duration::from_millis(100));
            loop {
                monitor.tick().await;
                lease.phase("authority_monitor");
                if let Err(error) = static_authority(&app, &binding).await {
                    return error;
                }
                lease.phase("fetch");
            }
        };
        tokio::pin!(authority);
        tokio::select! {
            response=&mut fetch => response,
            error=&mut authority => return Err(error),
            _=tokio::time::sleep_until(binding.deadline)=>return Err(deadline()),
        }
    };
    let mut response = match fetched {
        Ok(response) => response,
        Err(error) => {
            lease.phase(if error.code == "DEADLINE_EXCEEDED" {
                "fetch_timeout"
            } else {
                "fetch_error"
            });
            return Err(error);
        }
    };
    lease.phase("lease_complete");
    tokio::time::timeout_at(binding.deadline, lease.complete())
        .await
        .map_err(|_| deadline())??;
    static_authority(&app, &binding).await?;
    if binding.manifest.get("webui").is_some() || binding.manifest.get("ui").is_some() {
        tokio::time::timeout_at(
            binding
                .deadline
                .min(tokio::time::Instant::now() + Duration::from_secs(3)),
            async {
                let mut c = app.db.connect_scoped(Duration::from_secs(3)).await?;
                let tx = c.transaction().await?;
                db::auth_lock(&tx, false).await?;
                if binding.manifest.get("webui").is_some() && framing.is_none() {
                    response.headers.insert(
                        "content-security-policy",
                        crate::webui::csp(&tx, &app, binding.deployment, &binding.manifest)
                            .await?
                            .parse()
                            .map_err(|_| Error::denied())?,
                    );
                } else {
                    let ancestors = if let Some(policy) = framing.as_ref() {
                        let parent = crate::webui::validate_framing_query(
                            &tx,
                            &app,
                            binding.deployment,
                            path,
                            &policy.query,
                        )
                        .await?;
                        if parent != policy.parent {
                            return Err(Error::denied());
                        }
                        vec![parent]
                    } else {
                        crate::webui::frame_ancestors(&tx, &app, binding.deployment).await?
                    };
                    let ancestors = if ancestors.is_empty() {
                        "'none'".into()
                    } else {
                        ancestors.join(" ")
                    };
                    let old = api::header(&response.headers, "content-security-policy")?;
                    let mut directives = old
                        .split(';')
                        .map(str::trim)
                        .filter(|s| !s.is_empty() && !s.starts_with("frame-ancestors "))
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    directives.push(format!("frame-ancestors {ancestors}"));
                    response.headers.insert(
                        "content-security-policy",
                        directives.join("; ").parse().map_err(|_| Error::denied())?,
                    );
                }
                tx.commit().await?;
                Ok::<_, Error>(())
            },
        )
        .await
        .map_err(|_| deadline())??;
    }
    let bytes = Bytes::from(response.bytes);
    let body_app = app.clone();
    let body_binding = binding.clone();
    let stream = futures_util::stream::try_unfold((bytes, 0usize), move |(bytes, offset)| {
        let app = body_app.clone();
        let binding = body_binding.clone();
        async move {
            if offset >= bytes.len() {
                return Ok::<_, Error>(None);
            }
            static_authority(&app, &binding).await?;
            let end = (offset + 65536).min(bytes.len());
            let chunk = bytes.slice(offset..end);
            Ok(Some((chunk, (bytes, end))))
        }
    });
    let mut result = Response::builder()
        .status(response.status)
        .body(Body::from_stream(stream))
        .map_err(|_| Error::unavailable("Cannot construct static response"))?;
    *result.headers_mut() = response.headers;
    result.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request.to_string()).unwrap(),
    );
    Ok(retain(result, permit))
}

/// CORS preflight never selects an arbitrary URL or grants authority. It only
/// admits the exact method/path/headers of one healthy approved signed binding;
/// the actual request independently passes the complete Broker admission.
pub(crate) async fn preflight_route(
    c: &(impl GenericClient + Sync),
    app: &App,
    method: &Method,
    path: &str,
) -> Result<Vec<String>> {
    plugin_http::canonical_path(path)?;
    let plugin = path
        .strip_prefix("/plugins/")
        .and_then(|tail| tail.split_once('/'))
        .map(|(id, _)| id)
        .ok_or_else(Error::not_found)?;
    let rows=c.query("SELECT DISTINCT d.id,d.installation_id,n.manifest FROM deployments d JOIN installations n ON n.id=d.installation_id JOIN publishers p ON p.id=n.publisher_id JOIN instances i ON i.deployment_id=d.id WHERE n.plugin_id=$1 AND n.state='installed' AND p.status='trusted' AND d.spec->>'enabled'='true' AND d.archived_at IS NULL AND d.phase='ready' AND i.phase='ready' AND i.health='alive' AND i.generation=d.observed_generation AND i.last_heartbeat_at>now()-interval '1 millisecond'*((n.manifest->'health'->>'interval_ms')::bigint*(n.manifest->'health'->>'failure_threshold')::bigint+(n.manifest->'health'->>'timeout_ms')::bigint)",&[&plugin]).await?;
    if rows.len() != 1 {
        return Err(Error::unavailable("No unique healthy fixed HTTP binding"));
    }
    let row = &rows[0];
    let deployment = row.get("id");
    check_approval(c, app, deployment, &row.get::<_, Value>("manifest")).await?;
    let registry = crate::http_contracts::load(c, row.get("installation_id")).await?;
    Ok(registry.resolve(method, path)?.cors_request_header_names())
}

async fn static_authority(app: &App, binding: &StaticBinding) -> Result<()> {
    crate::skill_streams::ensure_active(app)?;
    ensure_deadline(binding.deadline)?;
    let remaining = binding
        .deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(Duration::from_secs(3));
    tokio::time::timeout(remaining, async {
        let mut connection = app.db.connect_scoped(remaining).await?;
        let transaction = connection.transaction().await?;
        db::auth_lock(&transaction, false).await?;
        let rows = transaction
            .query(STATIC_INSTANCE, &[&binding.deployment])
            .await?;
        if rows.len() != 1 {
            return Err(Error::unavailable("Static deployment is no longer ready"));
        }
        let row = &rows[0];
        if row.get::<_, Uuid>("id") != binding.instance
            || row.get::<_, i64>("generation") != binding.generation
            || row.get::<_, Uuid>("installation_id") != binding.installation
            || row.get::<_, Value>("manifest") != binding.manifest
            || row.get::<_, Value>("approval") != binding.approval
            || row.get::<_, Uuid>("owner_id") != binding.owner
        {
            return Err(Error::conflict("Static deployment binding changed"));
        }
        check_approval(&transaction, app, binding.deployment, &binding.manifest).await?;
        if let Some(ctx) = &binding.ctx {
            let mut current = app
                .auth
                .resume(&transaction, ctx.credential, ctx.peer, ctx.request)
                .await?;
            current.browser_origin = ctx.browser_origin.clone();
            if current.subject != ctx.subject {
                return Err(Error::credential());
            }
            let resource = Resource::new(
                "plugin",
                string(&binding.manifest, "plugin_id")?,
                Some(binding.owner),
            );
            let paths = Policy::for_context(&transaction, &current)
                .revalidate(&binding.paths, current.credential)
                .await?;
            if !auth::rules_allow(
                &policy::rules(&paths),
                &binding.route.action,
                &resource,
                ctx.peer,
            ) {
                return Err(Error::denied());
            }
            auth::authorize(&transaction, &current, &binding.route.action, &resource).await?;
        }
        transaction.commit().await?;
        Ok::<_, Error>(())
    })
    .await
    .map_err(|_| deadline())??;
    ensure_deadline(binding.deadline)
}
fn ensure_deadline(deadline_at: tokio::time::Instant) -> Result<()> {
    if deadline_at <= tokio::time::Instant::now() {
        Err(deadline())
    } else {
        Ok(())
    }
}
fn deadline() -> Error {
    Error::new(
        "DEADLINE_EXCEEDED",
        "HTTP gateway deadline exceeded",
        StatusCode::GATEWAY_TIMEOUT,
    )
}

#[cfg(test)]
mod tests;
