//! Actual PostgreSQL/OAuth/router and independent HTTPS execution. Installation
//! and Ready observations are fixtures; this does not certify signed OCI.
use super::*;
use crate::oauth::{authorization, tokens};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

struct FramingFixture {
    ui: UiFixture,
    shell: Uuid,
    signed: Value,
    origin: String,
    client: Value,
}
impl FramingFixture {
    async fn new() -> Option<Self> {
        let ui = UiFixture::new().await?;
        let c = ui.app.db.connect().await.unwrap();
        let signed: Value = serde_json::from_str(include_str!(
            "../../../docs/preset-plugins/webui/manifest.json"
        ))
        .unwrap();
        let installation = Uuid::new_v4();
        let shell = Uuid::new_v4();
        let origin = StaticOrigin::for_deployment(&ui.app.config, shell, None)
            .unwrap()
            .origin()
            .to_owned();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())", &[&installation,&string(&signed,"plugin_id").unwrap(),&string(&signed,"version").unwrap(),&ui.publisher,&crate::contracts::digest(b"framing shell metadata image"),&crate::contracts::digest(&serde_json::to_vec(&signed).unwrap()),&signed]).await.unwrap();
        crate::http_contracts::store(&c, installation, &signed, None)
            .await
            .unwrap();
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,observed_generation,approval,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":true,\"config\":{}}','ready',1,'{}',now(),now())", &[&shell,&ui.owner.subject,&installation]).await.unwrap();
        crate::plugin_http_gateway::approve(
            &c,
            &ui.owner,
            shell,
            &signed,
            &json!({}),
            ui.owner.subject,
        )
        .await
        .unwrap();
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())", &[&Uuid::new_v4(),&shell]).await.unwrap();
        let rules = json!([{"action":"plugin.ui.open","resource":{"kind":"plugin","match":"all"}}]);
        let (client,_) = crate::oauth::clients::execute(&c,&ui.app.auth,&ui.owner,"createClient",&HashMap::new(),&json!({"name":"Framing policy client","plugin_id":"org.xelab.webui","allowed_origins":[origin],"redirect_uris":[format!("{origin}/ui/org.xelab.webui/callback")],"maximum_rules":rules}),&HeaderMap::new(),"").await.unwrap();
        c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config}',jsonb_build_object('client_id',$2::text)) WHERE id=$1", &[&shell,&client["client_id"].as_str().unwrap()]).await.unwrap();
        Some(Self {
            ui,
            shell,
            signed,
            origin,
            client,
        })
    }
    async fn observe(&self) {
        self.ui
            .app
            .db
            .connect()
            .await
            .unwrap()
            .execute(
                "UPDATE instances SET last_heartbeat_at=now() WHERE deployment_id IN($1,$2)",
                &[&self.shell, &self.ui.deployment],
            )
            .await
            .unwrap();
    }
    async fn token(&self) -> Value {
        self.token_for(&self.ui.owner).await
    }
    async fn token_for(&self, session: &Context) -> Value {
        let verifier = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let input = authorization::AuthorizationInput {
            client_id: self.client["client_id"].as_str().unwrap().into(),
            redirect_uri: format!("{}/ui/org.xelab.webui/callback", self.origin),
            state: "exact-component-browser".into(),
            code_challenge: URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)),
            scope: format!("client:{}", self.client["client_id"].as_str().unwrap()),
        };
        let mut c = self.ui.app.db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let view = authorization::begin_authorization(&tx, &self.ui.app.auth, session, &input)
            .await
            .unwrap();
        let authorization::ConsentOutcome::Approved { location } = authorization::decide_consent(
            &tx,
            &self.ui.app.auth,
            session,
            view.authorization_request_id,
            true,
            &view.csrf_token,
        )
        .await
        .unwrap() else {
            panic!("Actual consent must approve");
        };
        let code = url::Url::parse(&location)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "code")
            .unwrap()
            .1
            .into_owned();
        tx.commit().await.unwrap();
        self.exchange(json!({"grant_type":"authorization_code","client_id":input.client_id,"redirect_uri":input.redirect_uri,"code":code,"code_verifier":verifier})).await
    }
    async fn exchange(&self, request: Value) -> Value {
        let tokens::ExchangeOutcome::Token(value) = tokens::exchange(
            &self.ui.app.db,
            &self.ui.app.auth,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
            &request,
            Some(&self.origin),
        )
        .await
        .unwrap() else {
            panic!("Actual OAuth exchange must succeed");
        };
        value
    }
    async fn context(&self, token: &Value) -> Context {
        let c = self.ui.app.db.connect().await.unwrap();
        let mut ctx = self
            .ui
            .app
            .auth
            .authenticate(
                &c,
                token["access_token"].as_str().unwrap(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        ctx.browser_origin = Some(self.origin.clone());
        ctx
    }
    async fn discover(&self, token: &str, origin: Option<&str>) -> axum::response::Response {
        let path = format!(
            "/api/platform/v1/ui/contributions/{}/desktop",
            self.ui.deployment
        );
        let mut request = Request::builder()
            .uri(path)
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.ui.app.config).unwrap(),
            )
            .header("authorization", format!("Bearer {token}"));
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let mut request = request.body(Body::empty()).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
        ));
        crate::api::router(self.ui.app.clone())
            .oneshot(request)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn actual_postgres_nondefault_framing_policy_is_private_stable_and_revocable() {
    let Some(f) = FramingFixture::new().await else {
        return;
    };
    f.observe().await;
    let c = f.ui.app.db.connect().await.unwrap();
    let public = crate::webui::frame_ancestors(&c, &f.ui.app, f.ui.deployment)
        .await
        .unwrap();
    assert_eq!(public, vec!["https://desktop.example.org"]);
    let csp = crate::webui::csp(&c, &f.ui.app, f.shell, &f.signed)
        .await
        .unwrap();
    assert!(csp.contains("frame-src https://*.component-ui.example.org:8443;"));
    for id in [f.shell, f.ui.deployment, f.ui.installation] {
        assert!(!csp.contains(&id.to_string()) && !csp.contains(&id.simple().to_string()));
    }
    let bare = f.ui.get(&f.ui.owner, "desktop").await.unwrap();
    assert_eq!(bare["availability"], "ready");
    assert!(url::Url::parse(bare["frame_url"].as_str().unwrap())
        .unwrap()
        .query()
        .is_none());
    let counterfeit = f.discover(&f.ui.base.token, Some(&f.origin)).await;
    assert_eq!(
        counterfeit.status(),
        403,
        "CLI session cannot claim a private shell Origin"
    );
    let token = f.token().await;
    let response = f
        .discover(token["access_token"].as_str().unwrap(), Some(&f.origin))
        .await;
    assert_eq!(response.status(), 200);
    let contribution: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_048_576).await.unwrap()).unwrap();
    assert_eq!(contribution["availability"], "ready");
    let frame = url::Url::parse(contribution["frame_url"].as_str().unwrap()).unwrap();
    let query = frame
        .query()
        .expect("A nondefault browser parent requires a framing proof");
    assert_eq!(
        crate::webui::validate_framing_query(&c, &f.ui.app, f.ui.deployment, frame.path(), query)
            .await
            .unwrap(),
        f.origin
    );
    let selector = query
        .strip_prefix("xelab_frame=v1.")
        .unwrap()
        .split('.')
        .next()
        .unwrap();
    let selector: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(selector).unwrap()).unwrap();
    assert_eq!(selector.as_object().unwrap().len(), 3);
    for name in [
        "subject",
        "credential",
        "family",
        "session",
        "deployment",
        "installation",
        "origin",
    ] {
        assert!(!selector.as_object().unwrap().contains_key(name));
    }
    let ctx = f.context(&token).await;
    assert_eq!(
        f.ui.get(&ctx, "desktop").await.unwrap()["frame_url"],
        contribution["frame_url"]
    );
    let next = f.exchange(json!({"grant_type":"refresh_token","client_id":f.client["client_id"],"refresh_token":token["refresh_token"]})).await;
    let fresh = f.context(&next).await;
    f.observe().await;
    assert_eq!(
        f.ui.get(&fresh, "desktop").await.unwrap()["frame_url"],
        contribution["frame_url"],
        "Normal refresh must preserve the framing policy URL"
    );
    for deployment in [f.ui.deployment, f.shell] {
        c.execute(
            "UPDATE instances SET health='unhealthy' WHERE deployment_id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
        let unavailable = f.ui.get(&fresh, "desktop").await.unwrap();
        assert_eq!(unavailable["availability"], "unavailable");
        assert!(url::Url::parse(unavailable["frame_url"].as_str().unwrap())
            .unwrap()
            .query()
            .is_none());
        c.execute(
            "UPDATE instances SET health='alive',last_heartbeat_at=now() WHERE deployment_id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
    }
    for (target, path, proof) in [
        (Uuid::new_v4(), frame.path(), query.to_owned()),
        (
            f.ui.deployment,
            "/ui/org.xelab.http-echo/other.html",
            query.to_owned(),
        ),
        (f.ui.deployment, frame.path(), format!("{query}&extra=1")),
    ] {
        assert!(
            crate::webui::validate_framing_query(&c, &f.ui.app, target, path, &proof)
                .await
                .is_err()
        );
    }
    let (prefix, mac) = query.rsplit_once('.').unwrap();
    let mut mac = mac.as_bytes().to_vec();
    mac[0] = if mac[0] == b'A' { b'B' } else { b'A' };
    assert!(crate::webui::validate_framing_query(
        &c,
        &f.ui.app,
        f.ui.deployment,
        frame.path(),
        &format!("{prefix}.{}", String::from_utf8(mac).unwrap())
    )
    .await
    .is_err());
    c.execute(
        "UPDATE clients SET revision=revision+1 WHERE client_id=$1",
        &[&f.client["client_id"].as_str().unwrap()],
    )
    .await
    .unwrap();
    assert!(crate::webui::validate_framing_query(
        &c,
        &f.ui.app,
        f.ui.deployment,
        frame.path(),
        query
    )
    .await
    .is_err());
    c.execute(
        "UPDATE clients SET revision=revision-1 WHERE client_id=$1",
        &[&f.client["client_id"].as_str().unwrap()],
    )
    .await
    .unwrap();
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config,client_id}','\"other-client\"') WHERE id=$1",&[&f.shell]).await.unwrap();
    assert!(crate::webui::validate_framing_query(
        &c,
        &f.ui.app,
        f.ui.deployment,
        frame.path(),
        query
    )
    .await
    .is_err());
    c.execute("UPDATE deployments SET spec=jsonb_set(spec,'{config,client_id}',to_jsonb($2::text)) WHERE id=$1",&[&f.shell,&f.client["client_id"].as_str().unwrap()]).await.unwrap();
    for deployment in [f.ui.deployment, f.shell] {
        c.execute(
            "DELETE FROM plugin_http_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
        assert!(crate::webui::validate_framing_query(
            &c,
            &f.ui.app,
            f.ui.deployment,
            frame.path(),
            query
        )
        .await
        .is_err());
        let (manifest, approval) = if deployment == f.shell {
            (&f.signed, json!({}))
        } else {
            (&f.ui.manifest, json!({"rules":[],"egress":[]}))
        };
        crate::plugin_http_gateway::approve(
            &c,
            &f.ui.owner,
            deployment,
            manifest,
            &approval,
            f.ui.owner.subject,
        )
        .await
        .unwrap();
    }
    f.observe().await;
    crate::webui::validate_framing_query(&c, &f.ui.app, f.ui.deployment, frame.path(), query)
        .await
        .unwrap();
    let binding = crate::oauth::binding(&c, fresh.credential).await.unwrap();
    crate::oauth::revoke_family(&c, binding.family_id)
        .await
        .unwrap();
    assert!(
        f.ui.get(&fresh, "desktop").await.is_err(),
        "A framing URL must not restore caller/UI authority"
    );
    crate::webui::validate_framing_query(&c, &f.ui.app, f.ui.deployment, frame.path(), query)
        .await
        .unwrap();
    c.execute(
        "UPDATE clients SET status='disabled' WHERE client_id=$1",
        &[&f.client["client_id"].as_str().unwrap()],
    )
    .await
    .unwrap();
    assert!(crate::webui::validate_framing_query(
        &c,
        &f.ui.app,
        f.ui.deployment,
        frame.path(),
        query
    )
    .await
    .is_err());
}

