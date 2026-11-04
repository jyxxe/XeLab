//! Actual PostgreSQL UI metadata and source-graph execution. Runtime observations
//! here are explicit fixtures; these tests do not claim OCI/browser acceptance.
use super::*;
use crate::{backups::tests::Fixture, config::Config, runtime::Runtime};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
};
use std::sync::{atomic::AtomicBool, Arc};
use tower::ServiceExt;

struct UiFixture {
    base: Fixture,
    app: Arc<App>,
    owner: Context,
    deployment: Uuid,
    installation: Uuid,
    skill: Uuid,
    publisher: Uuid,
    manifest: Value,
    _artifact: tempfile::TempDir,
}

fn ui_spec(manifest: &Value) -> Value {
    let plugin = string(manifest, "plugin_id").unwrap();
    let binding = json!({"alias":"echo","name":manifest["skills"][0]["name"],"version":manifest["skills"][0]["version"],"modes":["call","batch"]});
    json!({"protocol_version":"1","base_path":format!("/ui/{plugin}/"),"contributions":[
        {"id":"desktop","kind":"window","title":"Actual UI metadata window","description":"An independently admitted static window","entry_path":"/index.html","skill_bindings":[binding.clone()],"window":{"width":800,"height":600,"min_width":320,"min_height":200,"max_width":1600,"max_height":1200,"resizable":true},"refresh_interval_seconds":0},
        {"id":"status","kind":"widget","title":"Actual UI metadata widget","description":"A separately managed widget","entry_path":"/index.html","skill_bindings":[binding],"widget":{"columns":2,"rows":2,"min_columns":1,"min_rows":1,"max_columns":4,"max_rows":4},"refresh_interval_seconds":30}
    ]})
}

impl UiFixture {
    async fn new() -> Option<Self> {
        let base = Fixture::new().await?;
        let c = base.app.db.connect().await.unwrap();
        let owner = base
            .app
            .auth
            .authenticate(
                &c,
                &base.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let mut value = base.app.config.value.clone();
        value["ui_gateway"] = json!({"base_domain":"component-ui.example.org","public_port":8443,"shell_origins":["https://desktop.example.org"]});
        let config = Config::from_value(value).unwrap();
        let app = Arc::new(App {
            config: config.clone(),
            db: base.app.db.clone(),
            auth: base.app.auth.clone(),
            runtime: Runtime::new(config),
            leader: base.app.leader.clone(),
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        let publisher = Uuid::new_v4();
        let (artifact, mut manifest, raw) =
            crate::http_contracts::tests::fixture_manifest(publisher);
        manifest["ui"] = ui_spec(&manifest);
        validate_manifest(&manifest).unwrap();
        let installation = Uuid::new_v4();
        let deployment = Uuid::new_v4();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'UI metadata publisher',$2,'fixture-only','trusted',now(),now())", &[&publisher,&publisher.to_string()]).await.unwrap();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'installed',now(),now())", &[&installation,&string(&manifest,"plugin_id").unwrap(),&string(&manifest,"version").unwrap(),&publisher,&crate::contracts::digest(b"UI metadata image"),&crate::contracts::digest(&serde_json::to_vec(&manifest).unwrap()),&manifest]).await.unwrap();
        crate::http_contracts::store(&c, installation, &manifest, Some(&raw))
            .await
            .unwrap();
        let approval = json!({"rules":[],"egress":[]});
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,observed_generation,approval,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":true}','ready',1,$4,now(),now())", &[&deployment,&owner.subject,&installation,&approval]).await.unwrap();
        crate::plugin_http_gateway::approve(
            &c,
            &owner,
            deployment,
            &manifest,
            &approval,
            owner.subject,
        )
        .await
        .unwrap();
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())", &[&Uuid::new_v4(),&deployment]).await.unwrap();
        let (skill, _) = crate::skills::execute(&c, &app.auth, &owner, "registerSkill", &HashMap::new(), &json!({"installation_id":installation,"definition":manifest["skills"][0],"enabled":true}), &HeaderMap::new(), "").await.unwrap();
        let skill = id(&skill, "id").unwrap();
        Some(Self {
            base,
            app,
            owner,
            deployment,
            installation,
            skill,
            publisher,
            manifest,
            _artifact: artifact,
        })
    }

    async fn guest(&self) -> (Context, String) {
        let c = self.app.db.connect().await.unwrap();
        let guest = Uuid::new_v4();
        let password = "UI component guest password";
        c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','UI Guest',$2,now(),now())", &[&guest,&auth::password_hash(password).unwrap()]).await.unwrap();
        c.execute(
            "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
            &[&guest],
        )
        .await
        .unwrap();
        let issued = self
            .app
            .auth
            .login(&c, guest, password.into(), self.owner.peer, Uuid::new_v4())
            .await
            .unwrap();
        let token = issued["access_token"].as_str().unwrap().to_owned();
        let ctx = self
            .app
            .auth
            .authenticate(&c, &token, self.owner.peer, Uuid::new_v4())
            .await
            .unwrap();
        (ctx, token)
    }

    async fn grant(&self, guest: &Context, action: &str, kind: &str, resource_id: &str) -> Uuid {
        self.grant_scope(
            guest,
            action,
            json!({"kind":kind,"match":"ids","ids":[resource_id]}),
        )
        .await
    }

    async fn grant_scope(&self, guest: &Context, action: &str, scope: Value) -> Uuid {
        let c = self.app.db.connect().await.unwrap();
        let group = Uuid::new_v4();
        let grant = Uuid::new_v4();
        let rules = json!([{"action":action,"resource":scope}]);
        c.execute("INSERT INTO permission_groups(id,owner_id,name,visibility,latest_version,status,created_at,updated_at) VALUES($1,$2,$3,'private',1,'active',now(),now())", &[&group,&self.owner.subject,&format!("UI {action} {group}")]).await.unwrap();
        c.execute("INSERT INTO group_versions(group_id,version,rules,created_by,created_at) VALUES($1,1,$2,$3,now())", &[&group,&rules,&self.owner.subject]).await.unwrap();
        c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,group_id,group_version,scope,status,created_at,updated_at) VALUES($1,$2,$3,$4,$5,1,$6,'active',now(),now())", &[&grant,&guest.subject,&self.owner.subject,&self.owner.credential,&group,&scope]).await.unwrap();
        grant
    }

    async fn get(&self, ctx: &Context, contribution: &str) -> Result<Value> {
        let c = self.app.db.connect().await.unwrap();
        let params = HashMap::from([
            ("deployment_id".to_owned(), self.deployment.to_string()),
            ("contribution_id".to_owned(), contribution.to_owned()),
        ]);
        execute(
            &c,
            &self.app,
            ctx,
            "getUiContribution",
            &params,
            &Value::Null,
            &HeaderMap::new(),
            "",
        )
        .await
        .map(|v| v.0)
    }
}

