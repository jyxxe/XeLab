//! Actual PostgreSQL and production App.router OAuth/CORS/catalog execution.
//! Installation/deployment rows are explicit metadata fixtures, not OCI proof.
use super::*;
use crate::{api::App, config::Config, db::Db, runtime::Runtime};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    response::Response,
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

const UI: &str = "https://catalog-ui.example.org";
const PASSWORD: &str = "actual catalog test account password";
const VERIFIER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
struct Fixture {
    app: Arc<App>,
    owner: Uuid,
    session: String,
    _root: tempfile::TempDir,
}
#[derive(Clone)]
struct Objects {
    installation: Uuid,
    deployment: Uuid,
    instance: Uuid,
    plugin: String,
}
impl Fixture {
    async fn new() -> Option<Self> {
        crate::init_crypto();
        let base = std::env::var("XELAB_TEST_DATABASE_URL").ok()?;
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("catalog_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let mut value = serde_json::to_value(t).unwrap();
        value["storage"]["root_path"] = json!(root.path());
        let config = Config::from_value(value).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(
            &tx,
            "Actual catalog Owner",
            PASSWORD,
            &config.value["quota_defaults"],
        )
        .await
        .unwrap();
        crate::admission::initialize(&tx, &config.value["resources"], 8000, 34359738368)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let auth = Auth::new(vec![81; 32]).unwrap();
        let login = auth
            .login(
                &c,
                owner,
                PASSWORD.into(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let session = login["access_token"].as_str().unwrap().to_owned();
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            leader: Arc::new(db.leader().await.unwrap()),
            db,
            auth,
            ready: true.into(),
            shutdown: false.into(),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        Some(Self {
            app,
            owner,
            session,
            _root: root,
        })
    }
    fn request(&self, method: &str, path: &str) -> axum::http::request::Builder {
        let url = self.app.config.origin().unwrap();
        Request::builder().method(method).uri(path).header(
            "host",
            &url[url::Position::BeforeHost..url::Position::AfterPort],
        )
    }
    async fn call(&self, mut request: Request<Body>) -> Response {
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:49152".parse::<SocketAddr>().unwrap(),
        ));
        crate::api::router(self.app.clone())
            .oneshot(request)
            .await
            .unwrap()
    }
    async fn manage(
        &self,
        name: &str,
        params: &[(&str, String)],
        token: Option<&str>,
        body: Value,
        revision: Option<i64>,
    ) -> Value {
        let mut selected = None;
        for (path, verbs) in crate::contracts::openapi()["paths"].as_object().unwrap() {
            for (method, op) in verbs.as_object().unwrap() {
                if op["operationId"] == name {
                    selected = Some((path.clone(), method.to_uppercase()));
                }
            }
        }
        let (mut path, method) = selected.unwrap();
        for (key, value) in params {
            path = path.replace(&format!("{{{key}}}"), value);
        }
        let mut builder = self
            .request(&method, &path)
            .header("idempotency-key", Uuid::new_v4().to_string());
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        if let Some(rev) = revision {
            builder = builder.header("if-match", format!("\"rev-{rev}\""));
        }
        let bytes = if body.is_null() {
            vec![]
        } else {
            builder = builder.header("content-type", "application/json");
            serde_json::to_vec(&body).unwrap()
        };
        let response = self.call(builder.body(Body::from(bytes)).unwrap()).await;
        assert!(
            response.status().is_success(),
            "Actual public {name} must succeed: {}",
            response.status()
        );
        value(response).await
    }
    async fn publisher(&self, name: &str) -> Uuid {
        let key = SigningKey::from_bytes(&[if name == "public" { 12 } else { 13 }; 32]);
        let response=self.manage("trustPublisher",&[],Some(&self.session),json!({"name":name,"key_id":format!("catalog-{name}"),"public_key_pem":key.verifying_key().to_public_key_pem(Default::default()).unwrap()}),None).await;
        response["id"].as_str().unwrap().parse().unwrap()
    }
    async fn objects(&self, owner: Uuid, publisher: Uuid, plugin: &str, name: &str) -> Objects {
        let c = self.app.db.connect().await.unwrap();
        let installation = Uuid::new_v4();
        let deployment = Uuid::new_v4();
        let instance = Uuid::new_v4();
        let mut manifest: Value =
            serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
        manifest["plugin_id"] = json!(plugin);
        manifest["publisher_id"] = json!(publisher);
        manifest["requested_rules"] = json!([]);
        crate::artifact::validate_profile(&manifest).unwrap();
        let hash = digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,'1.0.0',$3,$4,$4,$5,'installed',now(),now())",&[&installation,&plugin,&publisher,&hash,&manifest]).await.unwrap();
        let spec = json!({"installation_id":installation,"display_name":name,"enabled":false,"replicas":1,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'disabled',now(),now())",&[&deployment,&owner,&installation,&spec]).await.unwrap();
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'stopped','unknown',$3,now(),now())",&[&instance,&deployment,&spec["limits"]]).await.unwrap();
        Objects {
            installation,
            deployment,
            instance,
            plugin: plugin.into(),
        }
    }
    async fn client(&self, rules: Value, name: &str) -> Value {
        self.manage("createClient",&[],Some(&self.session),json!({"name":name,"plugin_id":"org.xelab.webui","redirect_uris":[format!("{UI}/callback")],"allowed_origins":[UI],"maximum_rules":rules}),None).await
    }
    async fn oauth(&self, session: &str, client: &Value) -> String {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()));
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("response_type", "code"),
                ("client_id", client["client_id"].as_str().unwrap()),
                ("redirect_uri", &format!("{UI}/callback")),
                ("state", "catalog-test-state"),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                (
                    "scope",
                    &format!("client:{}", client["client_id"].as_str().unwrap()),
                ),
            ])
            .finish();
        let cookie = format!("__Host-xelab_auth={session}");
        let response = self
            .call(
                self.request("GET", &format!("/oauth/authorize?{query}"))
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), 302);
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        let consent = self
            .call(
                self.request("GET", &location)
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(consent.status(), 200);
        let html = String::from_utf8(to_bytes(consent.into_body(), 65536).await.unwrap().to_vec())
            .unwrap();
        let fields = [
            (
                "authorization_request_id",
                field(&html, "authorization_request_id"),
            ),
            ("approved", "true".into()),
            ("X-CSRF-Token", field(&html, "X-CSRF-Token")),
            ("Idempotency-Key", field(&html, "Idempotency-Key")),
        ];
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().map(|(key, value)| (*key, value.as_str())))
            .finish();
        let decision = self
            .call(
                self.request("POST", "/api/platform/v1/auth/consents")
                    .header("cookie", &cookie)
                    .header("origin", self.app.config.text("server", "public_origin"))
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(decision.status(), 302);
        let callback = url::Url::parse(decision.headers()["location"].to_str().unwrap()).unwrap();
        let code = callback
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("grant_type", "authorization_code"),
                ("client_id", client["client_id"].as_str().unwrap()),
                ("redirect_uri", &format!("{UI}/callback")),
                ("code", &code),
                ("code_verifier", VERIFIER),
            ])
            .finish();
        let response = self
            .call(
                self.request("POST", "/oauth/token")
                    .header("origin", UI)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["access-control-allow-origin"], UI);
        assert!(response.headers().get("set-cookie").is_none());
        value(response).await["access_token"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    async fn list(&self, name: &str, token: &str, origin: Option<&str>, query: &str) -> Response {
        let path = match name {
            "listPublishers" => "publishers",
            "listInstallations" => "installations",
            "listDeployments" => "deployments",
            "listInstances" => "instances",
            _ => panic!("Known catalog"),
        };
        let path = format!(
            "/api/platform/v1/{path}{}",
            if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            }
        );
        let mut req = self
            .request("GET", &path)
            .header("authorization", format!("Bearer {token}"));
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        self.call(req.body(Body::empty()).unwrap()).await
    }
    async fn child(&self) -> (Uuid, String) {
        let child=self.manage("createChildAccount",&[("account_id",self.owner.to_string())],Some(&self.session),json!({"display_name":"Catalog limited child","kind":"user","inheritance":{"mode":"follow_parent"}}),None).await;
        let id: Uuid = child["account"]["id"].as_str().unwrap().parse().unwrap();
        self.manage(
            "activateAccount",
            &[],
            None,
            json!({"activation_token":child["activation_token"],"password":PASSWORD}),
            None,
        )
        .await;
        let login = self
            .manage(
                "login",
                &[],
                None,
                json!({"account_id":id,"password":PASSWORD}),
                None,
            )
            .await;
        (id, login["access_token"].as_str().unwrap().to_owned())
    }
    async fn inheritance(&self, child: Uuid, rules: Value) {
        let account = self
            .manage(
                "getAccount",
                &[("account_id", child.to_string())],
                Some(&self.session),
                Value::Null,
                None,
            )
            .await;
        self.manage(
            "updateInheritance",
            &[("account_id", child.to_string())],
            Some(&self.session),
            json!({"mode":"selected","rules":rules}),
            account["revision"].as_i64(),
        )
        .await;
    }
}
async fn value(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
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
fn rules(publisher: Uuid, objects: &[Objects]) -> Value {
    json!([
        {"action":"publisher.read","resource":{"kind":"publisher","match":"ids","ids":[publisher]}},
        {"action":"plugin.read","resource":{"kind":"plugin","match":"ids","ids":objects.iter().map(|o|o.plugin.clone()).collect::<Vec<_>>()}},
        {"action":"plugin.read","resource":{"kind":"deployment","match":"ids","ids":objects.iter().map(|o|o.deployment).collect::<Vec<_>>()}},
        {"action":"plugin.read","resource":{"kind":"instance","match":"ids","ids":objects.iter().map(|o|o.instance).collect::<Vec<_>>()}}
    ])
}
fn owned_rules(owner: Uuid, publisher: Uuid, plugins: &[&str]) -> Value {
    json!([
        {"action":"publisher.read","resource":{"kind":"publisher","match":"ids","ids":[publisher]}},
        {"action":"plugin.read","resource":{"kind":"plugin","match":"ids","ids":plugins}},
        {"action":"plugin.read","resource":{"kind":"deployment","match":"owned_by","owner_id":owner}},
        {"action":"plugin.read","resource":{"kind":"instance","match":"owned_by","owner_id":owner}}
    ])
}

#[tokio::test]
async fn main_router_oauth_catalogs_filter_real_owners_and_limited_client_scopes() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let public = f.publisher("public").await;
    let private = f.publisher("private").await;
    let first = f
        .objects(f.owner, public, "org.xelab.webui", "M3 official WebUI")
        .await;
    let second = f
        .objects(
            f.owner,
            public,
            "org.xelab.ui-reference",
            "M3 independent UI reference",
        )
        .await;
    let (child, session) = f.child().await;
    let hidden = f
        .objects(
            child,
            private,
            "org.xelab.catalog-private",
            "Hidden child deployment",
        )
        .await;
    let owner_client = f
        .client(
            rules(public, &[first.clone(), second.clone()]),
            "Limited desktop catalog",
        )
        .await;
    let owner_token = f.oauth(&f.session, &owner_client).await;
    for (name, count, model) in [
        ("listPublishers", 1, "PublisherPage"),
        ("listInstallations", 2, "InstallationPage"),
        ("listDeployments", 2, "DeploymentPage"),
        ("listInstances", 2, "InstancePage"),
    ] {
        let response = f.list(name, &owner_token, Some(UI), "").await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["access-control-allow-origin"], UI);
        let body = value(response).await;
        validate(model, &body).unwrap();
        assert_eq!(body["items"].as_array().unwrap().len(), count);
        assert!(body["next_cursor"].is_null());
        let bytes = serde_json::to_string(&body).unwrap();
        assert!(!bytes.contains(&hidden.deployment.to_string()));
        assert!(!bytes.contains(&hidden.installation.to_string()));
        assert!(!bytes.contains(&hidden.instance.to_string()));
    }
    let list = value(f.list("listDeployments", &owner_token, Some(UI), "").await).await;
    let names: Vec<_> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["spec"]["display_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"M3 official WebUI") && names.contains(&"M3 independent UI reference"));
    assert_eq!(
        f.list(
            "listDeployments",
            &owner_token,
            Some("https://evil.example.org"),
            ""
        )
        .await
        .status(),
        403
    );
    assert_eq!(
        f.list("listDeployments", &f.session, Some(UI), "")
            .await
            .status(),
        403
    );
    let child_rules = owned_rules(child, private, &["org.xelab.catalog-private"]);
    f.inheritance(child, child_rules.clone()).await;
    let client = f
        .client(
            json!([
                {"action":"publisher.read","resource":{"kind":"publisher","match":"all"}},
                {"action":"plugin.read","resource":{"kind":"plugin","match":"all"}},
                {"action":"plugin.read","resource":{"kind":"deployment","match":"all"}},
                {"action":"plugin.read","resource":{"kind":"instance","match":"all"}}
            ]),
            "Child catalog ceiling",
        )
        .await;
    let token = f.oauth(&session, &client).await;
    for name in [
        "listPublishers",
        "listInstallations",
        "listDeployments",
        "listInstances",
    ] {
        let response = f.list(name, &token, Some(UI), "").await;
        assert_eq!(response.status(), 200);
        let body = value(response).await;
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert!(body["next_cursor"].is_null());
        let text = body.to_string();
        assert!(!text.contains(&first.deployment.to_string()));
        assert!(!text.contains(&second.deployment.to_string()));
    }
    let filtered = value(
        f.list(
            "listInstances",
            &token,
            Some(UI),
            &format!("deployment_id={}", first.deployment),
        )
        .await,
    )
    .await;
    assert!(filtered["items"].as_array().unwrap().is_empty());
    assert!(filtered["next_cursor"].is_null());
    let unknown = value(
        f.list(
            "listInstances",
            &token,
            Some(UI),
            &format!("deployment_id={}", Uuid::new_v4()),
        )
        .await,
    )
    .await;
    assert_eq!(
        filtered, unknown,
        "Private filter targets cannot leak existence"
    );
}