#[tokio::test]
async fn actual_gateway_framing_proof_uses_mtls_and_never_forwards_query_or_user_credentials() {
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        SanType,
    };
    use std::{fs, os::unix::fs::PermissionsExt, process::Stdio};
    use tokio::io::{AsyncBufReadExt, BufReader};
    let Some(mut f) = FramingFixture::new().await else {
        return;
    };
    let token = f.token().await;
    let ctx = f.context(&token).await;
    let root = f.ui.app.config.root();
    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::new(vec!["framing-component-ca".into()]).unwrap();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    fs::write(root.join("framing-ca.crt"), ca.pem()).unwrap();
    fs::write(root.join("framing-ca.key"), ca_key.serialize_pem()).unwrap();
    fs::set_permissions(
        root.join("framing-ca.key"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let mut config = f.ui.app.config.value.clone();
    config["security"]["workload_ca_cert_file"] = json!(root.join("framing-ca.crt"));
    config["security"]["workload_ca_key_file"] = json!(root.join("framing-ca.key"));
    config["runtime"]["network_cidr"] = json!("127.31.0.0/16");
    config["server"]["host_rpc_listen"] = json!("127.31.0.1:7443");
    let config = Config::from_value(config).unwrap();
    crate::tls::issue(&config, None, &root.join("kernel-identity")).unwrap();
    let app = Arc::get_mut(&mut f.ui.app).unwrap();
    app.config = config.clone();
    app.runtime = Runtime::new(config);
    let c = f.ui.app.db.connect().await.unwrap();
    let instance: Uuid = c
        .query_one(
            "SELECT id FROM instances WHERE deployment_id=$1",
            &[&f.ui.deployment],
        )
        .await
        .unwrap()
        .get(0);
    let runtime = root.join("runtime").join(instance.to_string());
    let identity = runtime.join("identity");
    fs::create_dir_all(&identity).unwrap();
    fs::write(
        runtime.join("network.json"),
        serde_json::to_vec(&json!({"instance":instance,"peer":"127.31.0.2"})).unwrap(),
    )
    .unwrap();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![format!("i-{instance}-g-1.xelab.local")]).unwrap();
    params
        .distinguished_name
        .push(DnType::CommonName, format!("i-{instance}-g-1.xelab.local"));
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names.push(SanType::URI(
        format!("spiffe://xelab.local/instance/{instance}/generation/1")
            .try_into()
            .unwrap(),
    ));
    let cert = params.signed_by(&key, &ca, &ca_key).unwrap();
    fs::write(identity.join("tls.crt"), cert.pem()).unwrap();
    fs::write(identity.join("tls.key"), key.serialize_pem()).unwrap();
    fs::write(identity.join("ca.crt"), ca.pem()).unwrap();
    // This is the independent reference plugin's actual TLS/HTTP listener.
    // The adapter rejects a request if any core-only selector or credential
    // reaches it; no application result or core authority is simulated.
    let script = r#"
import sys
sys.path.insert(0,sys.argv[1])
from http_server import KernelHTTPServer,State,Handler
original=Handler.do_GET
def checked(self):
 if '?' in self.path or self.headers.get('Authorization') or self.headers.get('Cookie'):
  return self.reply(409,b'forbidden-forwarding','text/plain')
 return original(self)
Handler.do_GET=checked
state=State();state.initialize()
server=KernelHTTPServer(('127.31.0.2',8080),state,sys.argv[2])
print('READY',flush=True)
server.serve_forever()
"#;
    let examples = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/http");
    let mut server = tokio::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(examples)
        .arg(identity)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("READY")
    );
    f.observe().await;
    let contribution = f.ui.get(&ctx, "desktop").await.unwrap();
    assert_eq!(contribution["availability"], "ready");
    let frame = url::Url::parse(contribution["frame_url"].as_str().unwrap()).unwrap();
    let path = format!("{}?{}", frame.path(), frame.query().unwrap());
    let mut request = Request::builder()
        .uri(path)
        .header(
            "host",
            frame[url::Position::BeforeHost..url::Position::AfterPort].to_owned(),
        )
        .header("authorization", "Bearer never-forward-this")
        .header("cookie", "private=never-forward-this")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap(),
    ));
    let response = crate::api::router(f.ui.app.clone())
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let csp = response.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    assert!(csp.contains(&format!("frame-ancestors {}", f.origin)));
    assert!(!csp.contains("https://desktop.example.org"));
    assert!(!response.headers().contains_key("set-cookie"));
    assert!(!to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap()
        .is_empty());
    let remaining: i64 = c
        .query_one("SELECT count(*) FROM plugin_http_requests", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        remaining, 0,
        "Completed HTTP transport must release its original static lease"
    );
    server.kill().await.unwrap();
}

