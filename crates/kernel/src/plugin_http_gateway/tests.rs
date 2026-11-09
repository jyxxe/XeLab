use super::*;
use tower::ServiceExt;

fn config() -> Config {
    let mut value = serde_json::to_value(
        toml::from_str::<toml::Value>(include_str!(
            "../../../../docs/contracts/examples/kernel.toml"
        ))
        .unwrap(),
    )
    .unwrap();
    value["ui_gateway"] = json!({"base_domain":"plugins.example","public_port":9443,"shell_origins":["https://desktop.example","https://other-desktop.example"]});
    Config::from_value(value).unwrap()
}
#[test]
fn exact_allocated_host_precedes_trusted_route_selection() {
    let config = config();
    let request = Request::builder()
        .uri("/api/platform/v1/auth/login")
        .header("host", core_authority(&config).unwrap())
        .body(Body::empty())
        .unwrap();
    assert_eq!(host(&config, &request).unwrap(), Host::Core);
    let deployment = Uuid::new_v4();
    let name = format!("d-{}.plugins.example:9443", deployment.simple());
    let request = Request::builder()
        .uri("/auth/ui/login")
        .header("host", &name)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        host(&config, &request).unwrap(),
        Host::Deployment(deployment)
    );
    for name in [
        "evil.example".to_owned(),
        format!("{name}.evil.example"),
        format!("d-{}.plugins.example", deployment.simple()),
        format!("d-{}.plugins.example:9443", deployment.hyphenated()),
    ] {
        let request = Request::builder()
            .uri("/health/live")
            .header("host", name)
            .body(Body::empty())
            .unwrap();
        assert!(host(&config, &request).is_err());
    }
    let request = Request::builder()
        .uri("/health/live")
        .body(Body::empty())
        .unwrap();
    assert!(host(&config, &request).is_err());
    for name in ["forwarded", "x-forwarded-host", "x-forwarded-proto"] {
        let request = Request::builder()
            .uri("/health/live")
            .header("host", core_authority(&config).unwrap())
            .header(name, "evil.example")
            .body(Body::empty())
            .unwrap();
        assert!(host(&config, &request).is_err());
    }
    let mut request = Request::builder()
        .uri("/health/live")
        .header("host", core_authority(&config).unwrap())
        .body(Body::empty())
        .unwrap();
    request
        .headers_mut()
        .append("host", HeaderValue::from_static("evil.example"));
    assert!(host(&config, &request).is_err());
    let request = Request::builder()
        .uri("https://evil.example/health/live")
        .header("host", core_authority(&config).unwrap())
        .body(Body::empty())
        .unwrap();
    assert!(host(&config, &request).is_err());
    let policy = StaticOrigin::for_deployment(&config, deployment, None).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("text/html"));
    let protected = crate::plugin_http::transport::static_response(
        &Method::GET,
        StatusCode::OK,
        &headers,
        b"safe",
        &policy,
    )
    .unwrap();
    let csp = protected["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("frame-ancestors https://desktop.example https://other-desktop.example"));
    assert!(csp.contains("worker-src 'none'"));
}
#[test]
fn csrf_proof_is_bound_to_one_session_and_domain() {
    let auth = auth::Auth::new(vec![42; 32]).unwrap();
    let first = "01234567890123456789012345678901";
    let second = "01234567890123456789012345678902";
    let proof = auth.csrf_token(first);
    auth.verify_csrf(first, &proof).unwrap();
    assert!(auth.verify_csrf(second, &proof).is_err());
    assert!(auth.verify_csrf(first, first).is_err());
    assert!(auth.verify_csrf(first, "malformed").is_err());
    use base64::Engine;
    let verification =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(auth.token_digest(first));
    assert!(auth.verify_csrf(first, &verification).is_err());
}

