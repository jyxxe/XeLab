//! Real PostgreSQL/API regression for the ordinary Skill registry. Installed
//! metadata is a fixture, not evidence that an OCI artifact executed; the real
//! signed-image path is exercised by tests/oci.rs.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{atomic::AtomicBool, Arc},
};
use tower::ServiceExt;
use uuid::Uuid;
use xelab_kernel::{
    api::{router, App},
    auth::{self, Auth},
    config::Config,
    contracts,
    db::Db,
    runtime::Runtime,
};

const PASSWORD: &str = "correct horse Skill acceptance password";

struct Fixture {
    db: Db,
    router: Router,
    owner: Uuid,
    token: String,
    installation: Uuid,
    manifest: Value,
}
impl Fixture {
    async fn new() -> Option<Self> {
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: Skill API regression requires PostgreSQL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("skills_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();
        let source: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let config = Config::from_value(serde_json::to_value(source).unwrap()).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(&tx, "Owner", PASSWORD, &config.value["quota_defaults"])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let publisher = Uuid::new_v4();
        let pem = SigningKey::from_bytes(&[7; 32])
            .verifying_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'Skill fixture','skill-fixture',$2,'trusted',now(),now())", &[&publisher,&pem]).await.unwrap();
        let mut manifest: Value =
            serde_json::from_str(include_str!("fixtures/secret-manifest.json")).unwrap();
        manifest["publisher_id"] = json!(publisher);
        manifest["requested_rules"] = json!([]);
        let cap = manifest["capabilities"][0].clone();
        manifest["skills"] = json!([{"name":format!("{}.echo",manifest["plugin_id"].as_str().unwrap()),"version":"1.0.0","description":"Fixed ordinary echo","input_schema":cap["input_schema"],"output_schema":cap["output_schema"],"stream_item_schema":null,"target":{"kind":"capability","plugin_id":manifest["plugin_id"],"contract":cap["contract"]},"mapping":"identity","maximum_timeout_ms":10000,"input_blobs":false,"tags":[]}]);
        xelab_kernel::artifact::validate_profile(&manifest).unwrap();
        let installation = Uuid::new_v4();
        let hash = contracts::digest(&serde_json::to_vec(&manifest).unwrap());
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,$2,'1.0.0',$3,$4,$4,$5,'installed',now(),now())", &[&installation,&manifest["plugin_id"].as_str().unwrap(),&publisher,&hash,&manifest]).await.unwrap();
        let contract = &cap["contract"];
        c.execute(
            "INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4)",
            &[
                &contract["name"].as_str().unwrap(),
                &contract["version"].as_str().unwrap(),
                &contract["schema_digest"].as_str().unwrap(),
                &cap,
            ],
        )
        .await
        .unwrap();
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db: db.clone(),
            auth: Auth::new(vec![72; 32]).unwrap(),
            leader: Arc::new(db.leader().await.unwrap()),
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        let router = router(app);
        let session = request(
            &router,
            "login",
            &[],
            None,
            json!({"account_id":owner,"password":PASSWORD}),
            None,
            "login",
        )
        .await;
        assert_eq!(session.0, 200, "{}", session.1);
        Some(Self {
            db,
            router,
            owner,
            token: session.1["access_token"].as_str().unwrap().into(),
            installation,
            manifest,
        })
    }
    async fn call(
        &self,
        op: &str,
        params: &[(&str, String)],
        token: &str,
        body: Value,
        revision: Option<i64>,
    ) -> (u16, Value) {
        request(
            &self.router,
            op,
            params,
            Some(token),
            body,
            revision,
            &Uuid::new_v4().to_string(),
        )
        .await
    }
    async fn key(&self, rules: Value) -> Value {
        let result=self.call("createApiKey",&[("account_id",self.owner.to_string())],&self.token,json!({"name":"Skill test key","inheritance":{"mode":"selected","rules":rules},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::days(1)}}),None).await;
        assert_eq!(result.0, 201, "{}", result.1);
        result.1
    }
    fn registration(&self) -> Value {
        json!({"installation_id":self.installation,"definition":self.manifest["skills"][0]})
    }
}

