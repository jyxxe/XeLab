//! Real PG HTTP components. Main Host/router and sequential schema16 wiring
//! remain separate integration gates; no OCI or production acceptance claim.
use super::*;
use crate::{auth, identity, oauth::clients};
use axum::extract::ConnectInfo;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tower::ServiceExt;

const PASSWORD: &str = "correct horse backup fixture password";
const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Fixture {
    _base: crate::backups::tests::Fixture,
    app: Arc<App>,
    client: Value,
    owner: Uuid,
    session: String,
    ui: String,
}
impl Fixture {
    async fn new(core: &str, ui: &str) -> Option<Self> {
        let base = crate::backups::tests::Fixture::new().await?;
        let mut c = base.app.db.connect().await.unwrap();
        if !oauth::schema_available(&c).await.unwrap() {
            c.batch_execute(include_str!("../migrations/0016_oauth_state.sql"))
                .await
                .unwrap();
        }
        let tx = c.transaction().await.unwrap();
        db::auth_lock(&tx, true).await.unwrap();
        let ctx = base
            .app
            .auth
            .authenticate(
                &tx,
                &base.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let spec = json!({"name":"HTTP <actual> UI","plugin_id":"org.xelab.webui","redirect_uris":[format!("{ui}/callback")],"allowed_origins":[ui],"maximum_rules":[{"action":"platform.read","resource":{"kind":"platform","match":"all"}},{"action":"account.read","resource":{"kind":"account","match":"all"}},{"action":"credential.api-key.create","resource":{"kind":"account","match":"all"}},{"action":"grant.create","resource":{"kind":"account","match":"all"}},{"action":"account.child.create","resource":{"kind":"account","match":"all"}}]});
        clients::authorize(&tx, &ctx, "createClient", &HashMap::new(), &spec)
            .await
            .unwrap();
        let (client, _) = clients::execute(
            &tx,
            &base.app.auth,
            &ctx,
            "createClient",
            &HashMap::new(),
            &spec,
            &HeaderMap::new(),
            "",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut config = base.app.config.clone();
        config.value["server"]["public_origin"] = json!(core);
        let app = Arc::new(App {
            config: config.clone(),
            db: base.app.db.clone(),
            auth: base.app.auth.clone(),
            runtime: crate::runtime::Runtime::new(config),
            leader: base.app.leader.clone(),
            ready: true.into(),
            shutdown: false.into(),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        Some(Self {
            session: base.token.clone(),
            _base: base,
            app,
            client,
            owner: ctx.subject,
            ui: ui.into(),
        })
    }
    fn core(&self) -> &str {
        self.app.config.text("server", "public_origin")
    }
    fn query(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("response_type", "code"),
                ("client_id", self.client["client_id"].as_str().unwrap()),
                ("redirect_uri", &format!("{}/callback", self.ui)),
                ("state", "actual-browser-state"),
                (
                    "code_challenge",
                    &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes())),
                ),
                ("code_challenge_method", "S256"),
                (
                    "scope",
                    &format!("client:{}", self.client["client_id"].as_str().unwrap()),
                ),
            ])
            .finish()
    }
    fn request(&self, method: &str, path: &str) -> axum::http::request::Builder {
        let u = self.app.config.origin().unwrap();
        Request::builder().method(method).uri(path).header(
            "host",
            &u[url::Position::BeforeHost..url::Position::AfterPort],
        )
    }
    async fn call(&self, request: Request<Body>) -> Response {
        handle(
            self.app.clone(),
            "127.0.0.1:9876".parse().unwrap(),
            Uuid::new_v4(),
            request,
        )
        .await
    }
    async fn authorize(&self) -> (String, String, String) {
        let response = self
            .call(
                self.request("GET", &format!("/oauth/authorize?{}", self.query()))
                    .header("cookie", format!("{AUTH_COOKIE}={}", self.session))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), 302);
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        let response = self
            .call(
                self.request("GET", &location)
                    .header("cookie", format!("{AUTH_COOKIE}={}", self.session))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), 200);
        let html = text(response).await;
        (
            field(&html, "authorization_request_id"),
            field(&html, "X-CSRF-Token"),
            field(&html, "Idempotency-Key"),
        )
    }
    fn decision(&self, id: &str, csrf: &str, key: &str) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("authorization_request_id", id),
                ("approved", "true"),
                ("X-CSRF-Token", csrf),
                ("Idempotency-Key", key),
            ])
            .finish()
    }
    async fn submit(&self, body: String) -> Response {
        self.call(
            self.request("POST", CONSENT_PATH)
                .header("cookie", format!("{AUTH_COOKIE}={}", self.session))
                .header("origin", self.core())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }
    async fn token(&self, fields: &[(&str, &str)]) -> Response {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().copied())
            .finish();
        self.call(
            self.request("POST", "/oauth/token")
                .header("origin", &self.ui)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }
    async fn tokens(&self) -> Value {
        let (id, csrf, key) = self.authorize().await;
        let response = self.submit(self.decision(&id, &csrf, &key)).await;
        assert_eq!(response.status(), 302);
        let code = code(&response);
        let public = self.client["client_id"].as_str().unwrap();
        let response = self
            .token(&[
                ("grant_type", "authorization_code"),
                ("client_id", public),
                ("redirect_uri", &format!("{}/callback", self.ui)),
                ("code", &code),
                ("code_verifier", VERIFIER),
            ])
            .await;
        assert_eq!(response.status(), 200);
        value(response).await
    }
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 65536)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn value(response: Response) -> Value {
    serde_json::from_str(&text(response).await).unwrap()
}
fn field(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn code(response: &Response) -> String {
    url::Url::parse(response.headers()["location"].to_str().unwrap())
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "code")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn real_http_login_continuation_native_consent_pkce_and_secret_free_idempotency() {
    let Some(f) = Fixture::new("https://kernel.example.org:8443", "https://ui.example.org").await
    else {
        return;
    };
    let start = f
        .call(
            f.request("GET", &format!("/oauth/authorize?{}", f.query()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(start.status(), 302);
    assert_eq!(start.headers()["location"], "/auth/ui/login");
    let cookies = start
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|c| c.to_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let continuation = cookies
        .iter()
        .find(|c| c.starts_with(CONTINUATION_COOKIE))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert!(cookies
        .iter()
        .all(|c| c.contains("Secure; HttpOnly; SameSite=Lax; Path=/")));
    let page = f
        .call(
            f.request("GET", "/auth/ui/login")
                .header("cookie", continuation)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
    assert!(text(page).await.contains("window.location.replace(resume)"));
    // Existing actual public password endpoint creates the trusted Cookie.
    let mut login = f
        .request("POST", "/api/platform/v1/auth/login")
        .header("origin", f.core())
        .header("content-type", "application/json")
        .body(Body::from(
            json!({"account_id":f.owner,"password":PASSWORD}).to_string(),
        ))
        .unwrap();
    login
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:9876".parse::<SocketAddr>().unwrap()));
    let logged = crate::api::router(f.app.clone())
        .oneshot(login)
        .await
        .unwrap();
    assert_eq!(logged.status(), 200);
    let session = logged.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let both = format!("{session}; {continuation}");
    let resumed = f
        .call(
            f.request("GET", "/auth/ui/login")
                .header("cookie", &both)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(resumed.status(), 302);
    assert!(resumed.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .contains("Max-Age=0"));
    let location = resumed.headers()["location"].to_str().unwrap();
    let page = f
        .call(
            f.request("GET", location)
                .header("cookie", session)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(page.status(), 200);
    let html = text(page).await;
    assert!(html.contains("HTTP &lt;actual&gt; UI"));
    let id = field(&html, "authorization_request_id");
    let csrf = field(&html, "X-CSRF-Token");
    let key = field(&html, "Idempotency-Key");
    let body = f.decision(&id, &csrf, &key);
    let post = || {
        f.request("POST", CONSENT_PATH)
            .header("origin", f.core())
            .header("cookie", session)
            .header("content-type", "application/x-www-form-urlencoded")
    };
    let denied = f
        .call(
            post()
                .body(Body::from(body.replace(&csrf, &"a".repeat(43))))
                .unwrap(),
        )
        .await;
    assert_eq!(denied.status(), 403);
    let approved = f.call(post().body(Body::from(body.clone())).unwrap()).await;
    assert_eq!(approved.status(), 302);
    assert!(!approved.headers().contains_key("set-cookie"));
    let rawcode = code(&approved);
    let repeated = f.call(post().body(Body::from(body.clone())).unwrap()).await;
    assert_eq!(repeated.status(), 409);
    assert_eq!(
        value(repeated).await["error"]["code"],
        "ONE_TIME_RESPONSE_UNAVAILABLE"
    );
    let changed = f
        .call(
            post()
                .body(Body::from(body.replace("approved=true", "approved=false")))
                .unwrap(),
        )
        .await;
    assert_eq!(changed.status(), 409);
    assert_eq!(
        value(changed).await["error"]["code"],
        "IDEMPOTENCY_CONFLICT"
    );
    let c = f.app.db.connect().await.unwrap();
    let row=c.query_one("SELECT safe_response,sensitive,resource_id,row_to_json(i)::text AS persisted FROM idempotency_records i WHERE path=$1",&[&CONSENT_PATH]).await.unwrap();
    assert!(row.get::<_, Option<Value>>("safe_response").is_none());
    assert!(row.get::<_, bool>("sensitive"));
    assert_eq!(row.get::<_, String>("resource_id"), id);
    assert!(!row.get::<_, String>("persisted").contains(&rawcode));
    let public = f.client["client_id"].as_str().unwrap();
    let callback = format!("{}/callback", f.ui);
    let response = f
        .token(&[
            ("grant_type", "authorization_code"),
            ("client_id", public),
            ("redirect_uri", &callback),
            ("code", &rawcode),
            ("code_verifier", VERIFIER),
        ])
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["access-control-allow-origin"], f.ui);
    assert!(!response
        .headers()
        .contains_key("access-control-allow-credentials"));
    assert!(!response.headers().contains_key("set-cookie"));
    let token = value(response).await;
    validate("OAuthToken", &token).unwrap();
    let replay = f
        .token(&[
            ("grant_type", "authorization_code"),
            ("client_id", public),
            ("redirect_uri", &callback),
            ("code", &rawcode),
            ("code_verifier", VERIFIER),
        ])
        .await;
    assert_eq!(replay.status(), 400);
    let error = value(replay).await;
    validate("OAuthError", &error).unwrap();
    assert_eq!(error["error"], "invalid_grant");
}

#[tokio::test]
async fn real_http_cors_requires_direct_client_bound_access_and_rechecks_refresh_reuse() {
    let Some(f) = Fixture::new("https://kernel.example.org:8443", "https://ui.example.org").await
    else {
        return;
    };
    let preflight = f
        .call(
            f.request("OPTIONS", "/api/platform/v1/accounts/me")
                .header("origin", &f.ui)
                .header("access-control-request-method", "GET")
                .header(
                    "access-control-request-headers",
                    "authorization, content-type",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(preflight.status(), 204);
    assert_eq!(preflight.headers()["access-control-allow-origin"], f.ui);
    assert!(!preflight
        .headers()
        .contains_key("access-control-allow-credentials"));
    for (path, method, headers) in [
        ("/api/platform/v1/auth/login", "POST", "content-type"),
        ("/oauth/token", "GET", "authorization"),
        ("/oauth/token", "POST", "x-user-id"),
        ("/api/platform/v1/unknown", "GET", "authorization"),
    ] {
        assert!(!f
            .call(
                f.request("OPTIONS", path)
                    .header("origin", &f.ui)
                    .header("access-control-request-method", method)
                    .header("access-control-request-headers", headers)
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .status()
            .is_success());
    }
    let tokens = f.tokens().await;
    let c = f.app.db.connect().await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("origin", f.ui.parse().unwrap());
    headers.insert(
        "authorization",
        format!("Bearer {}", tokens["access_token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let ctx = credential_context(
        &c,
        &f.app.auth,
        &headers,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core(),
    )
    .await
    .unwrap();
    auth::authorize(
        &c,
        &ctx,
        "account.read",
        &auth::Resource::new("account", f.owner, Some(f.owner)),
    )
    .await
    .unwrap();
    let mut wrong = headers.clone();
    wrong.insert("origin", "https://other.example.org".parse().unwrap());
    assert!(credential_context(
        &c,
        &f.app.auth,
        &wrong,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core()
    )
    .await
    .is_err());
    let mut raw = headers.clone();
    raw.insert(
        "authorization",
        format!("Bearer {}", f.session).parse().unwrap(),
    );
    assert!(credential_context(
        &c,
        &f.app.auth,
        &raw,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core()
    )
    .await
    .is_err());
    let mut mixed = headers.clone();
    mixed.insert(
        "cookie",
        format!("{AUTH_COOKIE}={}", f.session).parse().unwrap(),
    );
    assert!(credential_context(
        &c,
        &f.app.auth,
        &mixed,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core()
    )
    .await
    .is_err());
    let mut forged_cookie = HeaderMap::new();
    forged_cookie.insert("origin", f.core().parse().unwrap());
    forged_cookie.insert(
        "cookie",
        format!("{AUTH_COOKIE}={}", tokens["access_token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    assert!(
        credential_context(
            &c,
            &f.app.auth,
            &forged_cookie,
            "127.0.0.1:9876".parse().unwrap(),
            Uuid::new_v4(),
            f.core()
        )
        .await
        .is_err(),
        "an OAuth token cannot be promoted to the trusted Cookie transport"
    );
    let public = f.client["client_id"].as_str().unwrap();
    let refresh = tokens["refresh_token"].as_str().unwrap();
    let rotated = f
        .token(&[
            ("grant_type", "refresh_token"),
            ("client_id", public),
            ("refresh_token", refresh),
        ])
        .await;
    assert_eq!(rotated.status(), 200);
    let next = value(rotated).await;
    let reused = f
        .token(&[
            ("grant_type", "refresh_token"),
            ("client_id", public),
            ("refresh_token", refresh),
        ])
        .await;
    assert_eq!(reused.status(), 400);
    assert_eq!(value(reused).await["error"], "invalid_grant");
    headers.insert(
        "authorization",
        format!("Bearer {}", next["access_token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    assert!(credential_context(
        &c,
        &f.app.auth,
        &headers,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core()
    )
    .await
    .is_err());
}

#[tokio::test]
async fn real_http_trusted_host_session_encoding_and_origin_fail_closed() {
    let Some(f) = Fixture::new("https://kernel.example.org:8443", "https://ui.example.org").await
    else {
        return;
    };
    let h2 = f
        .call(
            Request::builder()
                .version(axum::http::Version::HTTP_2)
                .uri(format!("{}/auth/ui/login", f.core()))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(h2.status(), 200);
    for request in [
        f.request("GET", "/auth/ui/login?next=https://evil.example/")
            .body(Body::empty())
            .unwrap(),
        f.request("GET", "/auth/ui/login")
            .header("authorization", format!("Bearer {}", f.session))
            .body(Body::empty())
            .unwrap(),
        f.request("GET", "/auth/ui/login")
            .header("origin", &f.ui)
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/auth/ui/login")
            .header("host", "ui.example.org")
            .body(Body::empty())
            .unwrap(),
        f.request("GET", "/auth/ui/consent?authorization_request_id=%FF")
            .body(Body::empty())
            .unwrap(),
        f.request("GET", "/auth/ui/login")
            .body(Body::from("body"))
            .unwrap(),
        f.request("GET", "/auth/ui/login")
            .header(
                "cookie",
                format!("{AUTH_COOKIE}={}; {AUTH_COOKIE}={}", f.session, f.session),
            )
            .header(
                "cookie",
                format!("{CONTINUATION_COOKIE}={}", "a".repeat(43)),
            )
            .body(Body::empty())
            .unwrap(),
    ] {
        assert!(!f.call(request).await.status().is_success());
    }
    let unsupported = f
        .call(
            f.request("POST", "/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=password&client_id=client&password=must-not-use-password",
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(unsupported.status(), 400);
    assert_eq!(value(unsupported).await["error"], "unsupported_grant_type");
    let malformed = f
        .call(
            f.request("POST", "/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=refresh_token&client_id=client&refresh_token=%FF",
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(malformed.status(), 400);
    validate("OAuthError", &value(malformed).await).unwrap();
}

#[tokio::test]
async fn real_origin_provenance_filters_grant_alternatives_and_all_derived_authority() {
    let Some(f) = Fixture::new("https://kernel.example.org:8443", "https://ui.example.org").await
    else {
        return;
    };
    let token = f.tokens().await;
    let mut c = f.app.db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("origin", f.ui.parse().unwrap());
    headers.insert(
        "authorization",
        format!("Bearer {}", token["access_token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let actor = credential_context(
        &tx,
        &f.app.auth,
        &headers,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core(),
    )
    .await
    .unwrap();
    let guest = Uuid::new_v4();
    let password = "origin-provenance guest correct horse password";
    tx.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Origin Guest',$2,now(),now())",&[&guest,&auth::password_hash(password).unwrap()]).await.unwrap();
    tx.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&guest],
    )
    .await
    .unwrap();
    let target = HashMap::from([("account_id".into(), guest.to_string())]);
    let root = f
        .app
        .auth
        .authenticate(&tx, &f.session, actor.peer, Uuid::new_v4())
        .await
        .unwrap();
    let read = json!({"action":"platform.read","resource":{"kind":"platform","match":"all"},"conditions":{"audiences":["xelab.platform"]}});
    let (group, _) = identity::execute(
        &tx,
        &f.app.auth,
        &root,
        "createGroup",
        &HashMap::new(),
        &json!({"name":"Origin fixed read","rules":[read]}),
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let grant = json!({"group_version":{"group_id":group["id"],"version":1},"scope":{"kind":"platform","match":"all"}});
    identity::authorize(&tx, &actor, "createGrant", &target, &grant)
        .await
        .unwrap();
    let (issued, _) = identity::execute(
        &tx,
        &f.app.auth,
        &actor,
        "createGrant",
        &target,
        &grant,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    assert_eq!(
        tx.query_one(
            "SELECT issuer_credential_id FROM grants WHERE id=$1",
            &[&contracts::id(&issued, "id").unwrap()]
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0),
        actor.credential
    );
    let logged = f
        .app
        .auth
        .login(&tx, guest, password.into(), actor.peer, Uuid::new_v4())
        .await
        .unwrap();
    let mut guest_ctx = f
        .app
        .auth
        .authenticate(
            &tx,
            logged["access_token"].as_str().unwrap(),
            actor.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let resource = auth::Resource::new("platform", "node", None);
    for (browser, allowed) in [
        (Some(f.ui.as_str()), true),
        (Some("https://other.example.org"), false),
        (Some(f.core()), false),
        (None, true),
    ] {
        guest_ctx.browser_origin = browser.map(str::to_owned);
        assert_eq!(
            auth::authorize(&tx, &guest_ctx, "platform.read", &resource)
                .await
                .is_ok(),
            allowed,
            "OAuth-backed grant must retain its origin restriction"
        );
    }
    let owner_params = HashMap::from([("account_id".into(), f.owner.to_string())]);
    let read = json!({"action":"platform.read","resource":{"kind":"platform","match":"all"}});
    let key_request = json!({"name":"origin derived key","inheritance":{"mode":"selected","rules":[read]},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::seconds(300)}});
    let (key, _) = identity::execute(
        &tx,
        &f.app.auth,
        &actor,
        "createApiKey",
        &owner_params,
        &key_request,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let child_request = json!({"kind":"service","display_name":"Origin child","inheritance":{"mode":"selected","rules":[read]}});
    identity::authorize(
        &tx,
        &actor,
        "createChildAccount",
        &owner_params,
        &child_request,
    )
    .await
    .unwrap();
    let (child, _) = identity::execute(
        &tx,
        &f.app.auth,
        &actor,
        "createChildAccount",
        &owner_params,
        &child_request,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let child_id = contracts::id(&child["account"], "id").unwrap();
    assert_eq!(
        tx.query_one(
            "SELECT issuer_credential_id FROM inheritance_grants WHERE account_id=$1",
            &[&child_id]
        )
        .await
        .unwrap()
        .get::<_, Uuid>(0),
        actor.credential
    );
    let (child_key, _) = identity::execute(
        &tx,
        &f.app.auth,
        &actor,
        "createApiKey",
        &HashMap::from([("account_id".into(), child_id.to_string())]),
        &key_request,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    for derived in [&key, &child_key] {
        let mut derived_headers = HeaderMap::new();
        derived_headers.insert(
            "authorization",
            format!("Bearer {}", derived["secret"].as_str().unwrap())
                .parse()
                .unwrap(),
        );
        let cli = credential_context(
            &tx,
            &f.app.auth,
            &derived_headers,
            "127.0.0.1:9876".parse().unwrap(),
            Uuid::new_v4(),
            f.core(),
        )
        .await
        .unwrap();
        assert!(cli.chain.contains(&actor.credential));
        auth::authorize(&tx, &cli, "platform.read", &resource)
            .await
            .unwrap();
        for browser in [f.core(), "https://other.example.org", &f.ui] {
            derived_headers.insert("origin", browser.parse().unwrap());
            let admitted = credential_context(
                &tx,
                &f.app.auth,
                &derived_headers,
                "127.0.0.1:9876".parse().unwrap(),
                Uuid::new_v4(),
                f.core(),
            )
            .await;
            if browser == f.core() {
                let bounded = admitted.unwrap();
                assert!(
                    auth::authorize(&tx, &bounded, "platform.read", &resource)
                        .await
                        .is_err(),
                    "derived credential authentication cannot restore origin-filtered authority"
                );
            } else {
                assert!(
                    admitted.is_err(),
                    "foreign origins require direct OAuth access Bearer"
                );
            }
        }
    }
    // A child user authenticates with its own password. Its fixed own-profile
    // source remains available at the trusted core while inherited A business
    // authority is filtered. Authentication must not reject that whole session.
    let mut user_request = child_request.clone();
    user_request["kind"] = json!("user");
    let (user, _) = identity::execute(
        &tx,
        &f.app.auth,
        &actor,
        "createChildAccount",
        &owner_params,
        &user_request,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    let user_id = contracts::id(&user["account"], "id").unwrap();
    identity::activate(&tx,&f.app.auth,&json!({"account_id":user_id,"activation_token":user["activation_token"],"password":"origin child fresh password"}),actor.peer,Uuid::new_v4()).await.unwrap();
    let activated = f
        .app
        .auth
        .login(
            &tx,
            user_id,
            "origin child fresh password".into(),
            actor.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let mut user_headers = HeaderMap::new();
    user_headers.insert("origin", f.core().parse().unwrap());
    user_headers.insert(
        "cookie",
        format!(
            "{AUTH_COOKIE}={}",
            activated["access_token"].as_str().unwrap()
        )
        .parse()
        .unwrap(),
    );
    let user_ctx = credential_context(
        &tx,
        &f.app.auth,
        &user_headers,
        "127.0.0.1:9876".parse().unwrap(),
        Uuid::new_v4(),
        f.core(),
    )
    .await
    .unwrap();
    auth::authorize(
        &tx,
        &user_ctx,
        "account.profile.update",
        &auth::Resource::new("account", user_id, Some(user_id)),
    )
    .await
    .unwrap();
    assert!(auth::authorize(&tx, &user_ctx, "platform.read", &resource)
        .await
        .is_err());
    // Trusted consent evaluates at the validated target Client origin, even
    // though this session is using the core's CSRF-protected consent page.
    guest_ctx.browser_origin = Some(f.core().into());
    let mut input = authorization::parse_query(&f.query()).unwrap();
    let matching = authorization::begin_authorization(&tx, &f.app.auth, &guest_ctx, &input)
        .await
        .unwrap();
    assert!(matching
        .rules
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| rule["action"] == "platform.read"));
    let root = f
        .app
        .auth
        .authenticate(&tx, &f.session, actor.peer, Uuid::new_v4())
        .await
        .unwrap();
    let mut other_spec = f.client["spec"].clone();
    other_spec["allowed_origins"] = json!(["https://other.example.org"]);
    other_spec["redirect_uris"] = json!(["https://other.example.org/callback"]);
    let (other, _) = clients::execute(
        &tx,
        &f.app.auth,
        &root,
        "createClient",
        &HashMap::new(),
        &other_spec,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    input.client_id = other["client_id"].as_str().unwrap().into();
    input.redirect_uri = "https://other.example.org/callback".into();
    input.scope = format!("client:{}", input.client_id);
    let mismatched = authorization::begin_authorization(&tx, &f.app.auth, &guest_ctx, &input)
        .await
        .unwrap();
    assert!(
        !mismatched
            .rules
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["action"] == "platform.read"),
        "consent must not launder A's grant into B"
    );
    // A separately issued ordinary grant remains a valid alternative, so a
    // mismatched OAuth grant cannot lock out independently held authority.
    identity::execute(
        &tx,
        &f.app.auth,
        &root,
        "createGrant",
        &target,
        &grant,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap();
    guest_ctx.browser_origin = Some(f.core().into());
    auth::authorize(&tx, &guest_ctx, "platform.read", &resource)
        .await
        .unwrap();
    let proof = identity::call_sources_for(&tx, &guest_ctx, "platform.read", &resource)
        .await
        .unwrap();
    assert!(
        !proof.contains(&actor.credential),
        "origin-filtered funding cannot choose the withdrawn alternative"
    );
    tx.commit().await.unwrap();
}

#[path = "oauth_browser_tests.rs"]
mod browser;
