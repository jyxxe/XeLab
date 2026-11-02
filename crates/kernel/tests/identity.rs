//! Real PostgreSQL + HTTP acceptance. Each test owns a separate database.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
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

const PASSWORD: &str = "correct horse acceptance password";
struct Fixture {
    db: Db,
    router: Router,
    owner: Uuid,
    token: String,
}
impl Fixture {
    async fn new() -> Option<Self> {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let base = std::env::var("XELAB_TEST_DATABASE_URL").ok()?;
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let database = format!("identity_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {database}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={database}")).unwrap();
        db.migrate().await.unwrap();
        let value: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let config = Config::from_value(serde_json::to_value(value).unwrap()).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(&tx, "Owner", PASSWORD, &config.value["quota_defaults"])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db: db.clone(),
            auth: Auth::new(vec![72; 32]).unwrap(),
            leader: Arc::new(db.leader().await.unwrap()),
            ready: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });
        let router = router(app);
        let result = api(
            &router,
            "login",
            &[],
            None,
            json!({"account_id":owner,"password":PASSWORD}),
            "login",
            None,
            "",
        )
        .await;
        assert_eq!(result.0, 200, "{}", result.1);
        Some(Self {
            db,
            router,
            owner,
            token: result.1["access_token"].as_str().unwrap().into(),
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
        api(
            &self.router,
            op,
            params,
            Some(token),
            body,
            &Uuid::new_v4().to_string(),
            revision,
            "",
        )
        .await
    }
    async fn child(&self, parent: Uuid, token: &str, inheritance: Value) -> (Uuid, String) {
        let out = self
            .call(
                "createChildAccount",
                &[("account_id", parent.to_string())],
                token,
                json!({"display_name":"child","inheritance":inheritance}),
                None,
            )
            .await;
        assert_eq!(out.0, 201, "{}", out.1);
        let account = contracts::id(&out.1["account"], "id").unwrap();
        let activate = json!({"activation_token":out.1["activation_token"],"password":PASSWORD});
        let activated = api(
            &self.router,
            "activateAccount",
            &[],
            None,
            activate.clone(),
            "activate",
            None,
            "",
        )
        .await;
        assert_eq!(activated.0, 200, "{}", activated.1);
        assert_eq!(
            api(
                &self.router,
                "activateAccount",
                &[],
                None,
                activate,
                "activate",
                None,
                ""
            )
            .await
            .0,
            401
        );
        let session = api(
            &self.router,
            "login",
            &[],
            None,
            json!({"account_id":account,"password":PASSWORD}),
            "login",
            None,
            "",
        )
        .await;
        assert_eq!(session.0, 200, "{}", session.1);
        (account, session.1["access_token"].as_str().unwrap().into())
    }
    async fn key(&self, account: Uuid, token: &str, inheritance: Value) -> Value {
        let out = self
            .call(
                "createApiKey",
                &[("account_id", account.to_string())],
                token,
                key_body(inheritance),
                None,
            )
            .await;
        assert_eq!(out.0, 201, "{}", out.1);
        out.1
    }
    async fn check(&self, token: &str, action: &str, resource: Value) -> bool {
        let out = self
            .call(
                "checkAuthorization",
                &[],
                token,
                json!({"action":action,"resource":resource}),
                None,
            )
            .await;
        assert_eq!(out.0, 200, "{}", out.1);
        out.1["allowed"].as_bool().unwrap()
    }
    async fn root_guest(&self) -> (Uuid, String) {
        // Trusted host fixture, deliberately no management API for creating roots.
        let id = Uuid::new_v4();
        let hash = auth::password_hash(PASSWORD).unwrap();
        let c = self.db.connect().await.unwrap();
        c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','Guest',$2,now(),now())", &[&id,&hash]).await.unwrap();
        c.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&id])
            .await
            .unwrap();
        c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2", &[&id,&self.owner]).await.unwrap();
        let grant = self
            .call(
                "createGrant",
                &[("account_id", id.to_string())],
                &self.token,
                json!({"source":"role","role_id":"Guest","scope":cap()}),
                None,
            )
            .await;
        assert_eq!(grant.0, 201, "{}", grant.1);
        let session = api(
            &self.router,
            "login",
            &[],
            None,
            json!({"account_id":id,"password":PASSWORD}),
            "guest-login",
            None,
            "",
        )
        .await;
        assert_eq!(session.0, 200, "{}", session.1);
        (id, session.1["access_token"].as_str().unwrap().into())
    }
}
fn cap() -> Value {
    json!({"kind":"capability","match":"ids","ids":["org.xelab.echo@1.0.0"]})
}
fn selected() -> Value {
    json!({"mode":"selected","rules":[{"action":"capability.invoke","resource":cap()}]})
}
fn key_body(inheritance: Value) -> Value {
    json!({"name":"acceptance key","inheritance":inheritance,"conditions":{"expires_at":(chrono::Utc::now()+chrono::Duration::days(30)).date_naive().and_hms_opt(0,0,0).unwrap().and_utc()}})
}