#[allow(clippy::too_many_arguments)]
async fn request(
    router: &Router,
    operation: &str,
    params: &[(&str, String)],
    token: Option<&str>,
    body: Value,
    revision: Option<i64>,
    key: &str,
) -> (u16, Value) {
    let mut selected = None;
    for (path, methods) in contracts::openapi()["paths"].as_object().unwrap() {
        for (method, spec) in methods.as_object().unwrap() {
            if spec["operationId"] == operation {
                selected = Some((path.clone(), method.to_uppercase()));
            }
        }
    }
    let (mut path, method) = selected.unwrap();
    for (name, value) in params {
        path = path.replace(&format!("{{{name}}}"), value);
    }
    let mut req = Request::builder()
        .method(method.as_str())
        .uri(path)
        .header("idempotency-key", key);
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    if let Some(revision) = revision {
        req = req.header("if-match", format!("\"rev-{revision}\""));
    }
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        req = req.header("content-type", "application/json");
        serde_json::to_vec(&body).unwrap()
    };
    let mut req = req.body(Body::from(bytes)).unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
    ));
    let response = router.clone().oneshot(req).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 16777216).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

#[tokio::test]
async fn immutable_registry_rejects_unsigned_changes_and_conflicting_installations() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let registration = f.registration();
    for field in [
        "description",
        "input_schema",
        "target",
        "maximum_timeout_ms",
        "version",
    ] {
        let mut bad = registration.clone();
        match field {
            "description" => bad["definition"][field] = json!("unsigned replacement"),
            "input_schema" => bad["definition"][field] = json!({"type":"object"}),
            "target" => bad["definition"][field]["plugin_id"] = json!("org.xelab.other"),
            "maximum_timeout_ms" => bad["definition"][field] = json!(2000),
            "version" => bad["definition"][field] = json!("9.0.0"),
            _ => unreachable!(),
        }
        let rejected = f.call("registerSkill", &[], &f.token, bad, None).await;
        assert!(matches!(rejected.0, 400 | 409), "{field}: {}", rejected.1);
    }
    let created = request(
        &f.router,
        "registerSkill",
        &[],
        Some(&f.token),
        registration.clone(),
        None,
        "register",
    )
    .await;
    assert_eq!(created.0, 201, "{}", created.1);
    contracts::validate("Skill", &created.1).unwrap();
    assert_eq!(created.1["enabled"], false);
    assert_eq!(created.1["availability"], "disabled");
    let replay = request(
        &f.router,
        "registerSkill",
        &[],
        Some(&f.token),
        registration.clone(),
        None,
        "register",
    )
    .await;
    assert_eq!(replay, created);
    let repeated = f
        .call("registerSkill", &[], &f.token, registration.clone(), None)
        .await;
    assert_eq!(repeated.0, 201, "{}", repeated.1);
    assert_eq!(repeated.1["id"], created.1["id"]);
    let c = f.db.connect().await.unwrap();
    let other = Uuid::new_v4();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) SELECT $1,plugin_id,'2.0.0',publisher_id,$2,manifest_digest,manifest,'installed',now(),now() FROM installations WHERE id=$3", &[&other,&contracts::digest(b"different immutable image"),&f.installation]).await.unwrap();
    let mut conflict = registration;
    conflict["installation_id"] = json!(other);
    let conflict = f.call("registerSkill", &[], &f.token, conflict, None).await;
    assert_eq!(conflict.0, 409, "{}", conflict.1);
    assert_eq!(conflict.1["error"]["code"], "CONTRACT_CONFLICT");
    let params = [("skill_id", created.1["id"].as_str().unwrap().to_owned())];
    let enabled = f
        .call(
            "setSkillEnabled",
            &params,
            &f.token,
            json!({"enabled":true}),
            Some(1),
        )
        .await;
    assert_eq!(enabled.0, 200, "{}", enabled.1);
    assert_eq!(enabled.1["availability"], "unavailable");
    let stale = f
        .call(
            "setSkillEnabled",
            &params,
            &f.token,
            json!({"enabled":false}),
            Some(1),
        )
        .await;
    assert_eq!(stale.0, 412, "{}", stale.1);
    let read = f
        .call("getSkill", &params, &f.token, Value::Null, None)
        .await;
    assert_eq!(read.0, 200, "{}", read.1);
    assert_eq!(read.1["definition_digest"], created.1["definition_digest"]);
    let archived = f
        .call(
            "archiveSkill",
            &params,
            &f.token,
            Value::Null,
            read.1["revision"].as_i64(),
        )
        .await;
    assert_eq!(archived.0, 204, "{}", archived.1);
    let historical = c
        .query_one(
            "SELECT definition,enabled,archived_at IS NOT NULL FROM skills WHERE id=$1",
            &[&params[0].1.parse::<Uuid>().unwrap()],
        )
        .await
        .unwrap();
    assert_eq!(historical.get::<_, Value>(0), f.manifest["skills"][0]);
    assert!(!historical.get::<_, bool>(1));
    assert!(historical.get::<_, bool>(2));
    let audits = c.query("SELECT action,resource FROM audit_events WHERE (action IN ('registerSkill','setSkillEnabled','archiveSkill') AND outcome='succeeded') OR action='setSkillEnabled' AND outcome='failed'", &[]).await.unwrap();
    assert!(audits.len() >= 5);
    for audit in audits {
        let resource: Value = audit.get("resource");
        assert_eq!(resource["kind"], "skill");
        assert_eq!(
            resource["ids"][0],
            created.1["id"],
            "audit must bind the actual Skill: {}",
            audit.get::<_, String>("action")
        );
    }
}