#[tokio::test]
async fn cookie_origin_csrf_and_mixed_principals_are_checked() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let peer = "127.0.0.1".parse().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        HeaderValue::from_str(&format!("__Host-xelab_auth={}", f.token)).unwrap(),
    );
    assert!(
        request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
            .await
            .is_err()
    );
    headers.insert(
        "origin",
        HeaderValue::from_str(f.app.config.text("server", "public_origin")).unwrap(),
    );
    assert!(
        request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
            .await
            .is_err()
    );
    headers.insert(
        "x-csrf-token",
        HeaderValue::from_str(&f.app.auth.csrf_token(&f.token)).unwrap(),
    );
    request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
        .await
        .unwrap();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {}", f.token)).unwrap(),
    );
    request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
        .await
        .unwrap();
    let connection = f.app.db.connect().await.unwrap();
    let other = Uuid::new_v4();
    connection.execute("INSERT INTO accounts(id,kind,status,display_name,independent_owner,password_hash,created_at,updated_at) SELECT $1,'user','active','Other fixture account',true,password_hash,now(),now() FROM accounts WHERE independent_owner LIMIT 1",&[&other]).await.unwrap();
    connection
        .execute(
            "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
            &[&other],
        )
        .await
        .unwrap();
    connection.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) VALUES($1,$2,current_date,now(),now())",&[&other,&f.app.config.value["quota_defaults"]]).await.unwrap();
    let issued = f
        .app
        .auth
        .login(
            &connection,
            other,
            "correct horse backup fixture password".into(),
            peer,
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    headers.insert(
        "authorization",
        HeaderValue::from_str(&format!(
            "Bearer {}",
            issued["access_token"].as_str().unwrap()
        ))
        .unwrap(),
    );
    assert_eq!(
        request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
            .await
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    headers.remove("authorization");
    headers.append("origin", HeaderValue::from_static("https://evil.example"));
    assert!(
        request_security(&f.app, peer, Uuid::new_v4(), &Method::POST, &headers)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn route_approval_revalidates_original_proof_and_exact_manifest() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let connection = f.app.db.connect().await.unwrap();
    let row = connection
        .query_one(
            "SELECT d.id,d.owner_id,d.installation_id FROM deployments d LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let deployment: Uuid = row.get("id");
    let owner: Uuid = row.get("owner_id");
    let installation: Uuid = row.get("installation_id");
    let manifest = json!({"plugin_id":"org.xelab.gateway-fixture","routes":[{"path_prefix":"/ui/org.xelab.gateway-fixture/","kind":"static","methods":["GET","HEAD"],"required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":1000,"authentication":"public"},{"path_prefix":"/plugins/org.xelab.gateway-fixture/assets/","kind":"static","methods":["GET","HEAD"],"required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":1000,"authentication":"public"}]});
    let approval = json!({"rules":[],"egress":[]});
    connection
        .execute(
            "UPDATE installations SET manifest=$2 WHERE id=$1",
            &[&installation, &manifest],
        )
        .await
        .unwrap();
    connection
        .execute(
            "UPDATE deployments SET approval=$2 WHERE id=$1",
            &[&deployment, &approval],
        )
        .await
        .unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(
            &connection,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    assert!(check_approval(&connection, &f.app, deployment, &manifest)
        .await
        .is_err());
    approve(&connection, &ctx, deployment, &manifest, &approval, owner)
        .await
        .unwrap();
    assert!(declared_static(
        &f.app,
        deployment,
        &Method::GET,
        "/plugins/org.xelab.gateway-fixture/assets/index.html"
    )
    .await
    .unwrap());
    assert!(!declared_static(
        &f.app,
        deployment,
        &Method::GET,
        "/plugins/org.xelab.gateway-fixture/business"
    )
    .await
    .unwrap());
    check_approval(&connection, &f.app, deployment, &manifest)
        .await
        .unwrap();
    let mut changed = manifest.clone();
    changed["routes"][0]["path_prefix"] = json!("/ui/org.xelab.gateway-fixture/new/");
    assert!(check_approval(&connection, &f.app, deployment, &changed)
        .await
        .is_err());
    let old:Uuid=connection.query_one("SELECT id FROM grants WHERE subject_id=$1 AND scope->>'kind'='deployment' AND status='active'",&[&owner]).await.unwrap().get(0);
    connection
        .execute("UPDATE grants SET status='revoked' WHERE id=$1", &[&old])
        .await
        .unwrap();
    let replacement = Uuid::new_v4();
    connection.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$2,'Owner','{\"kind\":\"deployment\",\"match\":\"all\"}','active',now(),now())",&[&replacement,&owner]).await.unwrap();
    // Model an independently established recovery Owner anchor. Merely
    // self-issuing a replacement binding correctly cannot restore authority.
    connection
        .execute(
            "INSERT INTO recovery_owner_anchors(grant_id,account_id,recovery_id) VALUES($1,$2,$3)",
            &[&replacement, &owner, &Uuid::new_v4()],
        )
        .await
        .unwrap();
    auth::authorize(
        &connection,
        &ctx,
        "plugin.permission.grant",
        &Resource::new("deployment", deployment, Some(owner)),
    )
    .await
    .unwrap();
    assert!(check_approval(&connection, &f.app, deployment, &manifest)
        .await
        .is_err());
    approve(&connection, &ctx, deployment, &manifest, &approval, owner)
        .await
        .unwrap();
    check_approval(&connection, &f.app, deployment, &manifest)
        .await
        .unwrap();
}

#[tokio::test]
async fn plugin_host_never_serves_trusted_login_or_health() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let mut value = f.app.config.value.clone();
    value["ui_gateway"] = json!({"base_domain":"plugins.example","public_port":443,"shell_origins":["https://desktop.example"]});
    let config = Config::from_value(value).unwrap();
    let app = Arc::new(App {
        config,
        db: f.app.db.clone(),
        auth: f.app.auth.clone(),
        runtime: f.app.runtime.clone(),
        leader: f.app.leader.clone(),
        ready: std::sync::atomic::AtomicBool::new(true),
        shutdown: std::sync::atomic::AtomicBool::new(false),
        blob_uploads: Semaphore::new(4),
    });
    let router = api::router(app.clone());
    for (method, path) in [
        ("GET", "/health/live"),
        ("POST", "/api/platform/v1/auth/login"),
        ("GET", "/auth/ui/login"),
        ("GET", "/oauth/authorize"),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(
                "host",
                format!("d-{}.plugins.example", Uuid::new_v4().simple()),
            )
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo(
            "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
        ));
        let response = router.clone().oneshot(request).await.unwrap();
        assert!(!response.status().is_success());
        assert!(!response.headers().contains_key("set-cookie"));
        assert!(!response.headers().contains_key("x-csrf-token"));
    }
}

#[tokio::test]
async fn fresh_password_login_recovers_from_revoked_cookie_without_using_its_authority() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let connection = f.app.db.connect().await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(
            &connection,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    connection
        .execute(
            "UPDATE credentials SET status='revoked' WHERE id=$1",
            &[&ctx.credential],
        )
        .await
        .unwrap();
    let path = crate::contracts::openapi()["paths"]
        .as_object()
        .unwrap()
        .iter()
        .find_map(|(path, item)| {
            item.get("post")
                .filter(|operation| operation["operationId"] == "login")
                .map(|_| path)
        })
        .unwrap();
    let router = api::router(f.app.clone());
    let mut request=Request::builder().method("POST").uri(path).header("host",core_authority(&f.app.config).unwrap()).header("origin",f.app.config.text("server","public_origin")).header("cookie",format!("__Host-xelab_auth={}",f.token)).header("x-csrf-token","stale-form-proof").header("content-type","application/json").body(Body::from(serde_json::to_vec(&json!({"account_id":ctx.subject,"password":"correct horse backup fixture password"})).unwrap())).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo(
        "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
    ));
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .to_owned();
    let csrf = response.headers()["x-csrf-token"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    let token = value["access_token"].as_str().unwrap();
    assert!(cookie.starts_with(&format!("__Host-xelab_auth={token};")));
    assert_ne!(token, f.token);
    f.app.auth.verify_csrf(token, &csrf).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("https://evil.example"));
    assert!(fresh_credential_security(&f.app, &headers).is_err());
}