#[allow(clippy::too_many_arguments)]
async fn api(
    router: &Router,
    operation: &str,
    params: &[(&str, String)],
    token: Option<&str>,
    body: Value,
    key: &str,
    revision: Option<i64>,
    query: &str,
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
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
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
async fn auth_01_02_guest_self_service_and_group_cannot_escape_parent() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (guest, token) = f.root_guest().await;
    assert_eq!(
        f.call("getNode", &[], &token, Value::Null, None).await.0,
        404
    );
    let (child, child_token) = f.child(guest, &token, selected()).await;
    assert!(f.check(&child_token, "capability.invoke", cap()).await);
    let key = f.key(child, &child_token, selected()).await;
    assert!(
        !f.check(
            key["secret"].as_str().unwrap(),
            "credential.api-key.create",
            json!({"kind":"account","match":"ids","ids":[child]})
        )
        .await,
        "a Key does not gain fixed self-service"
    );
    let group = f
        .call(
            "createGroup",
            &[],
            &child_token,
            json!({"name":"echo only","rules":[{"action":"capability.invoke","resource":cap()}]}),
            None,
        )
        .await;
    assert_eq!(group.0, 201, "{}", group.1);
    let broader=f.call("createGroup",&[],&child_token,json!({"name":"escalation","rules":[{"action":"platform.read","resource":{"kind":"platform","match":"all"}}]}),None).await;
    assert_eq!(broader.0, 403, "{}", broader.1);
    let bind = f
        .call(
            "createGrant",
            &[("account_id", child.to_string())],
            &f.token,
            json!({"source":"role","role_id":"Owner","scope":{"kind":"platform","match":"all"}}),
            None,
        )
        .await;
    assert_eq!(bind.0, 201, "{}", bind.1);
    assert!(
        !f.check(
            &child_token,
            "platform.read",
            json!({"kind":"platform","match":"ids","ids":["node"]})
        )
        .await
    );
    let c = f.db.connect().await.unwrap();
    let raw: Value = c
        .query_one("SELECT to_jsonb(a) FROM accounts a WHERE id=$1", &[&child])
        .await
        .unwrap()
        .get(0);
    assert_eq!(raw["parent_id"], guest.to_string());
    assert_eq!(raw["independent_owner"], false);
    assert!(raw["activation_digest"].is_null());
}