#[tokio::test]
async fn both_authorization_layers_precede_admission_and_no_failure_leaks_quota() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut registration = f.registration();
    registration["enabled"] = json!(true);
    let created = f
        .call("registerSkill", &[], &f.token, registration, None)
        .await;
    assert_eq!(created.0, 201, "{}", created.1);
    let skill = created.1["id"].as_str().unwrap();
    let params = [("skill_id", skill.to_owned())];
    let contract = &f.manifest["capabilities"][0]["contract"];
    let skill_rule =
        json!({"action":"skill.invoke","resource":{"kind":"skill","match":"ids","ids":[skill]}});
    let capability_rule = json!({"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":[format!("{}@{}",contract["name"].as_str().unwrap(),contract["version"].as_str().unwrap())]}});
    let request_body =
        json!({"input":{"message":"metadata-only admission probe"},"timeout_ms":10000.0});
    for rules in [json!([skill_rule]), json!([capability_rule])] {
        let key = f.key(rules).await;
        let refused = f
            .call(
                "invokeSkill",
                &params,
                key["secret"].as_str().unwrap(),
                request_body.clone(),
                None,
            )
            .await;
        assert_eq!(refused.0, 403, "{}", refused.1);
    }
    // A healthy-looking provider of the same contract from another immutable
    // installation must not satisfy this Skill's fixed installation binding.
    let c = f.db.connect().await.unwrap();
    let other = Uuid::new_v4();
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) SELECT $1,plugin_id,'2.0.0',publisher_id,$2,manifest_digest,manifest,'installed',now(),now() FROM installations WHERE id=$3", &[&other,&contracts::digest(b"other same-contract artifact"),&f.installation]).await.unwrap();
    let spec =
        json!({"installation_id":other,"enabled":true,"limits":f.manifest["minimum_resources"]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())",&[&deployment,&f.owner,&other,&spec]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive',$3,now(),now())",&[&instance,&deployment,&spec["limits"]]).await.unwrap();
    c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',1,now())",&[&instance,&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap()]).await.unwrap();
    let key = f.key(json!([skill_rule, capability_rule])).await;
    let admitted = f
        .call(
            "invokeSkill",
            &params,
            key["secret"].as_str().unwrap(),
            request_body.clone(),
            None,
        )
        .await;
    assert_eq!(
        admitted.0, 503,
        "a metadata fixture has no running provider: {}",
        admitted.1
    );
    let mut forged = request_body.clone();
    forged["provider_deployment_id"] = json!(Uuid::new_v4());
    assert_eq!(
        f.call("invokeSkill", &params, &f.token, forged, None)
            .await
            .0,
        400
    );
    let mut oversize = request_body;
    oversize["timeout_ms"] = json!(20000);
    assert_eq!(
        f.call("invokeSkill", &params, &f.token, oversize, None)
            .await
            .0,
        400
    );
    let c = f.db.connect().await.unwrap();
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let q = c
        .query_one(
            "SELECT concurrent_calls_used,daily_calls_used FROM quotas WHERE account_id=$1",
            &[&f.owner],
        )
        .await
        .unwrap();
    assert_eq!(q.get::<_, i64>(0), 0);
    assert_eq!(q.get::<_, i64>(1), 0);
}