#[tokio::test]
async fn actual_postgres_plugin_owner_scope_is_fixed_to_each_contribution_deployment() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let (reader, _) = f.guest().await;
    let (other_owner, _) = f.guest().await;
    f.grant_scope(
        &reader,
        "plugin.ui.open",
        json!({"kind":"plugin","match":"owned_by","owner_id":f.owner.subject}),
    )
    .await;
    let c = f.app.db.connect().await.unwrap();
    let other = Uuid::new_v4();
    let approval = json!({"rules":[],"egress":[]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,observed_generation,approval,created_at,updated_at) VALUES($1,$2,$3,'{\"enabled\":true}','ready',1,$4,now(),now())", &[&other,&other_owner.subject,&f.installation,&approval]).await.unwrap();
    crate::plugin_http_gateway::approve(
        &c,
        &f.owner,
        other,
        &f.manifest,
        &approval,
        other_owner.subject,
    )
    .await
    .unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())", &[&Uuid::new_v4(),&other]).await.unwrap();
    assert_eq!(
        f.get(&reader, "desktop").await.unwrap()["availability"],
        "ready"
    );
    let params = HashMap::from([
        ("deployment_id".to_owned(), other.to_string()),
        ("contribution_id".to_owned(), "desktop".to_owned()),
    ]);
    assert_eq!(
        execute(
            &c,
            &f.app,
            &reader,
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
    let page = list(&c, &f.app, &reader, "").await.unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert!(page["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["deployment_id"] == json!(f.deployment)));
}

#[test]
fn signed_ui_rejects_path_alias_version_and_size_escapes() {
    let (_artifact, mut manifest, _raw) =
        crate::http_contracts::tests::fixture_manifest(Uuid::new_v4());
    manifest["ui"] = ui_spec(&manifest);
    validate_manifest(&manifest).unwrap();
    for path in [
        "/../index.html",
        "/a//index.html",
        "/%2e%2e/index.html",
        "/index.html?secret=1",
        "//evil.example.org/x",
        "/a\\index.html",
    ] {
        let mut wrong = manifest.clone();
        wrong["ui"]["contributions"][0]["entry_path"] = json!(path);
        assert!(validate_manifest(&wrong).is_err(), "accepted {path}");
    }
    for mutation in ["alias", "version", "id", "size", "route", "base"] {
        let mut wrong = manifest.clone();
        match mutation {
            "alias" => {
                let binding = wrong["ui"]["contributions"][0]["skill_bindings"][0].clone();
                wrong["ui"]["contributions"][0]["skill_bindings"]
                    .as_array_mut()
                    .unwrap()
                    .push(binding);
            }
            "version" => {
                wrong["ui"]["contributions"][0]["skill_bindings"][0]["version"] = json!("2.0.0")
            }
            "id" => wrong["ui"]["contributions"][1]["id"] = json!("desktop"),
            "size" => wrong["ui"]["contributions"][0]["window"]["min_width"] = json!(1600),
            "route" => wrong["routes"][1]["authentication"] = json!("user"),
            "base" => wrong["ui"]["base_path"] = json!("/ui/org.xelab.other/"),
            _ => unreachable!(),
        }
        assert!(validate_manifest(&wrong).is_err(), "accepted {mutation}");
    }
}

#[tokio::test]
async fn actual_postgres_discovery_filters_dual_authority_and_converges_revocation() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let owner = f.get(&f.owner, "desktop").await.unwrap();
    assert_eq!(owner["availability"], "ready");
    assert_eq!(owner["installation_id"], json!(f.installation));
    assert_eq!(owner["bindings"][0]["skill_id"], json!(f.skill));
    assert_eq!(owner["bindings"][0]["modes"], json!(["call"]));
    assert_eq!(
        owner["contribution_digest"],
        json!(crate::skills::definition_digest(&f.manifest["ui"]["contributions"][0]).unwrap())
    );
    let mut parent = f.owner.clone();
    parent.browser_origin = Some("https://unregistered-shell.example.org".into());
    let foreign = f.get(&parent, "desktop").await.unwrap();
    assert_eq!(foreign["availability"], "unavailable");
    assert!(foreign["reason_codes"]
        .as_array()
        .unwrap()
        .contains(&json!("PARENT_ORIGIN_UNAVAILABLE")));
    parent.browser_origin = Some("https://desktop.example.org".into());
    assert_eq!(
        f.get(&parent, "desktop").await.unwrap()["availability"],
        "ready"
    );
    let (guest, _) = f.guest().await;
    assert_eq!(
        f.get(&guest, "desktop").await.unwrap_err().code,
        "RESOURCE_NOT_FOUND"
    );
    let c = f.app.db.connect().await.unwrap();
    assert_eq!(
        list(&c, &f.app, &guest, "").await.unwrap()["items"],
        json!([])
    );
    let plugin = string(&f.manifest, "plugin_id").unwrap();
    let ui_grant = f.grant(&guest, "plugin.ui.open", "plugin", plugin).await;
    let without_business = f.get(&guest, "desktop").await.unwrap();
    assert_eq!(without_business["availability"], "ready");
    assert_eq!(without_business["bindings"], json!([]));
    f.grant(&guest, "skill.invoke", "skill", &f.skill.to_string())
        .await;
    assert_eq!(
        f.get(&guest, "desktop").await.unwrap()["bindings"],
        json!([])
    );
    let business_grant = f
        .grant(&guest, "plugin.http.invoke", "plugin", plugin)
        .await;
    assert_eq!(
        f.get(&guest, "desktop").await.unwrap()["bindings"][0]["skill_id"],
        json!(f.skill)
    );
    c.execute(
        "UPDATE grants SET status='revoked' WHERE id=$1",
        &[&business_grant],
    )
    .await
    .unwrap();
    assert_eq!(
        f.get(&guest, "desktop").await.unwrap()["bindings"],
        json!([])
    );
    c.execute(
        "UPDATE grants SET status='revoked' WHERE id=$1",
        &[&ui_grant],
    )
    .await
    .unwrap();
    assert_eq!(
        f.get(&guest, "desktop").await.unwrap_err().code,
        "RESOURCE_NOT_FOUND"
    );
    c.execute("UPDATE skills SET enabled=false WHERE id=$1", &[&f.skill])
        .await
        .unwrap();
    let disabled = f.get(&f.owner, "desktop").await.unwrap();
    assert_eq!(disabled["availability"], "unavailable");
    assert!(disabled["reason_codes"]
        .as_array()
        .unwrap()
        .contains(&json!("SKILL_BINDING_UNAVAILABLE")));
    c.execute("UPDATE skills SET enabled=true WHERE id=$1", &[&f.skill])
        .await
        .unwrap();
    c.execute(
        "UPDATE publishers SET status='revoked' WHERE id=$1",
        &[&f.publisher],
    )
    .await
    .unwrap();
    assert_eq!(
        f.get(&f.owner, "desktop").await.unwrap()["availability"],
        "unavailable"
    );
    // Retain the fixture's source session until all reads and revocations finish.
    assert!(!f.base.token.is_empty());
}

#[tokio::test]
async fn actual_postgres_preferences_are_account_owned_etag_guarded_and_preserve_inert_layouts() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let params = HashMap::new();
    let mut tx = c.transaction().await.unwrap();
    db::auth_lock(&tx, false).await.unwrap();
    let initial = execute(
        &tx,
        &f.app,
        &f.owner,
        "getMyUiPreferences",
        &params,
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap()
    .0;
    assert_eq!(initial["revision"], 1);
    assert_eq!(initial["spec"], default_preferences());
    tx.commit().await.unwrap();
    let (guest, _) = f.guest().await;
    let guest_initial = execute(
        &c,
        &f.app,
        &guest,
        "getMyUiPreferences",
        &params,
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap()
    .0;
    assert_ne!(guest_initial["id"], initial["id"]);
    assert_eq!(guest_initial["owner_id"], json!(guest.subject));
    let mut wanted = default_preferences();
    wanted["theme"]["mode"] = json!("dark");
    wanted["dock"]["pinned"] = json!([{"deployment_id":f.deployment,"contribution_id":"desktop"}]);
    wanted["widgets"] = json!([{"instance_id":Uuid::new_v4(),"contribution":{"deployment_id":f.deployment,"contribution_id":"status"},"column":0,"row":0,"columns":2,"rows":2}]);
    let mut headers = HeaderMap::new();
    headers.insert("if-match", "\"rev-1\"".parse().unwrap());
    let updated = execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wanted,
        &headers,
        "",
    )
    .await
    .unwrap()
    .0;
    assert_eq!(updated["revision"], 2);
    assert_eq!(
        execute(
            &c,
            &f.app,
            &f.owner,
            "setMyUiPreferences",
            &params,
            &wanted,
            &headers,
            ""
        )
        .await
        .unwrap_err()
        .code,
        "ETAG_MISMATCH"
    );
    assert_eq!(
        execute(
            &c,
            &f.app,
            &guest,
            "setMyUiPreferences",
            &params,
            &wanted,
            &headers,
            ""
        )
        .await
        .unwrap_err()
        .code,
        "RESOURCE_NOT_FOUND"
    );
    headers.insert("if-match", "\"rev-2\"".parse().unwrap());
    let mut wrong = wanted.clone();
    wrong["widgets"][0]["columns"] = json!(5);
    assert!(execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wrong,
        &headers,
        ""
    )
    .await
    .is_err());
    wrong = wanted.clone();
    wrong["dock"]["pinned"]
        .as_array_mut()
        .unwrap()
        .push(wanted["dock"]["pinned"][0].clone());
    assert!(execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wrong,
        &headers,
        ""
    )
    .await
    .is_err());
    wrong = wanted.clone();
    wrong["theme"]["custom_tokens"] = json!({"accent":"url(https://evil.example.org)"});
    assert!(execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wrong,
        &headers,
        ""
    )
    .await
    .is_err());
    // A newly observed signed installation may change an old contribution's
    // kind or tighten widget bounds. Exact historical placements remain inert
    // placeholders; changing those invalid placements still requires admission.
    let mut upgraded = f.manifest.clone();
    upgraded["version"] = json!("2.0.0");
    upgraded["ui"]["contributions"][0]["kind"] = json!("widget");
    upgraded["ui"]["contributions"][0]
        .as_object_mut()
        .unwrap()
        .remove("window");
    upgraded["ui"]["contributions"][0]["widget"] =
        upgraded["ui"]["contributions"][1]["widget"].clone();
    upgraded["ui"]["contributions"][1]["widget"]["columns"] = json!(1);
    upgraded["ui"]["contributions"][1]["widget"]["max_columns"] = json!(1);
    validate_manifest(&upgraded).unwrap();
    let upgraded_installation = Uuid::new_v4();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,'2.0.0',$3,$4,$5,$6,'installed',now(),now())", &[&upgraded_installation,&string(&upgraded,"plugin_id").unwrap(),&f.publisher,&crate::contracts::digest(b"upgraded UI metadata image"),&crate::contracts::digest(&serde_json::to_vec(&upgraded).unwrap()),&upgraded]).await.unwrap();
    c.execute(
        "UPDATE deployments SET installation_id=$2 WHERE id=$1",
        &[&f.deployment, &upgraded_installation],
    )
    .await
    .unwrap();
    let retained_after_upgrade = execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wanted,
        &headers,
        "",
    )
    .await
    .unwrap()
    .0;
    assert_eq!(retained_after_upgrade["revision"], 3);
    headers.insert("if-match", "\"rev-3\"".parse().unwrap());
    let mut moved = wanted.clone();
    moved["widgets"][0]["column"] = json!(1);
    assert!(execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &moved,
        &headers,
        ""
    )
    .await
    .is_err());
    c.execute(
        "UPDATE deployments SET archived_at=now() WHERE id=$1",
        &[&f.deployment],
    )
    .await
    .unwrap();
    let retained = execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wanted,
        &headers,
        "",
    )
    .await
    .unwrap()
    .0;
    assert_eq!(retained["revision"], 4);
    headers.insert("if-match", "\"rev-4\"".parse().unwrap());
    wanted["widgets"][0]["column"] = json!(1);
    assert_eq!(
        execute(
            &c,
            &f.app,
            &f.owner,
            "setMyUiPreferences",
            &params,
            &wanted,
            &headers,
            ""
        )
        .await
        .unwrap_err()
        .code,
        "RESOURCE_NOT_FOUND"
    );
    wanted["widgets"] = json!([]);
    wanted["dock"]["pinned"] = json!([]);
    assert!(execute(
        &c,
        &f.app,
        &f.owner,
        "setMyUiPreferences",
        &params,
        &wanted,
        &headers,
        ""
    )
    .await
    .is_ok());
    tx = c.transaction().await.unwrap();
    let record = execute(
        &tx,
        &f.app,
        &guest,
        "getMyUiPreferences",
        &params,
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap()
    .0;
    assert_eq!(record["spec"], default_preferences());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn actual_postgres_discovery_cursor_binds_subject_filters_and_current_visibility() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let first = list(&c, &f.app, &f.owner, "limit=1").await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = list(&c, &f.app, &f.owner, &format!("limit=1&cursor={cursor}"))
        .await
        .unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_ne!(
        first["items"][0]["spec"]["id"],
        second["items"][0]["spec"]["id"]
    );
    assert!(second["next_cursor"].is_null());
    let (guest, _) = f.guest().await;
    assert!(
        list(&c, &f.app, &guest, &format!("limit=1&cursor={cursor}"))
            .await
            .is_err()
    );
    assert!(list(
        &c,
        &f.app,
        &f.owner,
        &format!("limit=1&kind=widget&cursor={cursor}")
    )
    .await
    .is_err());
    assert!(list(&c, &f.app, &f.owner, "kind=widget&%6bind=window")
        .await
        .is_err());
    let page = list(&c, &f.app, &f.owner, "kind=widget").await.unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["spec"]["kind"], "widget");
}