#[tokio::test]
async fn auth_03_snapshot_follow_and_independent_provenance() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (guest, token) = f.root_guest().await;
    let fixed = f.key(guest, &token, json!({"mode":"all_snapshot"})).await;
    let follow = f.key(guest, &token, json!({"mode":"follow_parent"})).await;
    let grant = f
        .call(
            "createGrant",
            &[("account_id", guest.to_string())],
            &f.token,
            json!({"source":"role","role_id":"Auditor","scope":{"kind":"platform","match":"all"}}),
            None,
        )
        .await;
    assert_eq!(grant.0, 201, "{}", grant.1);
    let resource = json!({"kind":"platform","match":"ids","ids":["node"]});
    assert!(
        !f.check(
            fixed["secret"].as_str().unwrap(),
            "platform.read",
            resource.clone()
        )
        .await
    );
    assert!(
        f.check(
            follow["secret"].as_str().unwrap(),
            "platform.read",
            resource
        )
        .await
    );
    // Two independent live grant paths support the same frozen capability.
    let extra = f
        .call(
            "createGrant",
            &[("account_id", guest.to_string())],
            &f.token,
            json!({"source":"role","role_id":"Guest","scope":cap()}),
            None,
        )
        .await;
    assert_eq!(extra.0, 201, "{}", extra.1);
    let both = f.key(guest, &token, selected()).await;
    let all = f
        .call(
            "listGrants",
            &[("account_id", guest.to_string())],
            &token,
            Value::Null,
            None,
        )
        .await;
    assert_eq!(all.0, 200, "{}", all.1);
    let grants: Vec<Value> = all.1["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["spec"]["role_id"] == "Guest")
        .cloned()
        .collect();
    assert_eq!(grants.len(), 2);
    let revoked = f
        .call(
            "revokeGrant",
            &[("grant_id", grants[0]["id"].as_str().unwrap().into())],
            &f.token,
            Value::Null,
            Some(1),
        )
        .await;
    assert_eq!(revoked.0, 204, "{}", revoked.1);
    assert!(
        f.check(both["secret"].as_str().unwrap(), "capability.invoke", cap())
            .await
    );
    assert_eq!(
        f.call(
            "revokeGrant",
            &[("grant_id", grants[1]["id"].as_str().unwrap().into())],
            &f.token,
            Value::Null,
            Some(1)
        )
        .await
        .0,
        204
    );
    assert!(
        !f.check(both["secret"].as_str().unwrap(), "capability.invoke", cap())
            .await
    );
    let replacement = f
        .call(
            "createGrant",
            &[("account_id", guest.to_string())],
            &f.token,
            json!({"source":"role","role_id":"Guest","scope":cap()}),
            None,
        )
        .await;
    assert_eq!(replacement.0, 201, "{}", replacement.1);
    assert!(
        !f.check(both["secret"].as_str().unwrap(), "capability.invoke", cap())
            .await,
        "a new grant cannot revive a withdrawn snapshot source"
    );
    assert!(
        f.check(
            follow["secret"].as_str().unwrap(),
            "capability.invoke",
            cap()
        )
        .await
    );
}