#[tokio::test]
async fn actual_postgres_opaque_ui_cursor_filters_private_prefix_and_binds_current_source() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let (reader, _) = f.guest().await;
    let (private_owner, _) = f.guest().await;
    let c = f.app.db.connect().await.unwrap();
    let mut manifest = f.manifest.clone();
    manifest["version"] = json!("1.0.1");
    let template = manifest["ui"]["contributions"][0].clone();
    manifest["ui"]["contributions"] = json!((0..16)
        .map(|index| {
            let mut spec = template.clone();
            spec["id"] = json!(format!("private-{index:02}"));
            spec["skill_bindings"] = json!([]);
            spec
        })
        .collect::<Vec<_>>());
    validate_manifest(&manifest).unwrap();
    let installation = Uuid::new_v4();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())", &[&installation,&string(&manifest,"plugin_id").unwrap(),&string(&manifest,"version").unwrap(),&f.publisher,&crate::contracts::digest(b"private cursor metadata image"),&crate::contracts::digest(&serde_json::to_vec(&manifest).unwrap()),&manifest]).await.unwrap();
    let raw = std::fs::read(f._artifact.path().join("xelab/http/openapi.json")).unwrap();
    crate::http_contracts::store(&c, installation, &manifest, Some(&raw))
        .await
        .unwrap();
    let private: Vec<_> = (1..=13).map(Uuid::from_u128).collect();
    for deployment in &private {
        assert!(*deployment < f.deployment);
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,approval,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":false}','disabled','{}',now(),now())", &[deployment,&private_owner.subject,&installation]).await.unwrap();
    }
    let empty = list(&c, &f.app, &reader, "limit=1").await.unwrap();
    assert!(empty["items"].as_array().unwrap().is_empty());
    assert!(
        empty["next_cursor"].is_null(),
        "208 inaccessible contributions must not create a cursor/count bucket"
    );
    let grant = f
        .grant_scope(
            &reader,
            "plugin.ui.open",
            json!({"kind":"plugin","match":"owned_by","owner_id":f.owner.subject}),
        )
        .await;
    let first = list(&c, &f.app, &reader, "limit=1").await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    assert_eq!(first["items"][0]["deployment_id"], f.deployment.to_string());
    let cursor = first["next_cursor"].as_str().unwrap();
    let ciphertext = URL_SAFE_NO_PAD
        .decode(cursor.strip_prefix("u1.").unwrap())
        .unwrap();
    assert!(serde_json::from_slice::<Value>(&ciphertext).is_err());
    for deployment in &private {
        assert!(!cursor.contains(&deployment.to_string()));
        assert!(!ciphertext
            .windows(36)
            .any(|part| part == deployment.to_string().as_bytes()));
    }
    let query = format!("limit=1&cursor={cursor}");
    let second = list(&c, &f.app, &reader, &query).await.unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    let (other, _) = f.guest().await;
    assert!(list(&c, &f.app, &other, &query).await.is_err());
    for origin in [None, Some("https://other-source.example")] {
        let mut changed = reader.clone();
        changed.browser_origin = origin.map(str::to_owned);
        if changed.browser_origin != reader.browser_origin {
            assert!(list(&c, &f.app, &changed, &query).await.is_err());
        }
    }
    let mut changed = reader.clone();
    changed.peer = "127.0.0.2".parse().unwrap();
    assert!(list(&c, &f.app, &changed, &query).await.is_err());
    let login = f
        .app
        .auth
        .login(
            &c,
            reader.subject,
            "UI component guest password".into(),
            reader.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    let replacement = f
        .app
        .auth
        .authenticate(
            &c,
            login["access_token"].as_str().unwrap(),
            reader.peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert!(list(&c, &f.app, &replacement, &query).await.is_err());
    assert!(
        list(&c, &f.app, &reader, &query.replace("limit=1", "limit=2"))
            .await
            .is_err()
    );
    let mut bad = cursor.as_bytes().to_vec();
    bad[10] = if bad[10] == b'A' { b'B' } else { b'A' };
    assert!(list(
        &c,
        &f.app,
        &reader,
        &format!("limit=1&cursor={}", String::from_utf8(bad).unwrap())
    )
    .await
    .is_err());
    c.execute(
        "UPDATE grants SET revision=revision+1 WHERE id=$1",
        &[&grant],
    )
    .await
    .unwrap();
    assert!(list(&c, &f.app, &reader, &query).await.is_err());
    let legacy = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&json!({"binding":{},"last_deployment":private[0]})).unwrap());
    let legacy = format!(
        "{legacy}.{}",
        URL_SAFE_NO_PAD.encode(f.app.auth.token_digest(&legacy))
    );
    assert!(
        list(&c, &f.app, &reader, &format!("limit=1&cursor={legacy}"))
            .await
            .is_err()
    );
    // All 208 metadata contributions below are now explicitly in this user's
    // scope. Their unavailability is real fixture state, not a fake runtime.
    f.grant_scope(
        &reader,
        "plugin.ui.open",
        json!({"kind":"plugin","match":"owned_by","owner_id":private_owner.subject}),
    )
    .await;
    let large = list(&c, &f.app, &reader, "limit=200").await.unwrap();
    assert_eq!(large["items"].as_array().unwrap().len(), 200);
    let tail = list(
        &c,
        &f.app,
        &reader,
        &format!(
            "limit=200&cursor={}",
            large["next_cursor"].as_str().unwrap()
        ),
    )
    .await
    .unwrap();
    assert_eq!(tail["items"].as_array().unwrap().len(), 10);
    assert!(tail["next_cursor"].is_null());
}