#[tokio::test]
async fn actual_postgres_public_preferences_api_replays_only_the_same_authorized_request() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    async fn request(
        f: &UiFixture,
        method: &str,
        body: Value,
        key: Option<&str>,
        revision: Option<i64>,
    ) -> (u16, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri("/api/platform/v1/me/ui-preferences")
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&f.app.config).unwrap(),
            )
            .header("authorization", format!("Bearer {}", f.base.token));
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if let Some(revision) = revision {
            request = request.header("if-match", format!("\"rev-{revision}\""));
        }
        let bytes = if body.is_null() {
            Vec::new()
        } else {
            request = request.header("content-type", "application/json");
            serde_json::to_vec(&body).unwrap()
        };
        let mut request = request.body(Body::from(bytes)).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:50100".parse::<std::net::SocketAddr>().unwrap(),
        ));
        let response = crate::api::router(f.app.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1048576).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    let initial = request(&f, "GET", Value::Null, None, None).await;
    assert_eq!(initial.0, 200, "{}", initial.1);
    let mut wanted = default_preferences();
    wanted["theme"]["mode"] = json!("dark");
    let updated = request(
        &f,
        "PUT",
        wanted.clone(),
        Some("UI-prefs-stable-request"),
        Some(1),
    )
    .await;
    assert_eq!(updated.0, 200, "{}", updated.1);
    assert_eq!(updated.1["revision"], 2);
    let replay = request(
        &f,
        "PUT",
        wanted.clone(),
        Some("UI-prefs-stable-request"),
        Some(1),
    )
    .await;
    assert_eq!(replay, updated);
    wanted["theme"]["mode"] = json!("light");
    let conflict = request(
        &f,
        "PUT",
        wanted.clone(),
        Some("UI-prefs-stable-request"),
        Some(1),
    )
    .await;
    assert_eq!(conflict.0, 409);
    assert_eq!(conflict.1["error"]["code"], "IDEMPOTENCY_CONFLICT");
    let stale = request(&f, "PUT", wanted, Some("UI-prefs-second-request"), Some(1)).await;
    assert_eq!(stale.0, 412);
    let current = request(&f, "GET", Value::Null, None, None).await;
    assert_eq!(current.1, updated.1);
}