#[tokio::test]
async fn auth_04_05_06_source_revocation_idempotency_and_secret_storage() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (guest, token) = f.root_guest().await;
    let k1 = f.key(guest, &token, json!({"mode":"all_snapshot"})).await;
    let k2 = f
        .key(guest, k1["secret"].as_str().unwrap(), selected())
        .await;
    let independent = f.key(guest, &token, selected()).await;
    let (child, child_token) = f
        .child(guest, k1["secret"].as_str().unwrap(), selected())
        .await;
    let params = [("account_id", guest.to_string())];
    let body = key_body(selected());
    let first = api(
        &f.router,
        "createApiKey",
        &params,
        Some(&token),
        body.clone(),
        "same-one-time-key",
        None,
        "",
    )
    .await;
    assert_eq!(first.0, 201, "{}", first.1);
    let replay = api(
        &f.router,
        "createApiKey",
        &params,
        Some(&token),
        body,
        "same-one-time-key",
        None,
        "",
    )
    .await;
    assert_eq!(replay.0, 409, "{}", replay.1);
    assert_eq!(replay.1["error"]["code"], "ONE_TIME_RESPONSE_UNAVAILABLE");
    assert_eq!(
        replay.1["error"]["resource_id"],
        first.1["credential"]["id"]
    );
    let c = f.db.connect().await.unwrap();
    let record = c
        .query_one(
            "SELECT sensitive,safe_response FROM idempotency_records WHERE key='same-one-time-key'",
            &[],
        )
        .await
        .unwrap();
    assert!(record.get::<_, bool>(0));
    assert!(record.get::<_, Option<Value>>(1).is_none());
    let persisted: String = c
        .query_one(
            "SELECT string_agg(to_jsonb(k)::text,'') FROM credentials k",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!persisted.contains(first.1["secret"].as_str().unwrap()));
    let revoke = f
        .call(
            "revokeApiKey",
            &[(
                "credential_id",
                k1["credential"]["id"].as_str().unwrap().into(),
            )],
            &token,
            Value::Null,
            Some(1),
        )
        .await;
    assert_eq!(revoke.0, 204, "{}", revoke.1);
    for secret in [
        k1["secret"].as_str().unwrap(),
        k2["secret"].as_str().unwrap(),
        &child_token,
    ] {
        assert_eq!(
            f.call(
                "checkAuthorization",
                &[],
                secret,
                json!({"action":"capability.invoke","resource":cap()}),
                None
            )
            .await
            .0,
            401
        );
    }
    assert!(
        f.check(
            independent["secret"].as_str().unwrap(),
            "capability.invoke",
            cap()
        )
        .await
    );
    // Archiving retains the original parent and source; no orphan promotion.
    let value = f
        .call(
            "getAccount",
            &[("account_id", child.to_string())],
            &f.token,
            Value::Null,
            None,
        )
        .await;
    let archive = f
        .call(
            "archiveAccount",
            &[("account_id", child.to_string())],
            &f.token,
            Value::Null,
            value.1["revision"].as_i64(),
        )
        .await;
    assert_eq!(archive.0, 204, "{}", archive.1);
    assert_eq!(
        c.query_one("SELECT parent_id FROM accounts WHERE id=$1", &[&child])
            .await
            .unwrap()
            .get::<_, Uuid>(0),
        guest
    );
}

#[tokio::test]
async fn auth_07_08_object_visibility_last_owner_and_stale_etag() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (_, guest) = f.root_guest().await;
    let secret = f.key(f.owner, &f.token, selected()).await;
    assert_eq!(
        f.call(
            "getApiKey",
            &[(
                "credential_id",
                secret["credential"]["id"].as_str().unwrap().into()
            )],
            &guest,
            Value::Null,
            None
        )
        .await
        .0,
        404
    );
    let p = [("account_id", f.owner.to_string())];
    assert_eq!(
        f.call(
            "updateAccount",
            &p,
            &f.token,
            json!({"status":"disabled"}),
            Some(1)
        )
        .await
        .0,
        409
    );
    assert_eq!(
        f.call("archiveAccount", &p, &f.token, Value::Null, Some(1))
            .await
            .0,
        409
    );
    let ok = f
        .call(
            "updateAccount",
            &p,
            &f.token,
            json!({"display_name":"renamed"}),
            Some(1),
        )
        .await;
    assert_eq!(ok.0, 200, "{}", ok.1);
    assert_eq!(
        f.call(
            "updateAccount",
            &p,
            &f.token,
            json!({"display_name":"stale"}),
            Some(1)
        )
        .await
        .0,
        412
    );
    let page = api(
        &f.router,
        "listAccounts",
        &[],
        Some(&f.token),
        Value::Null,
        "list",
        None,
        "limit=1",
    )
    .await;
    assert_eq!(page.0, 200, "{}", page.1);
    let cursor = page.1["next_cursor"].as_str().unwrap();
    let query = format!("limit=1&cursor={cursor}");
    assert_eq!(
        api(
            &f.router,
            "listAccounts",
            &[],
            Some(&guest),
            Value::Null,
            "list",
            None,
            &query
        )
        .await
        .0,
        400
    );
    let next = api(
        &f.router,
        "listAccounts",
        &[],
        Some(&f.token),
        Value::Null,
        "list",
        None,
        &query,
    )
    .await;
    assert_eq!(next.0, 200, "{}", next.1);
    assert_ne!(page.1["items"][0]["id"], next.1["items"][0]["id"]);
}