#[tokio::test]
async fn static_slot_counts_rpc_and_http_leases_and_abandonment_is_not_released() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let connection = f.app.db.connect().await.unwrap();
    let row = connection
        .query_one(
            "SELECT d.id,d.owner_id,d.installation_id FROM deployments d LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let deployment: Uuid = row.get("id");
    let owner: Uuid = row.get("owner_id");
    let installation: Uuid = row.get("installation_id");
    let manifest = json!({"plugin_id":"org.xelab.gateway-fixture","routes":[{"path_prefix":"/ui/org.xelab.gateway-fixture/","kind":"static","methods":["GET","HEAD"],"required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":10000,"authentication":"public"}],"health":{"interval_ms":5000,"failure_threshold":3,"timeout_ms":2000},"concurrency":{"max_active_requests":1,"max_queue_requests":0,"queue_timeout_ms":100}});
    let approval = json!({"rules":[],"egress":[]});
    connection
        .execute(
            "UPDATE installations SET manifest=$2 WHERE id=$1",
            &[&installation, &manifest],
        )
        .await
        .unwrap();
    connection.execute("UPDATE deployments SET approval=$2,spec='{\"enabled\":true}',phase='ready',observed_generation=1 WHERE id=$1",&[&deployment,&approval]).await.unwrap();
    let instance = Uuid::new_v4();
    connection.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())",&[&instance,&deployment]).await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(
            &connection,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    approve(&connection, &ctx, deployment, &manifest, &approval, owner)
        .await
        .unwrap();
    let mut value = f.app.config.value.clone();
    value["ui_gateway"] = json!({"base_domain":"plugins.example","public_port":443,"shell_origins":["https://desktop.example"]});
    let config = Config::from_value(value).unwrap();
    let app = Arc::new(App {
        config,
        db: f.app.db.clone(),
        auth: f.app.auth.clone(),
        runtime: f.app.runtime.clone(),
        leader: f.app.leader.clone(),
        ready: std::sync::atomic::AtomicBool::new(true),
        shutdown: std::sync::atomic::AtomicBool::new(false),
        blob_uploads: Semaphore::new(4),
    });
    let call = Uuid::new_v4();
    connection.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,'{}','fixture','running',now()+interval '30 seconds',now())",&[&call,&owner,&ctx.credential,&instance]).await.unwrap();
    let error = static_page(
        app.clone(),
        ctx.peer,
        Uuid::new_v4(),
        deployment,
        &Method::GET,
        "/ui/org.xelab.gateway-fixture/index.html",
        &HeaderMap::new(),
        tokio::time::Instant::now(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "RESOURCE_EXHAUSTED");
    connection
        .execute("UPDATE calls SET state='succeeded' WHERE id=$1", &[&call])
        .await
        .unwrap();
    let id = Uuid::new_v4();
    connection.execute("INSERT INTO plugin_http_requests(id,instance_id,generation,expires_at,state) VALUES($1,$2,1,now()+interval '30 seconds','active')",&[&id,&instance]).await.unwrap();
    let error = static_page(
        app.clone(),
        ctx.peer,
        Uuid::new_v4(),
        deployment,
        &Method::GET,
        "/ui/org.xelab.gateway-fixture/index.html",
        &HeaderMap::new(),
        tokio::time::Instant::now(),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "RESOURCE_EXHAUSTED");
    drop(StaticLease::new(
        app.clone(),
        id,
        deployment,
        tokio::time::Instant::now(),
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state: String = connection
                .query_one("SELECT state FROM plugin_http_requests WHERE id=$1", &[&id])
                .await
                .unwrap()
                .get(0);
            if state == "unknown" {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        connection
            .query_one(
                "SELECT count(*) FROM plugin_http_requests WHERE id=$1",
                &[&id]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    let mut completed = StaticLease::new(app, id, deployment, tokio::time::Instant::now());
    assert!(completed.complete().await.is_err());
    assert_eq!(
        connection
            .query_one(
                "SELECT count(*) FROM plugin_http_requests WHERE id=$1",
                &[&id]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
}

#[tokio::test]
async fn full_ingress_pool_refuses_before_credentials_or_body_collection() {
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let pool = Arc::new(Semaphore::new(1));
    let held = pool.clone().acquire_owned().await.unwrap();
    let body = Body::from_stream(futures_util::stream::once(async {
        panic!("a refused request body must not be polled");
        #[allow(unreachable_code)]
        Ok::<_, Error>(Bytes::new())
    }));
    let request = Request::builder()
        .method("POST")
        .uri("/plugins/org.xelab.gateway-fixture/echo")
        .body(body)
        .unwrap();
    let error = handle_with_ingress(
        f.app.clone(),
        "127.0.0.1:30000".parse().unwrap(),
        Uuid::new_v4(),
        Host::Core,
        request,
        pool.clone(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "RESOURCE_EXHAUSTED");
    assert_eq!(pool.available_permits(), 0);
    drop(held);
    let request = Request::builder()
        .method("POST")
        .uri("/plugins/org.xelab.gateway-fixture/echo")
        .body(Body::empty())
        .unwrap();
    assert!(handle_with_ingress(
        f.app.clone(),
        "127.0.0.1:30000".parse().unwrap(),
        Uuid::new_v4(),
        Host::Core,
        request,
        pool.clone()
    )
    .await
    .is_err());
    assert_eq!(pool.available_permits(), 1);
}

// Actual PostgreSQL lock scheduling and the independent reference HTTPS
// listener; installation/readiness are metadata fixtures, not OCI evidence.
#[tokio::test]
async fn static_transport_progresses_while_authority_waits_and_rechecks_before_delivery() {
    for revoke in [false, true] {
        static_authority_transport_barrier(revoke).await;
    }
}

async fn static_authority_transport_barrier(revoke: bool) {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType,
    };
    use std::{fs, os::unix::fs::PermissionsExt, process::Stdio};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let Some(f) = crate::backups::tests::Fixture::new().await else {
        return;
    };
    let root = f.app.config.root();
    let ca_key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["static-monitor-test-ca".into()]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = params.self_signed(&ca_key).unwrap();
    fs::write(root.join("monitor-ca.crt"), ca.pem()).unwrap();
    fs::write(root.join("monitor-ca.key"), ca_key.serialize_pem()).unwrap();
    fs::set_permissions(
        root.join("monitor-ca.key"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let mut value = f.app.config.value.clone();
    value["security"]["workload_ca_cert_file"] = json!(root.join("monitor-ca.crt"));
    value["security"]["workload_ca_key_file"] = json!(root.join("monitor-ca.key"));
    value["runtime"]["network_cidr"] = json!("127.41.0.0/16");
    value["server"]["host_rpc_listen"] = json!("127.41.0.1:7443");
    value["ui_gateway"] =
        json!({"base_domain":"static-monitor.example","public_port":443,"shell_origins":[]});
    let config = Config::from_value(value).unwrap();
    crate::tls::issue(&config, None, &root.join("kernel-identity")).unwrap();
    let app = Arc::new(App {
        config: config.clone(),
        db: f.app.db.clone(),
        auth: f.app.auth.clone(),
        runtime: crate::runtime::Runtime::new(config),
        leader: f.app.leader.clone(),
        ready: std::sync::atomic::AtomicBool::new(true),
        shutdown: std::sync::atomic::AtomicBool::new(false),
        blob_uploads: Semaphore::new(4),
    });
    let connection = app.db.connect().await.unwrap();
    let row = connection
        .query_one(
            "SELECT id,owner_id,installation_id FROM deployments LIMIT 1",
            &[],
        )
        .await
        .unwrap();
    let deployment: Uuid = row.get("id");
    let owner: Uuid = row.get("owner_id");
    let installation: Uuid = row.get("installation_id");
    let instance = Uuid::new_v4();
    let manifest = json!({"plugin_id":"org.xelab.http-echo","routes":[{"path_prefix":"/ui/org.xelab.http-echo/","kind":"static","methods":["GET","HEAD"],"required_action":"plugin.ui.open","maximum_body_bytes":0,"timeout_ms":10000,"authentication":"public"}],"health":{"interval_ms":5000,"failure_threshold":3,"timeout_ms":2000},"concurrency":{"max_active_requests":1,"max_queue_requests":0,"queue_timeout_ms":100}});
    let approval = json!({"rules":[],"egress":[]});
    connection
        .execute(
            "UPDATE installations SET manifest=$2 WHERE id=$1",
            &[&installation, &manifest],
        )
        .await
        .unwrap();
    connection.execute("UPDATE deployments SET approval=$2,spec='{\"enabled\":true}',phase='ready',observed_generation=1 WHERE id=$1", &[&deployment,&approval]).await.unwrap();
    connection.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,last_heartbeat_at,created_at,updated_at) VALUES($1,$2,1,0,'ready','alive','{}',now(),now(),now())", &[&instance,&deployment]).await.unwrap();
    let ctx = app
        .auth
        .authenticate(
            &connection,
            &f.token,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
        )
        .await
        .unwrap();
    approve(&connection, &ctx, deployment, &manifest, &approval, owner)
        .await
        .unwrap();
    let runtime = root.join("runtime").join(instance.to_string());
    let identity = runtime.join("identity");
    fs::create_dir_all(&identity).unwrap();
    fs::write(
        runtime.join("network.json"),
        serde_json::to_vec(&json!({"instance":instance,"peer":"127.41.0.2"})).unwrap(),
    )
    .unwrap();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![format!("i-{instance}-g-1.xelab.local")]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.subject_alt_names.push(SanType::URI(
        format!("spiffe://xelab.local/instance/{instance}/generation/1")
            .try_into()
            .unwrap(),
    ));
    let certificate = params.signed_by(&key, &ca, &ca_key).unwrap();
    fs::write(identity.join("tls.crt"), certificate.pem()).unwrap();
    fs::write(identity.join("tls.key"), key.serialize_pem()).unwrap();
    fs::write(identity.join("ca.crt"), ca.pem()).unwrap();
    // Gate the real TCP accept before TLS, without changing either the
    // production two-second connect timeout or ten-second signed route bound.
    let script = r#"
import sys
sys.path.insert(0,sys.argv[1])
from http_server import State,KernelHTTPServer,Handler
class Gate:
 def __init__(self,socket):self.socket=socket
 def __getattr__(self,name):return getattr(self.socket,name)
 def accept(self):
  value=self.socket.accept()
  print('ACCEPTED',flush=True)
  if sys.stdin.readline()!='GO\n':raise RuntimeError('barrier closed')
  return value
original=Handler.do_GET
def checked(self):
 print('HTTP_GET',flush=True)
 original(self)
 self.wfile.flush()
 print('BODY_EOF',flush=True)
Handler.do_GET=checked
state=State();state.initialize()
server=KernelHTTPServer(('127.41.0.2',8080),state,sys.argv[2])
server.socket=Gate(server.socket)
print('READY',flush=True)
server.serve_forever()
"#;
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/http");
    let mut server = tokio::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(examples)
        .arg(identity)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("READY")
    );
    let request_app = app.clone();
    let request = tokio::spawn(async move {
        static_page(
            request_app,
            "127.0.0.1".parse().unwrap(),
            Uuid::new_v4(),
            deployment,
            &Method::GET,
            "/ui/org.xelab.http-echo/index.html",
            &HeaderMap::new(),
            tokio::time::Instant::now(),
            None,
        )
        .await
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("ACCEPTED")
    );
    let mut writer = app
        .db
        .connect_scoped(Duration::from_secs(10))
        .await
        .unwrap();
    let writer_pid: i32 = writer
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let tx = writer.transaction().await.unwrap();
    db::auth_lock(&tx, true).await.unwrap();
    let monitor_pid: i32 = tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let queued = connection.query_opt("SELECT pid FROM pg_locks WHERE locktype='advisory' AND NOT granted AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1", &[&writer_pid]).await.unwrap();
            if let Some(row) = queued {break row.get(0);}
            tokio::task::yield_now().await;
        }
    }).await.expect("the static authority reader must actually queue behind the held PostgreSQL writer");
    let started = std::time::Instant::now();
    server
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"GO\n")
        .await
        .unwrap();
    // The writer remains held. The actual TLS client must continue its
    // handshake, send GET and receive a finite response despite that reader.
    let get = tokio::time::timeout(Duration::from_secs(2), lines.next_line()).await;
    assert!(
        get.is_ok(),
        "the queued authority read stopped polling the actual TLS/HTTP transport"
    );
    assert_eq!(get.unwrap().unwrap().as_deref(), Some("HTTP_GET"));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("BODY_EOF")
    );
    // Observe a different backend waiting on the completion fence: this also
    // proves the core client consumed EOF, rather than inferring that from the
    // server's completed write. Keep the writer held until that transition.
    tokio::time::timeout(Duration::from_secs(2),async {
        loop {
            let queued: bool = connection.query_one("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND pid<>$2 AND $1=ANY(pg_blocking_pids(pid)))", &[&writer_pid,&monitor_pid]).await.unwrap().get(0);
            if queued {break;}
            tokio::task::yield_now().await;
        }
    }).await.expect("core transport EOF must reach its completion fence while authority remains blocked");
    assert!(
        !request.is_finished(),
        "no response may be delivered while its complete authority check is blocked"
    );
    if revoke {
        tx.execute(
            "DELETE FROM plugin_http_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    if revoke {
        let error =
            response.expect_err("a completed upstream body must not bypass current route approval");
        assert_eq!(error.code, "PERMISSION_DENIED");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
    } else {
        let response = response.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
        assert_eq!(
            bytes.as_ref(),
            include_bytes!("../../../../examples/http/index.html")
        );
    }
    let leases: i64 = connection
        .query_one(
            "SELECT count(*) FROM plugin_http_requests WHERE instance_id=$1",
            &[&instance],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        leases, 0,
        "physical EOF releases the lease only after the original completion fence"
    );
    println!(
        "Actual PostgreSQL writer-held TLS/HTTP progress: revoke={revoke}, elapsed_ms={}",
        started.elapsed().as_millis()
    );
    server.kill().await.unwrap();
}