#[tokio::test]
async fn main_router_catalog_cursor_binds_origin_authority_filters_and_creation_watermark() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let publisher = f.publisher("public").await;
    let (child, session) = f.child().await;
    let first = f
        .objects(child, publisher, "org.xelab.catalog-child-a", "Child A")
        .await;
    let second = f
        .objects(child, publisher, "org.xelab.catalog-child-b", "Child B")
        .await;
    let hidden = f
        .objects(
            f.owner,
            publisher,
            "org.xelab.catalog-hidden",
            "Root private",
        )
        .await;
    let child_rules = owned_rules(child, publisher, &[&first.plugin, &second.plugin]);
    f.inheritance(child, child_rules.clone()).await;
    let client = f
        .client(
            json!([
                {"action":"plugin.read","resource":{"kind":"deployment","match":"all"}},
                {"action":"plugin.read","resource":{"kind":"instance","match":"all"}},
                {"action":"plugin.read","resource":{"kind":"plugin","match":"all"}}
            ]),
            "Cursor test catalog ceiling",
        )
        .await;
    let token = f.oauth(&session, &client).await;
    let response = f.list("listDeployments", &token, Some(UI), "limit=1").await;
    assert_eq!(response.status(), 200);
    let initial = value(response).await;
    let cursor = initial["next_cursor"].as_str().unwrap();
    let encrypted = URL_SAFE_NO_PAD
        .decode(cursor.strip_prefix("c1.").unwrap())
        .unwrap();
    assert!(!encrypted
        .windows(36)
        .any(|bytes| bytes == hidden.deployment.to_string().as_bytes()));
    let original = initial["items"][0]["id"].as_str().unwrap();
    let remaining = if original == first.deployment.to_string() {
        second.deployment
    } else {
        first.deployment
    };
    let c = f.app.db.connect().await.unwrap();
    c.execute("UPDATE deployments SET phase='degraded',conditions='[\"ObservedFixtureState\"]',revision=revision+1 WHERE id=$1",&[&remaining]).await.unwrap();
    let created = f
        .objects(
            child,
            publisher,
            "org.xelab.catalog-child-new",
            "After watermark",
        )
        .await;
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("limit", "1"), ("cursor", cursor)])
        .finish();
    let next = f.list("listDeployments", &token, Some(UI), &query).await;
    assert_eq!(next.status(), 200);
    let next = value(next).await;
    assert_eq!(next["items"][0]["id"], remaining.to_string());
    assert_eq!(next["items"][0]["phase"], "degraded");
    assert!(next["next_cursor"].is_null());
    assert!(!next.to_string().contains(&created.deployment.to_string()));
    let fresh = value(f.list("listDeployments", &token, Some(UI), "").await).await;
    assert_eq!(fresh["items"].as_array().unwrap().len(), 3);
    assert_eq!(
        f.list("listDeployments", &f.session, None, &query)
            .await
            .status(),
        400
    );
    assert_eq!(
        f.list("listInstances", &token, Some(UI), &query)
            .await
            .status(),
        400
    );
    assert_eq!(
        f.list("listDeployments", &token, None, &query)
            .await
            .status(),
        400
    );
    assert_eq!(
        f.list(
            "listDeployments",
            &token,
            Some(UI),
            &query.replace("limit=1", "limit=2")
        )
        .await
        .status(),
        400
    );
    let mut bad = cursor.as_bytes().to_vec();
    let last = bad.len() - 2;
    bad[last] = if bad[last] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        f.list(
            "listDeployments",
            &token,
            Some(UI),
            &format!("limit=1&cursor={}", String::from_utf8(bad).unwrap())
        )
        .await
        .status(),
        400
    );
    for query in [
        "limit=0",
        "limit=201",
        "limit=1&%6cimit=2",
        "limit=%ff",
        "limit=%zz",
        "unsupported=1",
        "plugin_id=invalid",
        "deployment_id=invalid",
    ] {
        assert_eq!(
            f.list("listDeployments", &token, Some(UI), query)
                .await
                .status(),
            400,
            "{query}"
        );
    }
    for (operation, filter) in [
        ("listInstallations", format!("plugin_id={}", first.plugin)),
        (
            "listInstances",
            format!("deployment_id={}", first.deployment),
        ),
    ] {
        let page = value(f.list(operation, &token, Some(UI), "limit=1").await).await;
        let original = page["next_cursor"].as_str().unwrap();
        let altered = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("limit", "1"), ("cursor", original)])
            .finish()
            + "&"
            + &filter;
        assert_eq!(
            f.list(operation, &token, Some(UI), &altered).await.status(),
            400
        );
    }
    let mut expired = decode(&f.app.auth, cursor).unwrap();
    expired.watermark = Utc::now() - chrono::Duration::minutes(16);
    expired.last_time = expired.watermark;
    expired.expires_at = expired.watermark + chrono::Duration::minutes(15);
    let expired = encode(&f.app.auth, &expired).unwrap();
    assert_eq!(
        f.list(
            "listDeployments",
            &token,
            Some(UI),
            &format!("limit=1&cursor={expired}")
        )
        .await
        .status(),
        400
    );
    // Updating and restoring the same visible ceiling still changes the saved
    // authority version. A new same-scope proof cannot continue the old cursor.
    f.inheritance(child, json!([{
        "action":"plugin.read","resource":{"kind":"deployment","match":"ids","ids":[first.deployment]}
    }])).await;
    f.inheritance(child, child_rules).await;
    assert_eq!(
        f.list("listDeployments", &token, Some(UI), &query)
            .await
            .status(),
        400
    );
}