#[tokio::test]
async fn scoped_child_visibility_and_live_inheritance_revocation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let created = f
        .call("registerSkill", &[], &f.token, f.registration(), None)
        .await;
    assert_eq!(created.0, 201, "{}", created.1);
    let skill = created.1["id"].as_str().unwrap();
    let params = [("skill_id", skill.to_owned())];
    let child=f.call("createChildAccount",&[("account_id",f.owner.to_string())],&f.token,json!({"display_name":"Skill reader","inheritance":{"mode":"selected","rules":[{"action":"skill.read","resource":{"kind":"skill","match":"ids","ids":[skill]}}]}}),None).await;
    assert_eq!(child.0, 201, "{}", child.1);
    let account = contracts::id(&child.1["account"], "id").unwrap();
    let activated = request(
        &f.router,
        "activateAccount",
        &[],
        None,
        json!({"activation_token":child.1["activation_token"],"password":PASSWORD}),
        None,
        "activate-child",
    )
    .await;
    assert_eq!(activated.0, 200, "{}", activated.1);
    let session = request(
        &f.router,
        "login",
        &[],
        None,
        json!({"account_id":account,"password":PASSWORD}),
        None,
        "child-login",
    )
    .await;
    assert_eq!(session.0, 200, "{}", session.1);
    let token = session.1["access_token"].as_str().unwrap();
    let visible = f.call("listSkills", &[], token, Value::Null, None).await;
    assert_eq!(visible.0, 200, "{}", visible.1);
    contracts::validate("SkillPage", &visible.1).unwrap();
    assert_eq!(visible.1["items"].as_array().unwrap().len(), 1);
    assert_eq!(visible.1["items"][0]["id"], skill);
    assert_eq!(
        f.call("getSkill", &params, token, Value::Null, None)
            .await
            .0,
        200
    );
    assert_eq!(
        f.call(
            "setSkillEnabled",
            &params,
            token,
            json!({"enabled":true}),
            Some(1)
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            "invokeSkill",
            &params,
            token,
            json!({"input":{"message":"read is not invoke"}}),
            None
        )
        .await
        .0,
        403
    );
    let current = f
        .call(
            "getAccount",
            &[("account_id", account.to_string())],
            &f.token,
            Value::Null,
            None,
        )
        .await;
    assert_eq!(current.0, 200, "{}", current.1);
    let narrowed = f
        .call(
            "updateInheritance",
            &[("account_id", account.to_string())],
            &f.token,
            json!({"mode":"selected","rules":[{"action":"skill.read","resource":{"kind":"skill","match":"ids","ids":[Uuid::new_v4()]}}]}),
            current.1["revision"].as_i64(),
        )
        .await;
    assert_eq!(narrowed.0, 200, "{}", narrowed.1);
    assert_eq!(
        f.call("getSkill", &params, token, Value::Null, None)
            .await
            .0,
        404
    );
    let hidden = f.call("listSkills", &[], token, Value::Null, None).await;
    assert_eq!(hidden.0, 200, "{}", hidden.1);
    assert!(hidden.1["items"].as_array().unwrap().is_empty());
    assert_eq!(
        f.call("getSkill", &params, &f.token, Value::Null, None)
            .await
            .0,
        200
    );
}

