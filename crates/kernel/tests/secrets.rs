//! Real PostgreSQL, HTTP and mTLS transport tests. Workload metadata here is a
//! fixture; the dedicated OCI test separately verifies real container identity.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, SanType};
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
    api::{router, App},
    auth::{self, Auth},
    config::Config,
    contracts,
    db::Db,
    rpc,
    runtime::Runtime,
    tls,
};
struct Fixture {
    app: Arc<App>,
    router: Router,
    token: String,
    directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Option<Self> {
        xelab_kernel::init_crypto();
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: Secret PostgreSQL tests require XELAB_TEST_DATABASE_URL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("secret_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let mut value = serde_json::to_value(t).unwrap();
        value["storage"]["root_path"] = json!(root.join("state"));
        value["security"]["master_key_file"] = json!(root.join("master.key"));
        value["security"]["workload_ca_cert_file"] = json!(root.join("ca.crt"));
        value["security"]["workload_ca_key_file"] = json!(root.join("ca.key"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        value["server"]["host_rpc_listen"] = json!(listener.local_addr().unwrap().to_string());
        drop(listener);
        fs::write(root.join("master.key"), [42; 32]).unwrap();
        fs::set_permissions(root.join("master.key"), fs::Permissions::from_mode(0o600)).unwrap();
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["test-ca".into()]).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = params.self_signed(&key).unwrap();
        fs::write(root.join("ca.crt"), ca.pem()).unwrap();
        fs::write(root.join("ca.key"), key.serialize_pem()).unwrap();
        fs::set_permissions(root.join("ca.key"), fs::Permissions::from_mode(0o600)).unwrap();
        let config = Config::from_value(value).unwrap();
        tls::issue(&config, None, &config.root().join("kernel-identity")).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(
            &tx,
            "Owner",
            "correct horse secret test password",
            &config.value["quota_defaults"],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let auth = Auth::new(vec![72; 32]).unwrap();
        let token = auth
            .login(
                &c,
                owner,
                "correct horse secret test password".into(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_string();
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db: db.clone(),
            auth,
            leader: Arc::new(db.leader().await.unwrap()),
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        Some(Self {
            router: router(app.clone()),
            app,
            token,
            directory,
        })
    }
    async fn call(
        &self,
        op: &str,
        params: &[(&str, Uuid)],
        body: Value,
        revision: Option<i64>,
        key: &str,
        token: Option<&str>,
    ) -> (u16, Value) {
        let mut selected = None;
        for (path, methods) in contracts::openapi()["paths"].as_object().unwrap() {
            for (method, spec) in methods.as_object().unwrap() {
                if spec["operationId"] == op {
                    selected = Some((path.clone(), method.clone()));
                }
            }
        }
        let (mut path, method) = selected.unwrap();
        for (k, v) in params {
            path = path.replace(&format!("{{{k}}}"), &v.to_string());
        }
        let mut builder = Request::builder()
            .method(method.to_uppercase().as_str())
            .uri(path)
            .header(
                "authorization",
                format!("Bearer {}", token.unwrap_or(&self.token)),
            )
            .header("idempotency-key", key);
        if let Some(rev) = revision {
            builder = builder.header("if-match", format!("\"rev-{rev}\""));
        }
        let mut request = builder
            .header("content-type", "application/json")
            .body(if body.is_null() {
                Body::empty()
            } else {
                Body::from(serde_json::to_vec(&body).unwrap())
            })
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let bytes = to_bytes(response.into_body(), 1048576).await.unwrap();
        (
            status,
            if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            },
        )
    }
    async fn secret(&self) -> Uuid {
        let (status,v)=self.call("createSecret",&[],json!({"name":"service","value_base64":STANDARD.encode(b"sensitive-provider-token")}),None,"create",None).await;
        assert_eq!(status, 201, "{v}");
        contracts::id(&v, "id").unwrap()
    }
    async fn workload(&self) -> (Uuid, Uuid) {
        let c = self.app.db.connect().await.unwrap();
        let owner: Uuid = c
            .query_one("SELECT id FROM accounts WHERE independent_owner", &[])
            .await
            .unwrap()
            .get(0);
        let publisher = Uuid::new_v4();
        let installation = Uuid::new_v4();
        let deployment = Uuid::new_v4();
        let instance = Uuid::new_v4();
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture','fixture','fixture','trusted',now(),now())",&[&publisher]).await.unwrap();
        let mut manifest: Value =
            serde_json::from_str(include_str!("fixtures/secret-manifest.json")).unwrap();
        manifest["publisher_id"] = json!(publisher);
        xelab_kernel::artifact::validate_profile(&manifest).unwrap();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.echo-python','1.0.0',$2,'fixture','fixture',$3,'installed',now(),now())",&[&installation,&publisher,&manifest]).await.unwrap();
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())",&[&deployment,&owner,&installation,&json!({"enabled":true})]).await.unwrap();
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,runtime_id,created_at,updated_at) VALUES($1,$2,7,0,'ready','alive','{}',$3,now(),now())",&[&instance,&deployment,&instance.to_string()]).await.unwrap();
        (deployment, instance)
    }
    async fn binding(&self, secret: Uuid, deployment: Uuid) -> Uuid {
        let (status, v) = self
            .call(
                "bindSecret",
                &[],
                json!({"secret_id":secret,"deployment_id":deployment,"maximum_lease_seconds":60}),
                None,
                "bind",
                None,
            )
            .await;
        assert_eq!(status, 201, "{v}");
        contracts::id(&v, "id").unwrap()
    }
    fn client_identity(&self, uri: &str) -> tonic::transport::Identity {
        let root = self.directory.path();
        let ca_key = KeyPair::from_pem(&fs::read_to_string(root.join("ca.key")).unwrap()).unwrap();
        let ca =
            CertificateParams::from_ca_cert_pem(&fs::read_to_string(root.join("ca.crt")).unwrap())
                .unwrap()
                .self_signed(&ca_key)
                .unwrap();
        let leaf = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(vec!["workload.test".into()]).unwrap();
        params
            .subject_alt_names
            .push(SanType::URI(uri.try_into().unwrap()));
        let cert = params.signed_by(&leaf, &ca, &ca_key).unwrap();
        tonic::transport::Identity::from_pem(cert.pem(), leaf.serialize_pem())
    }
    async fn client(
        &self,
        identity: tonic::transport::Identity,
    ) -> rpc::host_service_client::HostServiceClient<tonic::transport::Channel> {
        let tls = tonic::transport::ClientTlsConfig::new()
            .domain_name("kernel.xelab.local")
            .ca_certificate(tonic::transport::Certificate::from_pem(
                fs::read(self.directory.path().join("ca.crt")).unwrap(),
            ))
            .identity(identity);
        let endpoint = tonic::transport::Endpoint::from_shared(format!(
            "https://{}",
            self.app.config.text("server", "host_rpc_listen")
        ))
        .unwrap()
        .tls_config(tls)
        .unwrap();
        let mut result = None;
        for _ in 0..50 {
            match endpoint.clone().connect().await {
                Ok(channel) => {
                    result = Some(channel);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        rpc::host_service_client::HostServiceClient::new(
            result.expect("HostService must accept mTLS"),
        )
    }
}
fn request(binding: Uuid) -> rpc::LeaseSecretRequest {
    rpc::LeaseSecretRequest {
        context: Some(xelab_kernel::controller::context(None)),
        binding_id: binding.to_string(),
        requested_seconds: 30,
    }
}

#[tokio::test]
async fn encrypted_secret_api_preserves_metadata_only_and_live_revocation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let secret = f.secret().await;
    let params = [("secret_id", secret)];
    let c = f.app.db.connect().await.unwrap();
    let (status, replay) = f
        .call(
            "createSecret",
            &[],
            json!({"name":"service","value_base64":STANDARD.encode(b"sensitive-provider-token")}),
            None,
            "create",
            None,
        )
        .await;
    assert_eq!(status, 201);
    assert_eq!(replay["id"], secret.to_string());
    let (status, _) = f
        .call(
            "createSecret",
            &[],
            json!({"name":"service","value_base64":"!invalid!"}),
            None,
            "invalid",
            None,
        )
        .await;
    assert_eq!(status, 400);
    let (status, _) = f
        .call(
            "createSecret",
            &[],
            json!({"name":"service","value_base64":STANDARD.encode(vec![3;65537])}),
            None,
            "too-large",
            None,
        )
        .await;
    assert_eq!(status, 400);
    let row = c
        .query_one(
            "SELECT encrypted_value,encryption_key_id FROM secrets WHERE id=$1",
            &[&secret],
        )
        .await
        .unwrap();
    assert!(!row
        .get::<_, Vec<u8>>(0)
        .windows(b"sensitive-provider-token".len())
        .any(|p| p == b"sensitive-provider-token"));
    assert!(row.get::<_, String>(1).starts_with("sha256:"));
    let (status, list) = f
        .call("listSecrets", &[], Value::Null, None, "list", None)
        .await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let metadata = f
        .call("getSecretMetadata", &params, Value::Null, None, "get", None)
        .await;
    assert_eq!(metadata.0, 200);
    assert!(metadata.1.get("value_base64").is_none());
    assert!(metadata.1.get("encrypted_value").is_none());
    let (deployment, instance) = f.workload().await;
    let binding = f.binding(secret, deployment).await;
    let initial = xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
        .await
        .unwrap();
    assert_eq!(initial.value, b"sensitive-provider-token");
    assert_eq!(initial.secret_version, 1);
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 6, &request(binding))
            .await
            .is_err()
    );
    assert!(
        xelab_kernel::secrets::lease(&f.app, Uuid::new_v4(), 7, &request(binding))
            .await
            .is_err()
    );
    let mut too_long = request(binding);
    too_long.requested_seconds = 61;
    assert!(xelab_kernel::secrets::lease(&f.app, instance, 7, &too_long)
        .await
        .is_err());
    let rotated = json!({"name":"service","value_base64":STANDARD.encode(b"rotated-token")});
    assert_eq!(
        f.call(
            "rotateSecret",
            &params,
            rotated.clone(),
            Some(2),
            "stale",
            None
        )
        .await
        .0,
        412
    );
    let rotation = f
        .call("rotateSecret", &params, rotated, Some(1), "rotate", None)
        .await;
    assert_eq!(rotation.0, 200, "{}", rotation.1);
    assert_eq!(rotation.1["version"], 2);
    assert_eq!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .unwrap()
            .value,
        b"rotated-token"
    );
    let audits=c.query("SELECT to_jsonb(a)::text FROM audit_events a UNION ALL SELECT to_jsonb(i)::text FROM idempotency_records i",&[]).await.unwrap();
    for row in audits {
        let text: String = row.get(0);
        for raw in [
            "sensitive-provider-token",
            "rotated-token",
            &STANDARD.encode(b"sensitive-provider-token"),
            &STANDARD.encode(b"rotated-token"),
        ] {
            assert!(!text.contains(raw));
        }
    }
    assert_eq!(
        f.call(
            "revokeSecretBinding",
            &[("binding_id", binding)],
            Value::Null,
            Some(1),
            "revoke-binding",
            None
        )
        .await
        .0,
        204
    );
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_err()
    );
    assert_eq!(
        f.call(
            "revokeSecret",
            &params,
            Value::Null,
            Some(2),
            "revoke",
            None
        )
        .await
        .0,
        204
    );
    assert_eq!(
        f.call(
            "rotateSecret",
            &params,
            json!({"name":"service","value_base64":"YQ=="}),
            Some(3),
            "revive",
            None
        )
        .await
        .0,
        409
    );
}

