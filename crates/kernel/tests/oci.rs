//! Privileged, disposable-host component test. It never lowers daemon preflight limits.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{pkcs8::EncodePublicKey, Signer, SigningKey};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use serde_json::{json, Value};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;
use xelab_kernel::{
    admission,
    api::{router, App},
    artifact,
    auth::{self, Auth},
    config::Config,
    contracts, controller,
    db::Db,
    runtime::Runtime,
    tls,
};

#[allow(clippy::too_many_arguments)] // Test request envelope.
async fn api(
    router: &Router,
    operation: &str,
    params: &[(&str, String)],
    token: Option<&str>,
    body: Value,
    key: &str,
    revision: Option<i64>,
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

async fn operation(router: &Router, id: &str, token: &str) -> Value {
    for _ in 0..120 {
        let (status, value) = api(
            router,
            "getOperation",
            &[("operation_id", id.into())],
            Some(token),
            Value::Null,
            "poll",
            None,
        )
        .await;
        assert_eq!(status, 200, "{value}");
        if value["state"] == "succeeded" {
            return value["result"].clone();
        }
        assert_ne!(value["state"], "failed", "{value}");
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("operation {id} did not finish");
}

#[tokio::test]
#[ignore = "requires root, containerd, nftables and a disposable Linux host; see README"]
async fn signed_oci_plugin_runs_through_the_management_api_and_recovers() {
    assert_eq!(std::env::var("XELAB_OCI_TEST").as_deref(), Ok("1"));
    assert_eq!(unsafe { libc::geteuid() }, 0);
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    xelab_kernel::init_crypto();
    let base = std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    admin
        .batch_execute("CREATE DATABASE xelab_oci")
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname=xelab_oci")).unwrap();
    db.migrate().await.unwrap();
    let directory = tempfile::tempdir().unwrap().keep();
    let root = directory.as_path();
    let key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(vec!["xelab-test-ca".into()]).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let cert = ca.self_signed(&key).unwrap();
    fs::write(root.join("ca.crt"), cert.pem()).unwrap();
    fs::write(root.join("ca.key"), key.serialize_pem()).unwrap();
    fs::set_permissions(root.join("ca.key"), fs::Permissions::from_mode(0o600)).unwrap();
    let t: toml::Value =
        toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
    let mut value = serde_json::to_value(t).unwrap();
    value["storage"]["root_path"] = json!(root.join("state"));
    value["security"]["workload_ca_cert_file"] = json!(root.join("ca.crt"));
    value["security"]["workload_ca_key_file"] = json!(root.join("ca.key"));
    value["security"]["master_key_file"] = json!(root.join("master.key"));
    fs::write(root.join("master.key"), [42; 32]).unwrap();
    fs::set_permissions(root.join("master.key"), fs::Permissions::from_mode(0o600)).unwrap();
    value["runtime"]["namespace"] = json!("xelab-ci");
    value["runtime"]["network_cidr"] = json!("172.29.240.0/24");
    value["server"]["host_rpc_listen"] = json!("172.29.240.1:7443");
    let config = Config::from_value(value).unwrap();
    let runtime = Runtime::new(config.clone());
    // Hosted CI is smaller than the production 4 CPU + 16 GiB kernel reservation.
    // Exercise real per-instance cgroups separately, with a synthetic metadata pool.
    assert!(admission::pool(&config.value["resources"], 4000, 17179869184).is_err());
    fs::create_dir_all("/sys/fs/cgroup/xelab/plugins").unwrap();
    for path in [
        "/sys/fs/cgroup/cgroup.subtree_control",
        "/sys/fs/cgroup/xelab/cgroup.subtree_control",
        "/sys/fs/cgroup/xelab/plugins/cgroup.subtree_control",
    ] {
        fs::write(path, "+cpu +memory +pids").unwrap();
    }
    fs::write("/sys/fs/cgroup/xelab/plugins/memory.max", "1073741824").unwrap();
    fs::write("/sys/fs/cgroup/xelab/plugins/cpu.max", "200000 100000").unwrap();
    fs::create_dir_all(config.root()).unwrap();
    runtime.prepare_host_service_network().await.unwrap();
    tls::issue(&config, None, &config.root().join("kernel-identity")).unwrap();
    let mut c = db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let owner = auth::bootstrap(
        &tx,
        "OCI test owner",
        "correct horse test password",
        &config.value["quota_defaults"],
    )
    .await
    .unwrap();
    admission::initialize(&tx, &config.value["resources"], 8000, 34359738368)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let publisher: Uuid = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let pem = signing
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','fixture',$2,'trusted',now(),now())",&[&publisher,&pem]).await.unwrap();
    let raw = fs::read("examples/generated/xelab/plugin.json").unwrap();
    let manifest: Value = serde_json::from_slice(&raw).unwrap();
    let image = std::env::var("XELAB_TEST_IMAGE").unwrap();
    let envelope =
        artifact::signature_envelope(&manifest, artifact::image_ref(&image).unwrap(), &raw)
            .unwrap();
    let install = json!({"publisher_id":publisher,"image":image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())});
    let mut bad = install.clone();
    bad["signature"] = json!(STANDARD.encode([0; 64]));
    assert!(artifact::Artifact::inspect(&runtime, &bad, &pem)
        .await
        .is_err());
    let app = Arc::new(App {
        config: config.clone(),
        db: db.clone(),
        auth: Auth::new(vec![42; 32]).unwrap(),
        runtime,
        leader: Arc::new(db.leader().await.unwrap()),
        ready: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
    });
    let mut worker = tokio::spawn(controller::run(app.clone()));
    let router = router(app.clone());
    let (status, session) = api(
        &router,
        "login",
        &[],
        None,
        json!({"account_id":owner,"password":"correct horse test password"}),
        "login",
        None,
    )
    .await;
    assert_eq!(status, 200, "{session}");
    let token = session["access_token"].as_str().unwrap();
    let (status, installed) = api(
        &router,
        "installPlugin",
        &[],
        Some(token),
        install.clone(),
        "install-1",
        None,
    )
    .await;
    assert_eq!(status, 202, "{installed}");
    let replay = api(
        &router,
        "installPlugin",
        &[],
        Some(token),
        install,
        "install-1",
        None,
    )
    .await;
    assert_eq!(replay.1["id"], installed["id"]);
    let installed = operation(&router, installed["id"].as_str().unwrap(), token).await;
    let limits =
        json!({"cpu_millis":500,"memory_bytes":134217728,"ephemeral_bytes":1048576,"pids":64});
    let (status,created)=api(&router,"createDeployment",&[],Some(token),json!({"installation_id":installed["installation_id"],"display_name":"echo","enabled":false,"limits":limits,"config":{},"volume_ids":[]}),"deploy-1",None).await;
    assert_eq!(status, 202, "{created}");
    let deployment = operation(&router, created["id"].as_str().unwrap(), token).await
        ["deployment_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let params = [("deployment_id", deployment.clone())];
    let approved = api(
        &router,
        "approvePlugin",
        &params,
        Some(token),
        json!({"rules":[],"egress":[]}),
        "approve-1",
        Some(1),
    )
    .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    let stale = api(
        &router,
        "setDeploymentEnabled",
        &params,
        Some(token),
        json!({"enabled":true}),
        "stale-enable",
        Some(1),
    )
    .await;
    assert_eq!(stale.0, 412, "{}", stale.1);
    // RUN-01: interrupt reconciliation before persisting the enable intent.
    worker.abort();
    let _ = worker.await;
    app.ready.store(false, Ordering::SeqCst);
    let enabled = api(
        &router,
        "setDeploymentEnabled",
        &params,
        Some(token),
        json!({"enabled":true}),
        "enable-1",
        Some(2),
    )
    .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    let intended: Uuid = c
        .query_one(
            "SELECT id FROM instances WHERE deployment_id=$1",
            &[&deployment.parse::<Uuid>().unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!app.runtime.inventory().await.unwrap().contains(&intended));
    worker = tokio::spawn(controller::run(app.clone()));
    operation(&router, enabled.1["id"].as_str().unwrap(), token).await;
    let input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"hello OCI"},"provider_deployment_id":deployment,"timeout_ms":10000});
    let called = api(
        &router,
        "invokeCapability",
        &[],
        Some(token),
        input.clone(),
        "invoke-1",
        None,
    )
    .await;
    assert_eq!(called.0, 200, "{}", called.1);
    assert_eq!(called.1["output"]["message"], "hello OCI");
    let replay = api(
        &router,
        "invokeCapability",
        &[],
        Some(token),
        input.clone(),
        "invoke-1",
        None,
    )
    .await;
    assert_eq!(replay.1, called.1);
    let mut different = input.clone();
    different["input"]["message"] = json!("different");
    assert_eq!(
        api(
            &router,
            "invokeCapability",
            &[],
            Some(token),
            different,
            "invoke-1",
            None
        )
        .await
        .0,
        409
    );
    let call_id = called.1["call_id"].as_str().unwrap();
    assert_eq!(
        api(
            &router,
            "getCall",
            &[("call_id", call_id.into())],
            Some(token),
            Value::Null,
            "read",
            None
        )
        .await
        .1["state"],
        "succeeded"
    );
    let instance: Uuid = called.1["provider_instance_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        instance, intended,
        "restart must reuse the committed instance intent"
    );
    // Expected generation is part of the server certificate name; a different one fails closed.
    let instance_row = c
        .query_one("SELECT generation FROM instances WHERE id=$1", &[&instance])
        .await
        .unwrap();
    assert!(tls::channel(
        &config,
        instance,
        instance_row.get::<_, i64>(0) + 1,
        &app.runtime.address(instance).unwrap()
    )
    .await
    .is_err());
    let identity = config
        .root()
        .join("runtime")
        .join(instance.to_string())
        .join("identity");
    let before = fs::read(identity.join("tls.key")).unwrap();
    tls::issue(&config, Some((instance, instance_row.get(0))), &identity).unwrap();
    assert_eq!(fs::read(identity.join("tls.key")).unwrap(), before);
    // Restart the controller; recovery fences the old container before restoring capacity.
    worker.abort();
    let _ = worker.await;
    app.ready.store(false, Ordering::SeqCst);
    worker = tokio::spawn(controller::run(app.clone()));
    for _ in 0..100 {
        if app.ready.load(Ordering::SeqCst)
            && api(
                &router,
                "invokeCapability",
                &[],
                Some(token),
                input.clone(),
                "after-restart",
                None,
            )
            .await
            .0 == 200
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(
        api(
            &router,
            "invokeCapability",
            &[],
            Some(token),
            input,
            "after-restart",
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        c.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        500
    );
    // A second independently implemented language/runtime reuses the same contract.
    let python_raw = fs::read("examples/generated-python/xelab/plugin.json").unwrap();
    let python_manifest: Value = serde_json::from_slice(&python_raw).unwrap();
    assert_eq!(manifest["capabilities"], python_manifest["capabilities"]);
    assert_ne!(manifest["plugin_id"], python_manifest["plugin_id"]);
    let python_image = std::env::var("XELAB_TEST_PYTHON_IMAGE").unwrap();
    let python_digest = artifact::image_ref(&python_image).unwrap();
    let envelope =
        artifact::signature_envelope(&python_manifest, python_digest, &python_raw).unwrap();
    let install = json!({"publisher_id":publisher,"image":python_image,"signature":STANDARD.encode(signing.sign(envelope.as_bytes()).to_bytes())});
    let installed = api(
        &router,
        "installPlugin",
        &[],
        Some(token),
        install,
        "install-python",
        None,
    )
    .await;
    assert_eq!(installed.0, 202, "{}", installed.1);
    let installed = operation(&router, installed.1["id"].as_str().unwrap(), token).await;
    let created=api(&router,"createDeployment",&[],Some(token),json!({"installation_id":installed["installation_id"],"display_name":"independent Python","enabled":false,"limits":limits,"config":{},"volume_ids":[]}),"deploy-python",None).await;
    assert_eq!(created.0, 202, "{}", created.1);
    let python_deployment: Uuid = operation(&router, created.1["id"].as_str().unwrap(), token)
        .await["deployment_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let python_params = [("deployment_id", python_deployment.to_string())];
    let approved = api(
        &router,
        "approvePlugin",
        &python_params,
        Some(token),
        json!({"rules":[],"egress":[]}),
        "approve-python",
        Some(1),
    )
    .await;
    assert_eq!(approved.0, 200, "{}", approved.1);
    worker.abort();
    let _ = worker.await;
    app.ready.store(false, Ordering::SeqCst);
    let enabled = api(
        &router,
        "setDeploymentEnabled",
        &python_params,
        Some(token),
        json!({"enabled":true}),
        "enable-python",
        Some(2),
    )
    .await;
    assert_eq!(enabled.0, 202, "{}", enabled.1);
    let row = c
        .query_one(
            "SELECT id,generation FROM instances WHERE deployment_id=$1",
            &[&python_deployment],
        )
        .await
        .unwrap();
    let python_instance: Uuid = row.get(0);
    let python_generation: i64 = row.get(1);
    let tx = c.transaction().await.unwrap();
    admission::reserve(&tx, python_instance).await.unwrap();
    tx.commit().await.unwrap();
    // RUN-02: OCI has been created, but observation was never committed.
    app.runtime
        .ensure(
            python_instance,
            python_deployment,
            python_generation,
            &python_manifest,
            &python_image,
            python_digest,
            &limits,
        )
        .await
        .unwrap();
    assert!(app.runtime.running(python_instance).await.unwrap());
    let mut described = false;
    for _ in 0..30 {
        if let Ok(channel) = tls::channel(
            &config,
            python_instance,
            python_generation,
            &app.runtime.address(python_instance).unwrap(),
        )
        .await
        {
            let mut client =
                xelab_kernel::rpc::plugin_lifecycle_client::PluginLifecycleClient::new(channel);
            let description = client
                .describe(xelab_kernel::rpc::DescribeRequest {
                    context: Some(controller::context(None)),
                })
                .await;
            assert!(description.is_ok(), "independent Describe: {description:?}");
            assert_eq!(description.unwrap().into_inner().manifest_json, python_raw);
            described = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(described, "independent plugin TLS listener did not start");
    // A CA-trusted workload is still not the kernel: identity authorization
    // must reject it after mTLS, not merely trust every certificate from the CA.
    let imposter = root.join("imposter-identity");
    tls::issue(&config, Some((Uuid::new_v4(), 1)), &imposter).unwrap();
    let imposter_tls = tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(
            fs::read(root.join("ca.crt")).unwrap(),
        ))
        .identity(tonic::transport::Identity::from_pem(
            fs::read(imposter.join("tls.crt")).unwrap(),
            fs::read(imposter.join("tls.key")).unwrap(),
        ))
        .domain_name(format!(
            "i-{python_instance}-g-{python_generation}.xelab.local"
        ));
    let imposter_channel = tonic::transport::Endpoint::from_shared(format!(
        "https://{}:7000",
        app.runtime.address(python_instance).unwrap()
    ))
    .unwrap()
    .tls_config(imposter_tls)
    .unwrap()
    .timeout(Duration::from_secs(3))
    .connect()
    .await
    .unwrap();
    let denied =
        xelab_kernel::rpc::plugin_lifecycle_client::PluginLifecycleClient::new(imposter_channel)
            .describe(xelab_kernel::rpc::DescribeRequest {
                context: Some(controller::context(None)),
            })
            .await
            .unwrap_err();
    assert_eq!(denied.code(), tonic::Code::PermissionDenied);
    assert!(c
        .query_one(
            "SELECT runtime_id FROM instances WHERE id=$1",
            &[&python_instance]
        )
        .await
        .unwrap()
        .get::<_, Option<String>>(0)
        .is_none());
    worker = tokio::spawn(controller::run(app.clone()));
    operation(&router, enabled.1["id"].as_str().unwrap(), token).await;
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM instances WHERE deployment_id=$1",
            &[&python_deployment]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM reservations WHERE instance_id=$1 AND status<>'released'",
            &[&python_instance]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        1
    );
    assert_eq!(app.runtime.inventory().await.unwrap().len(), 2);
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM provider_records WHERE availability='ready'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        2
    );
    let python_input = json!({"contract":manifest["capabilities"][0]["contract"],"input":{"message":"hello OCI"},"provider_deployment_id":python_deployment,"timeout_ms":10000});
    let python_result = api(
        &router,
        "invokeCapability",
        &[],
        Some(token),
        python_input.clone(),
        "invoke-python",
        None,
    )
    .await;
    assert_eq!(python_result.0, 200, "{}", python_result.1);
    assert_eq!(python_result.1["output"], called.1["output"]);
    assert_eq!(
        python_result.1["provider_instance_id"],
        python_instance.to_string()
    );
    // Real plugin -> HostService mTLS path, with a signed requested secret.use
    // rule, approved binding, rotation, and immediate binding revocation.
    let host_worker = tokio::spawn(xelab_kernel::host::serve(app.clone()));
    let secret = api(
        &router,
        "createSecret",
        &[],
        Some(token),
        json!({"name":"oci-provider","value_base64":STANDARD.encode(b"oci-initial-secret")}),
        "oci-secret",
        None,
    )
    .await;
    assert_eq!(secret.0, 201, "{}", secret.1);
    let secret_id = secret.1["id"].as_str().unwrap();
    let bound = api(
        &router,
        "bindSecret",
        &[],
        Some(token),
        json!({"secret_id":secret_id,"deployment_id":python_deployment,"maximum_lease_seconds":60}),
        "oci-bind-secret",
        None,
    )
    .await;
    assert_eq!(bound.0, 201, "{}", bound.1);
    let binding = bound.1["id"].as_str().unwrap();
    let probe = |expected: String, version: &str| {
        vec![
            "tasks".into(),
            "exec".into(),
            "--exec-id".into(),
            format!("secret-{}", Uuid::new_v4()),
            python_instance.to_string(),
            "/usr/bin/python3".into(),
            "/opt/plugin/lease_probe.py".into(),
            binding.to_string(),
            expected,
            version.to_string(),
        ]
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let initial = app
        .runtime
        .ctr(&probe(
            contracts::digest(b"oci-initial-secret")[7..].to_string(),
            "1",
        ))
        .await
        .unwrap();
    assert_eq!(initial, b"lease-verified\n");
    let rotated = api(
        &router,
        "rotateSecret",
        &[("secret_id", secret_id.to_string())],
        Some(token),
        json!({"name":"oci-provider","value_base64":STANDARD.encode(b"oci-rotated-secret")}),
        "oci-rotate-secret",
        Some(1),
    )
    .await;
    assert_eq!(rotated.0, 200, "{}", rotated.1);
    let rotation = app
        .runtime
        .ctr(&probe(
            contracts::digest(b"oci-rotated-secret")[7..].to_string(),
            "2",
        ))
        .await
        .unwrap();
    assert_eq!(rotation, b"lease-verified\n");
    let revoked = api(
        &router,
        "revokeSecretBinding",
        &[("binding_id", binding.to_string())],
        Some(token),
        Value::Null,
        "oci-revoke-binding",
        Some(1),
    )
    .await;
    assert_eq!(revoked.0, 204, "{}", revoked.1);
    assert_eq!(
        app.runtime.ctr(&probe("denied".into(), "2")).await.unwrap(),
        b"lease-denied\n"
    );
    host_worker.abort();
    let _ = host_worker.await;
    // Real per-key lifetime accounting, including replay without another charge.
    let key_body = json!({"name":"one call","inheritance":{"mode":"selected","rules":[{"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":["org.xelab.echo@1.0.0"]}}]},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::hours(1),"max_calls":1}});
    let issued = api(
        &router,
        "createApiKey",
        &[("account_id", owner.to_string())],
        Some(token),
        key_body,
        "issue-one-call",
        None,
    )
    .await;
    assert_eq!(issued.0, 201, "{}", issued.1);
    let limited = issued.1["secret"].as_str().unwrap();
    assert_eq!(
        api(
            &router,
            "invokeCapability",
            &[],
            Some(limited),
            python_input.clone(),
            "limited-call",
            None
        )
        .await
        .0,
        200
    );
    assert_eq!(
        api(
            &router,
            "invokeCapability",
            &[],
            Some(limited),
            python_input.clone(),
            "limited-call",
            None
        )
        .await
        .0,
        200
    );
    let exhausted = api(
        &router,
        "invokeCapability",
        &[],
        Some(limited),
        python_input.clone(),
        "limited-again",
        None,
    )
    .await;
    assert_eq!(exhausted.0, 409, "{}", exhausted.1);
    assert_eq!(exhausted.1["error"]["code"], "QUOTA_EXCEEDED");
    grant_source_budget(&router, &db, owner, token, &python_input).await;
    // RUN-03: exercise the cgroup, not just serialized OCI configuration.
    let cg = std::path::Path::new("/sys/fs/cgroup/xelab/plugins").join(python_instance.to_string());
    assert_eq!(
        fs::read_to_string(cg.join("memory.max")).unwrap().trim(),
        "134217728"
    );
    assert_eq!(
        fs::read_to_string(cg.join("cpu.max")).unwrap().trim(),
        "50000 100000"
    );
    assert_eq!(
        fs::read_to_string(cg.join("pids.max")).unwrap().trim(),
        "64"
    );
    let before = counter(&cg.join("cpu.stat"), "nr_throttled");
    let stress = exec_python(
        python_instance,
        "import time\nend=time.monotonic()+2\nwhile time.monotonic()<end: pass",
    )
    .await;
    assert!(
        stress.status.success(),
        "{}",
        String::from_utf8_lossy(&stress.stderr)
    );
    assert!(counter(&cg.join("cpu.stat"), "nr_throttled") > before);
    let before = counter(&cg.join("memory.events"), "oom_kill");
    let oom = exec_python(python_instance, "a=bytearray(256*1024*1024)").await;
    assert!(!oom.status.success());
    assert!(counter(&cg.join("memory.events"), "oom_kill") > before);
    assert!(
        app.runtime.running(instance).await.unwrap(),
        "another plugin survives the OOM"
    );
    let isolation=exec_python(python_instance,"import socket\ntry:\n open('/xelab/forbidden','w')\n raise AssertionError('rootfs writable')\nexcept OSError: pass\ns=socket.socket();s.settimeout(1)\ntry:\n s.connect(('1.1.1.1',443))\n raise AssertionError('egress allowed')\nexcept OSError: pass").await;
    assert!(
        isolation.status.success(),
        "{}",
        String::from_utf8_lossy(&isolation.stderr)
    );
    // A dropped HTTP task can leave a durable running call without a finalizer.
    // Recovery must fence the old process before returning its slot, then restart.
    // Keep an unsettled disabled backlog larger than the old 1000-row page.
    // Recovery and health work must proceed before this cleanup sweep finishes.
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) SELECT gen_random_uuid(),owner_id,installation_id,'{\"enabled\":false}'::jsonb,'reconciling',created_at-interval '1 day',now() FROM deployments CROSS JOIN generate_series(1,1200) WHERE id=$1", &[&deployment.parse::<Uuid>().unwrap()]).await.unwrap();
    let recovery_started = tokio::time::Instant::now();
    let abandoned = Uuid::new_v4();
    let prior: Uuid = called.1["call_id"].as_str().unwrap().parse().unwrap();
    let tx = c.transaction().await.unwrap();
    xelab_kernel::db::call_lock(&tx).await.unwrap();
    tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at) SELECT $1,owner_id,credential_id,$2,contract,input_digest,'abandoned-fixture','running',now()-interval '10 seconds',now() FROM calls WHERE id=$3", &[&abandoned,&python_instance,&prior]).await.unwrap();
    tx.execute("INSERT INTO reservations(id,account_id,kind,expires_at,status,created_at) VALUES($1,$2,'call',now()-interval '10 seconds','active',now())", &[&abandoned,&owner]).await.unwrap();
    tx.execute(
        "UPDATE quotas SET concurrent_calls_used=concurrent_calls_used+1 WHERE account_id=$1",
        &[&owner],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut recovered = false;
    for _ in 0..120 {
        let state=c.query_one("SELECT c.state,r.status,q.concurrent_calls_used FROM calls c JOIN reservations r ON r.id=c.id JOIN quotas q ON q.account_id=c.owner_id WHERE c.id=$1", &[&abandoned]).await.unwrap();
        let ready:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM provider_records WHERE instance_id=$1 AND availability='ready')", &[&python_instance]).await.unwrap().get(0);
        if state.get::<_, String>(0) == "unknown"
            && state.get::<_, String>(1) == "released"
            && state.get::<_, i64>(2) == 0
            && ready
        {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        recovered,
        "abandoned call slot is reclaimed and stable provider recovers"
    );
    assert!(recovery_started.elapsed() < Duration::from_secs(60));
    let backlog: i64 = c.query_one("SELECT count(*) FROM deployments WHERE spec->'enabled'='false'::jsonb AND phase='reconciling'", &[]).await.unwrap().get(0);
    assert!(
        backlog > 0,
        "call recovery must not wait for the disabled sweep"
    );
    // Clean just the fixture rows (they have no instances/operations) so the
    // subsequent explicit disable operations retain their own latency budget.
    c.execute(
        "DELETE FROM deployments WHERE spec=$1",
        &[&json!({"enabled":false})],
    )
    .await
    .unwrap();
    // RUN-07: execute the shipped CLI over HTTPS against the same API, without WebUI.
    let leaf = KeyPair::generate().unwrap();
    let server_cert = CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .signed_by(&leaf, &cert, &key)
        .unwrap();
    let server_tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        server_cert.pem().into_bytes(),
        leaf.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let serve = tokio::spawn(
        axum_server::from_tcp_rustls(listener, server_tls).serve(
            router
                .clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        ),
    );
    let token_file = root.join("cli-token");
    fs::write(&token_file, token).unwrap();
    fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600)).unwrap();
    let input_file = root.join("invoke.json");
    fs::write(&input_file, serde_json::to_vec(&python_input).unwrap()).unwrap();
    let cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .args([
            "api",
            "invokeCapability",
            "--endpoint",
            &format!("https://{address}"),
            "--token-file",
            token_file.to_str().unwrap(),
            "--ca-file",
            root.join("ca.crt").to_str().unwrap(),
            "--body-file",
            input_file.to_str().unwrap(),
            "--idempotency-key",
            "cli-independent-python",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli_value: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(cli_value["output"], called.1["output"]);
    serve.abort();
    // Component-only: verify the actual RUN-03 exec gate / cgroup cleanup on
    // the disposable CI plugin. This does not satisfy production host admission.
    let component = tokio::process::Command::new("python3")
        .args([
            "tools/acceptance/oci_stress_component.py",
            &python_instance.to_string(),
        ])
        .output()
        .await
        .unwrap();
    assert!(
        component.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&component.stdout),
        String::from_utf8_lossy(&component.stderr)
    );
    println!("{}", String::from_utf8_lossy(&component.stdout));
    let python_value = api(
        &router,
        "getDeployment",
        &python_params,
        Some(token),
        Value::Null,
        "read-python",
        None,
    )
    .await
    .1;
    let disabled = api(
        &router,
        "setDeploymentEnabled",
        &python_params,
        Some(token),
        json!({"enabled":false}),
        "disable-python",
        python_value["revision"].as_i64(),
    )
    .await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    operation(&router, disabled.1["id"].as_str().unwrap(), token).await;
    let deployment_value = api(
        &router,
        "getDeployment",
        &params,
        Some(token),
        Value::Null,
        "read",
        None,
    )
    .await
    .1;
    let disabled = api(
        &router,
        "setDeploymentEnabled",
        &params,
        Some(token),
        json!({"enabled":false}),
        "disable-1",
        deployment_value["revision"].as_i64(),
    )
    .await;
    assert_eq!(disabled.0, 202, "{}", disabled.1);
    operation(&router, disabled.1["id"].as_str().unwrap(), token).await;
    assert!(!app.runtime.running(instance).await.unwrap());
    assert_eq!(
        c.query_one("SELECT reserved_cpu_millis FROM node_budget", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    let revoke = api(
        &router,
        "revokeSession",
        &[(
            "session_id",
            session["session"]["id"].as_str().unwrap().into(),
        )],
        Some(token),
        Value::Null,
        "revoke-1",
        Some(1),
    )
    .await;
    assert_eq!(revoke.0, 204, "{}", revoke.1);
    assert_eq!(
        api(
            &router,
            "getNode",
            &[],
            Some(token),
            Value::Null,
            "read",
            None
        )
        .await
        .0,
        401
    );
    app.shutdown.store(true, Ordering::SeqCst);
    worker.abort();
}

fn counter(path: &std::path::Path, name: &str) -> u64 {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            (key == name).then(|| value.parse().unwrap())
        })
        .unwrap()
}
async fn exec_python(instance: Uuid, script: &str) -> std::process::Output {
    tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("/usr/bin/ctr")
            .args([
                "--namespace",
                "xelab-ci",
                "tasks",
                "exec",
                "--exec-id",
                &Uuid::new_v4().to_string(),
                &instance.to_string(),
                "/usr/bin/python3",
                "-c",
                script,
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn grant_source_budget(router: &Router, db: &Db, owner: Uuid, token: &str, input: &Value) {
    let c = db.connect().await.unwrap();
    let parent = Uuid::new_v4();
    let password = "grant budget acceptance password";
    let hash = auth::password_hash(password).unwrap();
    // Trusted fixture root with one replaceable grant, not another Owner.
    c.execute("INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at) VALUES($1,'user','active','budget parent',$2,now(),now())", &[&parent,&hash]).await.unwrap();
    c.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&parent])
        .await
        .unwrap();
    c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2", &[&parent,&owner]).await.unwrap();
    let scope = json!({"kind":"capability","match":"ids","ids":["org.xelab.echo@1.0.0"]});
    let grant_body = json!({"source":"role","role_id":"Guest","scope":scope});
    let old = api(
        router,
        "createGrant",
        &[("account_id", parent.to_string())],
        Some(token),
        grant_body.clone(),
        "parent-old",
        None,
    )
    .await;
    assert_eq!(old.0, 201, "{}", old.1);
    let login = api(
        router,
        "login",
        &[],
        None,
        json!({"account_id":parent,"password":password}),
        "parent-login",
        None,
    )
    .await;
    assert_eq!(login.0, 200, "{}", login.1);
    let parent_token = login.1["access_token"].as_str().unwrap();
    let child = api(
        router,
        "createChildAccount",
        &[("account_id", parent.to_string())],
        Some(parent_token),
        json!({"display_name":"budget child","inheritance":{"mode":"all_snapshot"}}),
        "budget-child",
        None,
    )
    .await;
    assert_eq!(child.0, 201, "{}", child.1);
    let child_id = contracts::id(&child.1["account"], "id").unwrap();
    let activated = api(
        router,
        "activateAccount",
        &[],
        None,
        json!({"activation_token":child.1["activation_token"],"password":password}),
        "child-activate",
        None,
    )
    .await;
    assert_eq!(activated.0, 200, "{}", activated.1);
    let login = api(
        router,
        "login",
        &[],
        None,
        json!({"account_id":child_id,"password":password}),
        "child-login",
        None,
    )
    .await;
    assert_eq!(login.0, 200, "{}", login.1);
    let session = login.1["access_token"].as_str().unwrap();
    let expiry = chrono::Utc::now() + chrono::Duration::hours(1);
    let key = api(router,"createApiKey",&[("account_id",owner.to_string())],Some(token),json!({"name":"grant issuer one call","inheritance":{"mode":"all_snapshot"},"conditions":{"expires_at":expiry,"max_calls":1}}),"grant-key",None).await;
    assert_eq!(key.0, 201, "{}", key.1);
    let key_id = contracts::id(&key.1["credential"], "id").unwrap();
    let mut delegated = grant_body.clone();
    delegated["expires_at"] = json!(expiry);
    let grant = api(
        router,
        "createGrant",
        &[("account_id", child_id.to_string())],
        Some(key.1["secret"].as_str().unwrap()),
        delegated,
        "limited-grant",
        None,
    )
    .await;
    assert_eq!(grant.0, 201, "{}", grant.1);
    // Withdraw the frozen inherited source, then restore the parent's ceiling
    // with an independent replacement. Only the child's direct grant survives.
    let revoked = api(
        router,
        "revokeGrant",
        &[("grant_id", old.1["id"].as_str().unwrap().into())],
        Some(token),
        Value::Null,
        "old-revoke",
        Some(1),
    )
    .await;
    assert_eq!(revoked.0, 204, "{}", revoked.1);
    let replacement = api(
        router,
        "createGrant",
        &[("account_id", parent.to_string())],
        Some(token),
        grant_body.clone(),
        "parent-new",
        None,
    )
    .await;
    assert_eq!(replacement.0, 201, "{}", replacement.1);
    let first = api(
        router,
        "invokeCapability",
        &[],
        Some(session),
        input.clone(),
        "grant-call",
        None,
    )
    .await;
    assert_eq!(first.0, 200, "{}", first.1);
    let replay = api(
        router,
        "invokeCapability",
        &[],
        Some(session),
        input.clone(),
        "grant-call",
        None,
    )
    .await;
    assert_eq!(replay.0, 200, "{}", replay.1);
    assert_eq!(first.1["call_id"], replay.1["call_id"]);
    let second = api(
        router,
        "invokeCapability",
        &[],
        Some(session),
        input.clone(),
        "grant-call-again",
        None,
    )
    .await;
    assert_eq!(second.0, 409, "{}", second.1);
    assert_eq!(second.1["error"]["code"], "QUOTA_EXCEEDED");
    assert_eq!(
        xelab_kernel::identity::key_calls(&c, key_id).await.unwrap(),
        1
    );
    // An exhausted alternative cannot block or charge an independent grant.
    let independent = api(
        router,
        "createGrant",
        &[("account_id", child_id.to_string())],
        Some(token),
        grant_body,
        "independent-grant",
        None,
    )
    .await;
    assert_eq!(independent.0, 201, "{}", independent.1);
    let allowed = api(
        router,
        "invokeCapability",
        &[],
        Some(session),
        input.clone(),
        "independent-call",
        None,
    )
    .await;
    assert_eq!(allowed.0, 200, "{}", allowed.1);
    assert_eq!(
        xelab_kernel::identity::key_calls(&c, key_id).await.unwrap(),
        1
    );
    let revoke = api(
        router,
        "revokeGrant",
        &[("grant_id", independent.1["id"].as_str().unwrap().into())],
        Some(token),
        Value::Null,
        "independent-revoke",
        Some(1),
    )
    .await;
    assert_eq!(revoke.0, 204, "{}", revoke.1);
    let revoke = api(
        router,
        "revokeApiKey",
        &[("credential_id", key_id.to_string())],
        Some(token),
        Value::Null,
        "source-key-revoke",
        Some(1),
    )
    .await;
    assert_eq!(revoke.0, 204, "{}", revoke.1);
    let denied = api(
        router,
        "invokeCapability",
        &[],
        Some(session),
        input.clone(),
        "grant-call",
        None,
    )
    .await;
    assert_eq!(denied.0, 403, "{}", denied.1);
    assert_eq!(
        xelab_kernel::identity::key_calls(&c, key_id).await.unwrap(),
        1,
        "revocation never refunds used budget"
    );
}