#[tokio::test]
async fn skill_required_provider_rejects_bare_and_forged_routes_and_preserves_unknown_result() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.db.connect().await.unwrap();
    let mut manifest = f.manifest.clone();
    manifest["business_entry"] = json!("skill_required");
    manifest["concurrency"]["max_active_requests"] = json!(1);
    xelab_kernel::artifact::validate_profile(&manifest).unwrap();
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&f.installation, &manifest],
    )
    .await
    .unwrap();
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let spec = json!({"installation_id":f.installation,"display_name":"metadata-only Skill provider","enabled":true,"replicas":1,"limits":manifest["minimum_resources"],"config":{},"volume_ids":[]});
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())",&[&deployment,&f.owner,&f.installation,&spec]).await.unwrap();
    c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive',$3,now(),now())",&[&instance,&deployment,&spec["limits"]]).await.unwrap();
    let contract = &manifest["capabilities"][0]["contract"];
    c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at) VALUES($1,$2,$3,'ready','[]',1,now())",&[&instance,&contract["name"].as_str().unwrap(),&contract["version"].as_str().unwrap()]).await.unwrap();
    let mut registration = f.registration();
    registration["enabled"] = json!(true);
    let created = f
        .call("registerSkill", &[], &f.token, registration, None)
        .await;
    assert_eq!(created.0, 201, "{}", created.1);
    let skill = created.1["id"].as_str().unwrap();
    let params = [("skill_id", skill.to_owned())];
    let bare = json!({"contract":contract,"input":{"message":"cannot bypass fixed Skill"},"provider_deployment_id":deployment,"timeout_ms":1000});
    let denied = f
        .call("invokeCapability", &[], &f.token, bare.clone(), None)
        .await;
    assert_eq!(denied.0, 403, "{}", denied.1);
    let mut forged = bare.clone();
    forged["skill_id"] = json!(skill);
    assert_eq!(
        f.call("invokeCapability", &[], &f.token, forged, None)
            .await
            .0,
        400
    );
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/platform/v1/calls")
        .header("authorization", format!("Bearer {}", f.token))
        .header("content-type", "application/json")
        .header("idempotency-key", "forged-header")
        .header("x-xelab-skill-id", skill)
        .header("x-xelab-delegation", "untrusted")
        .body(Body::from(serde_json::to_vec(&bare).unwrap()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
    ));
    let response = f.router.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status().as_u16(), 403);
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    // This provider intentionally has no process. The trusted Skill route may
    // reach Broker dispatch, but cannot claim successful execution or free an
    // uncertain instance slot. The OCI test covers actual successful execution.
    let input = json!({"input":{"message":"route reached missing runtime"},"timeout_ms":10000,"idempotency_key":"stable-business-key-distinct-from-http"});
    let failed = request(
        &f.router,
        "invokeSkill",
        &params,
        Some(&f.token),
        input.clone(),
        None,
        "unknown-skill",
    )
    .await;
    assert_ne!(failed.0, 200, "{}", failed.1);
    assert_ne!(failed.0, 403, "{}", failed.1);
    let row = c
        .query_one("SELECT id,skill_id,state,idempotency_key FROM calls", &[])
        .await
        .unwrap_or_else(|error| {
            panic!("expected committed uncertain call after dispatch failure {failed:?}: {error}")
        });
    assert_eq!(row.get::<_, Uuid>(1).to_string(), skill);
    assert_eq!(row.get::<_, String>(2), "unknown");
    assert_eq!(
        row.get::<_, String>(3),
        "stable-business-key-distinct-from-http"
    );
    let replay = request(
        &f.router,
        "invokeSkill",
        &params,
        Some(&f.token),
        input,
        None,
        "unknown-skill",
    )
    .await;
    assert_eq!(replay.0, 409, "{}", replay.1);
    assert_eq!(replay.1["error"]["code"], "OUTCOME_UNKNOWN");
    let another = request(
        &f.router,
        "invokeSkill",
        &params,
        Some(&f.token),
        json!({"input":{"message":"must await physical fencing"},"timeout_ms":10000}),
        None,
        "another-call-before-fence",
    )
    .await;
    assert_eq!(another.0, 503, "{}", another.1);
    assert_eq!(another.1["error"]["code"], "RESOURCE_EXHAUSTED");
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT concurrent_calls_used FROM quotas WHERE account_id=$1",
            &[&f.owner]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn omitted_timeout_uses_fixed_skill_maximum_before_waiting_on_call_lock() {
    let Some(mut f) = Fixture::new().await else {
        return;
    };
    f.manifest["skills"][0]["maximum_timeout_ms"] = json!(1000.0);
    let mut c = f.db.connect().await.unwrap();
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&f.installation, &f.manifest],
    )
    .await
    .unwrap();
    let mut registration = f.registration();
    registration["enabled"] = json!(true);
    let created = f
        .call("registerSkill", &[], &f.token, registration, None)
        .await;
    assert_eq!(created.0, 201, "{}", created.1);
    let params = [("skill_id", created.1["id"].as_str().unwrap().to_owned())];
    let lock = c.transaction().await.unwrap();
    xelab_kernel::db::call_lock(&lock).await.unwrap();
    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        f.call(
            "invokeSkill",
            &params,
            &f.token,
            json!({"input":{"message":"use the fixed default deadline"}}),
            None,
        ),
    )
    .await
    .expect("omitted timeout must not inherit the outer 120s wait");
    assert_eq!(refused.0, 504, "{}", refused.1);
    assert_eq!(refused.1["error"]["code"], "DEADLINE_EXCEEDED");
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let waiting: bool = lock.query_one("SELECT EXISTS(SELECT 1 FROM pg_locks l JOIN pg_stat_activity a ON a.pid=l.pid WHERE a.datname=current_database() AND l.locktype='advisory' AND NOT l.granted)", &[]).await.unwrap().get(0);
            if !waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("deadline must cancel the PostgreSQL waiter before its lock is released");
    lock.rollback().await.unwrap();
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        c.query_one(
            "SELECT concurrent_calls_used,daily_calls_used FROM quotas WHERE account_id=$1",
            &[&f.owner]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        0
    );
}