#[tokio::test]
async fn actual_postgres_ui_snapshot_validates_structure_without_restoring_live_authority() {
    let Some(f) = UiFixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let stored = execute(
        &c,
        &f.app,
        &f.owner,
        "getMyUiPreferences",
        &HashMap::new(),
        &Value::Null,
        &HeaderMap::new(),
        "",
    )
    .await
    .unwrap()
    .0;
    let mut spec = stored["spec"].clone();
    let missing = Uuid::new_v4();
    spec["dock"]["pinned"] =
        json!([{"deployment_id":missing,"contribution_id":"historical_window"}]);
    spec["widgets"] = json!([{"instance_id":Uuid::new_v4(),"contribution":{"deployment_id":missing,"contribution_id":"historical_widget"},"column":0,"row":0,"columns":8,"rows":8}]);
    // Nil UUID rows must not be skipped by the bounded cursor's initial page.
    c.execute(
        "UPDATE ui_preferences SET id=$2,spec=$3 WHERE owner_id=$1",
        &[&f.owner.subject, &Uuid::nil(), &spec],
    )
    .await
    .unwrap();
    validate_snapshot(&c).await.unwrap();
    for fault in ["duplicate_pin", "duplicate_instance", "css"] {
        let mut invalid = spec.clone();
        match fault {
            "duplicate_pin" => invalid["dock"]["pinned"]
                .as_array_mut()
                .unwrap()
                .push(spec["dock"]["pinned"][0].clone()),
            "duplicate_instance" => invalid["widgets"]
                .as_array_mut()
                .unwrap()
                .push(spec["widgets"][0].clone()),
            "css" => {
                invalid["theme"]["custom_tokens"] =
                    json!({"accent":"url(https://invalid.example.org)"})
            }
            _ => unreachable!(),
        }
        c.execute(
            "UPDATE ui_preferences SET spec=$2 WHERE owner_id=$1",
            &[&f.owner.subject, &invalid],
        )
        .await
        .unwrap();
        assert!(
            validate_snapshot(&c).await.is_err(),
            "snapshot accepted {fault}"
        );
    }
    c.execute(
        "UPDATE ui_preferences SET spec=$2 WHERE owner_id=$1",
        &[&f.owner.subject, &spec],
    )
    .await
    .unwrap();
    let mut invalid = f.manifest.clone();
    invalid["ui"]["contributions"][0]["entry_path"] = json!("/../auth/ui/login");
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&f.installation, &invalid],
    )
    .await
    .unwrap();
    assert!(validate_snapshot(&c).await.is_err());
    c.execute(
        "UPDATE installations SET manifest=$2,state='uninstalled' WHERE id=$1",
        &[&f.installation, &f.manifest],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE publishers SET status='revoked' WHERE id=$1",
        &[&f.publisher],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE deployments SET archived_at=now(),phase='disabled' WHERE id=$1",
        &[&f.deployment],
    )
    .await
    .unwrap();
    c.execute("UPDATE credentials SET status='revoked'", &[])
        .await
        .unwrap();
    validate_snapshot(&c).await.unwrap();
}

#[path = "ui_frame_policy_tests.rs"]
mod frame_policy;