#[tokio::test]
async fn actual_mtls_checks_peer_generation_key_integrity_and_issuer_revocation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let secret = f.secret().await;
    let (deployment, instance) = f.workload().await;
    let binding = f.binding(secret, deployment).await;
    let server = tokio::spawn(xelab_kernel::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    assert_eq!(
        client
            .lease_secret(request(binding))
            .await
            .unwrap()
            .into_inner()
            .value,
        b"sensitive-provider-token"
    );
    let mut imposter = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/6"
        )))
        .await;
    assert_eq!(
        imposter
            .lease_secret(request(binding))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    let mut kernel = f
        .client(f.client_identity("spiffe://xelab.local/kernel"))
        .await;
    assert_eq!(
        kernel
            .lease_secret(request(binding))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    let mut spoof = request(binding);
    spoof.context.as_mut().unwrap().actor_instance_id = Uuid::new_v4().to_string();
    assert_eq!(
        client.lease_secret(spoof).await.unwrap_err().code(),
        tonic::Code::PermissionDenied
    );
    fs::write(f.directory.path().join("master.key"), [43; 32]).unwrap();
    assert_eq!(
        client
            .lease_secret(request(binding))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unavailable
    );
    fs::write(f.directory.path().join("master.key"), [42; 32]).unwrap();
    let c = f.app.db.connect().await.unwrap();
    c.execute("UPDATE secrets SET encrypted_value=set_byte(encrypted_value,15,get_byte(encrypted_value,15)#1) WHERE id=$1",&[&secret]).await.unwrap();
    assert_eq!(
        client
            .lease_secret(request(binding))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unavailable
    );
    c.execute("UPDATE credentials SET status='revoked' WHERE id=(SELECT credential_id FROM secret_binding_authority WHERE binding_id=$1)",&[&binding]).await.unwrap();
    assert_eq!(
        client
            .lease_secret(request(binding))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Unauthenticated
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn scoped_credentials_cannot_read_bind_or_enumerate_other_secrets() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let secret = f.secret().await;
    let c = f.app.db.connect().await.unwrap();
    let owner: Uuid = c
        .query_one("SELECT id FROM accounts WHERE independent_owner", &[])
        .await
        .unwrap()
        .get(0);
    let (deployment, _) = f.workload().await;
    let binding = f.binding(secret, deployment).await;
    let (status,key)=f.call("createApiKey",&[("account_id",owner)],json!({"name":"metadata only","inheritance":{"mode":"selected","rules":[{"action":"secret.metadata.read","resource":{"kind":"secret","match":"ids","ids":[Uuid::new_v4()]}}]},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::days(1)}}),None,"metadata-key",None).await;
    assert_eq!(status, 201, "{key}");
    let token = key["secret"].as_str().unwrap();
    assert_eq!(
        f.call(
            "getSecretMetadata",
            &[("secret_id", secret)],
            Value::Null,
            None,
            "hidden",
            Some(token)
        )
        .await
        .0,
        404
    );
    let listed = f
        .call(
            "listSecrets",
            &[],
            Value::Null,
            None,
            "hidden-list",
            Some(token),
        )
        .await;
    assert_eq!(listed.0, 200);
    assert_eq!(listed.1["items"], json!([]));
    assert_eq!(
        f.call(
            "getSecretBinding",
            &[("binding_id", binding)],
            Value::Null,
            None,
            "hidden-binding",
            Some(token)
        )
        .await
        .0,
        404
    );
    assert_eq!(
        f.call(
            "bindSecret",
            &[],
            json!({"secret_id":secret,"deployment_id":deployment,"maximum_lease_seconds":30}),
            None,
            "cannot-bind",
            Some(token)
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            "rotateSecret",
            &[("secret_id", secret)],
            json!({"name":"service","value_base64":"YQ=="}),
            Some(1),
            "cannot-rotate",
            Some(token)
        )
        .await
        .0,
        403
    );
    let list = f
        .call("listSecrets", &[], Value::Null, None, "list", None)
        .await;
    assert_eq!(list.1["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn expiry_and_current_deployment_state_fence_new_leases() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let secret = f.secret().await;
    let (deployment, instance) = f.workload().await;
    let binding = f.binding(secret, deployment).await;
    let c = f.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE secrets SET expires_at=now()-interval '1 second' WHERE id=$1",
        &[&secret],
    )
    .await
    .unwrap();
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_err()
    );
    let metadata = f
        .call(
            "getSecretMetadata",
            &[("secret_id", secret)],
            Value::Null,
            None,
            "expired",
            None,
        )
        .await;
    assert_eq!(metadata.0, 200);
    assert_eq!(metadata.1["status"], "expired");
    c.execute("UPDATE secrets SET expires_at=NULL WHERE id=$1", &[&secret])
        .await
        .unwrap();
    c.execute(
        "UPDATE deployments SET spec='{}' WHERE id=$1",
        &[&deployment],
    )
    .await
    .unwrap();
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_err()
    );
    c.execute(
        "UPDATE deployments SET spec=$2 WHERE id=$1",
        &[&deployment, &json!({"enabled":true})],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE instances SET phase='draining' WHERE id=$1",
        &[&instance],
    )
    .await
    .unwrap();
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_err()
    );
    c.execute(
        "UPDATE instances SET phase='ready' WHERE id=$1",
        &[&instance],
    )
    .await
    .unwrap();
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_ok()
    );
    f.app.ready.store(false, Ordering::SeqCst);
    assert!(
        xelab_kernel::secrets::lease(&f.app, instance, 7, &request(binding))
            .await
            .is_err()
    );
}