#[tokio::test]
async fn actual_postgres_ui_preselection_resolves_oauth_id_and_owned_intersection_without_cross_owner_access(
) {
    let Some(f) = FramingFixture::new().await else {
        return;
    };
    let (reader, _) = f.ui.guest().await;
    let (other_owner, _) = f.ui.guest().await;
    let plugin = string(&f.ui.manifest, "plugin_id").unwrap();
    let exact = json!({"kind":"plugin","match":"ids","ids":[plugin]});
    let grant =
        f.ui.grant_scope(&reader, "plugin.ui.open", exact.clone())
            .await;
    let c = f.ui.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE clients SET spec=jsonb_set(spec,'{maximum_rules}',$2) WHERE client_id=$1",
        &[
            &f.client["client_id"].as_str().unwrap(),
            &json!([{"action":"plugin.ui.open","resource":exact}]),
        ],
    )
    .await
    .unwrap();
    let token = f.token_for(&reader).await;
    let narrowed = json!({"kind":"plugin","match":"owned_by","owner_id":f.ui.owner.subject});
    let group: Uuid = c
        .query_one("SELECT group_id FROM grants WHERE id=$1", &[&grant])
        .await
        .unwrap()
        .get(0);
    c.execute("INSERT INTO group_versions(group_id,version,rules,created_by,created_at) VALUES($1,2,$2,$3,now())",&[&group,&json!([{"action":"plugin.ui.open","resource":narrowed}]),&f.ui.owner.subject]).await.unwrap();
    c.execute(
        "UPDATE permission_groups SET latest_version=2 WHERE id=$1",
        &[&group],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE grants SET group_version=2,scope=$2,revision=revision+1 WHERE id=$1",
        &[&grant, &narrowed],
    )
    .await
    .unwrap();
    let ctx = f.context(&token).await;
    let baseline = Policy::for_context(&c, &ctx)
        .credential(ctx.credential, 0)
        .await
        .unwrap();
    assert!(
        !baseline
            .iter()
            .any(|path| path.rule["action"] == "plugin.ui.open"),
        "Default unresolved-owner policy behavior must remain unchanged"
    );
    let candidates = policy::PluginOwnerCandidates::default();
    let observed = Policy::for_context(&c, &ctx)
        .with_plugin_owner_candidates(&candidates)
        .credential(ctx.credential, 0)
        .await
        .unwrap();
    assert_eq!(policy::rules(&observed), policy::rules(&baseline));
    assert!(candidates
        .pairs()
        .unwrap()
        .contains(&(plugin.to_owned(), f.ui.owner.subject)));
    let other = Uuid::from_u128(42);
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,approval,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":false}','disabled','{}',now(),now())",&[&other,&other_owner.subject,&f.ui.installation]).await.unwrap();
    f.observe().await;
    let page = list(&c, &f.ui.app, &ctx, "").await.unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["deployment_id"] == f.ui.deployment.to_string()));
    assert!(page["next_cursor"].is_null());
    let params = HashMap::from([
        ("deployment_id".into(), other.to_string()),
        ("contribution_id".into(), "desktop".into()),
    ]);
    assert_eq!(
        execute(
            &c,
            &f.ui.app,
            &ctx,
            "getUiContribution",
            &params,
            &Value::Null,
            &HeaderMap::new(),
            ""
        )
        .await
        .unwrap_err()
        .code,
        "RESOURCE_NOT_FOUND"
    );
}