#[tokio::test]
async fn auth_09_concurrent_tree_and_key_quotas_are_atomic() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let p = [("account_id", f.owner.to_string())];
    let quota = json!({"maximum_depth":4,"maximum_descendants":1,"maximum_api_keys":1,"maximum_concurrent_calls":16,"daily_calls":10000});
    let out = f.call("updateQuota", &p, &f.token, quota, Some(1)).await;
    assert_eq!(out.0, 200, "{}", out.1);
    let body = json!({"display_name":"racer","kind":"service","inheritance":selected()});
    let (a, b) = tokio::join!(
        f.call("createChildAccount", &p, &f.token, body.clone(), None),
        f.call("createChildAccount", &p, &f.token, body, None)
    );
    let mut statuses = vec![a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, vec![201, 409], "{a:?} {b:?}");
    let body = key_body(selected());
    let (a, b) = tokio::join!(
        f.call("createApiKey", &p, &f.token, body.clone(), None),
        f.call("createApiKey", &p, &f.token, body, None)
    );
    let mut statuses = vec![a.0, b.0];
    statuses.sort();
    assert_eq!(statuses, vec![201, 409], "{a:?} {b:?}");
    let c = f.db.connect().await.unwrap();
    let q = c
        .query_one(
            "SELECT descendants_used,api_keys_used FROM quotas WHERE account_id=$1",
            &[&f.owner],
        )
        .await
        .unwrap();
    assert_eq!((q.get::<_, i64>(0), q.get::<_, i64>(1)), (1, 1));
}

#[tokio::test]
async fn key_rotation_password_and_parent_disable_preserve_boundaries() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (guest, token) = f.root_guest().await;
    let (child, child_token) = f.child(guest, &token, selected()).await;
    let old = f.key(child, &child_token, selected()).await;
    let body = key_body(selected());
    let rotate = f
        .call(
            "rotateApiKey",
            &[(
                "credential_id",
                old["credential"]["id"].as_str().unwrap().into(),
            )],
            &child_token,
            body,
            Some(1),
        )
        .await;
    assert_eq!(rotate.0, 201, "{}", rotate.1);
    assert!(
        f.check(
            rotate.1["secret"].as_str().unwrap(),
            "capability.invoke",
            cap()
        )
        .await
    );
    assert_eq!(
        f.call(
            "checkAuthorization",
            &[],
            old["secret"].as_str().unwrap(),
            json!({"action":"capability.invoke","resource":cap()}),
            None
        )
        .await
        .0,
        401
    );
    let row = f
        .call(
            "getAccount",
            &[("account_id", guest.to_string())],
            &f.token,
            Value::Null,
            None,
        )
        .await;
    let disabled = f
        .call(
            "updateAccount",
            &[("account_id", guest.to_string())],
            &f.token,
            json!({"status":"disabled"}),
            row.1["revision"].as_i64(),
        )
        .await;
    assert_eq!(disabled.0, 200, "{}", disabled.1);
    assert_eq!(
        f.call(
            "getAccount",
            &[("account_id", child.to_string())],
            &child_token,
            Value::Null,
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(
        f.call(
            "updateAccount",
            &[("account_id", guest.to_string())],
            &f.token,
            json!({"status":"active"}),
            disabled.1["revision"].as_i64()
        )
        .await
        .0,
        200
    );
    let row = f
        .call(
            "getAccount",
            &[("account_id", child.to_string())],
            &child_token,
            Value::Null,
            None,
        )
        .await;
    let changed = f
        .call(
            "changePassword",
            &[("account_id", child.to_string())],
            &child_token,
            json!({"current_password":PASSWORD,"new_password":"changed acceptance password"}),
            row.1["revision"].as_i64(),
        )
        .await;
    assert_eq!(changed.0, 204, "{}", changed.1);
    assert!(
        f.check(
            rotate.1["secret"].as_str().unwrap(),
            "capability.invoke",
            cap()
        )
        .await,
        "password/session revocation does not revoke independently issued keys"
    );
    assert_eq!(
        f.call(
            "getAccount",
            &[("account_id", child.to_string())],
            &child_token,
            Value::Null,
            None
        )
        .await
        .0,
        401
    );
}
