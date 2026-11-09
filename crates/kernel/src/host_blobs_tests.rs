//! Real PostgreSQL, private files, HTTP and mTLS transport; workload metadata
//! is a fixture. The OCI acceptance separately proves container identity.
use crate::{
    api::{router, App},
    auth::{self, Auth},
    config::Config,
    contracts,
    db::Db,
    rpc,
    runtime::Runtime,
    tls,
};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
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
struct Fixture {
    app: Arc<App>,
    router: Router,
    token: String,
    directory: tempfile::TempDir,
}
impl Fixture {
    async fn new() -> Option<Self> {
        crate::init_crypto();
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: Host Blob PostgreSQL tests require XELAB_TEST_DATABASE_URL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("host_blob_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
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
        fs::create_dir_all(config.root()).unwrap();
        fs::set_permissions(config.root(), fs::Permissions::from_mode(0o700)).unwrap();
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
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.app.config).unwrap(),
            )
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
        c.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'fixture',$2,'fixture','trusted',now(),now())",&[&publisher,&publisher.to_string()]).await.unwrap();
        let mut manifest: Value =
            serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
        let offset: i64 = c
            .query_one(
                "SELECT count(*) FROM installations WHERE plugin_id='org.xelab.echo-python'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        let version = format!("1.0.{offset}");
        manifest["version"] = json!(version);
        manifest["publisher_id"] = json!(publisher);
        manifest["requested_rules"] = json!([{"required":false,"rule":{"action":"blob.read","resource":{"kind":"blob","match":"all"}}},{"required":false,"rule":{"action":"blob.create","resource":{"kind":"blob","match":"all"}}}]);
        manifest["capabilities"][0]["input_schema"] = json!({"type":"object","properties":{"source":{"type":"string","format":"uuid","x-xelab-resource-kind":"blob"}},"required":["source"],"additionalProperties":false});
        crate::artifact::validate_profile(&manifest).unwrap();
        c.execute("INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,manifest,state,created_at,updated_at) VALUES($1,'org.xelab.echo-python',$4,$2,'fixture','fixture',$3,'installed',now(),now())",&[&installation,&publisher,&manifest,&version]).await.unwrap();
        c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'ready',now(),now())",&[&deployment,&owner,&installation,&json!({"enabled":false})]).await.unwrap();
        c.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,runtime_id,created_at,updated_at) VALUES($1,$2,7,0,'ready','alive','{}',$3,now(),now())",&[&instance,&deployment,&instance.to_string()]).await.unwrap();
        let approval = json!({"rules":[{"action":"blob.read","resource":{"kind":"blob","match":"all"}},{"action":"blob.create","resource":{"kind":"blob","match":"all"}}],"egress":[]});
        let response = self
            .call(
                "approvePlugin",
                &[("deployment_id", deployment)],
                approval,
                Some(1),
                "approve",
                None,
            )
            .await;
        assert_eq!(response.0, 200, "{}", response.1);
        c.execute(
            "UPDATE deployments SET spec=jsonb_set(spec,'{enabled}','true') WHERE id=$1",
            &[&deployment],
        )
        .await
        .unwrap();
        (deployment, instance)
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
impl Fixture {
    async fn source(&self, data: Vec<u8>) -> Uuid {
        let mut request = Request::builder()
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.app.config).unwrap(),
            )
            .method("POST")
            .uri("/api/platform/v1/blobs?content_type=application%2Foctet-stream")
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/octet-stream")
            .header("content-length", data.len())
            .header("x-content-sha256", contracts::digest(&data))
            .header("idempotency-key", Uuid::new_v4().to_string())
            .body(Body::from(data))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1048576).await.unwrap())
                .unwrap();
        assert_eq!(status, 201, "{value}");
        contracts::id(&value, "id").unwrap()
    }
    async fn delegation(
        &self,
        deployment: Uuid,
        instance: Uuid,
        source: Uuid,
    ) -> (Uuid, rpc::RpcContext) {
        self.skill_delegation(deployment, instance, source, None, true)
            .await
    }
    async fn skill_delegation(
        &self,
        deployment: Uuid,
        instance: Uuid,
        source: Uuid,
        skill: Option<Uuid>,
        input_blobs: bool,
    ) -> (Uuid, rpc::RpcContext) {
        let mut c = self.app.db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        crate::db::auth_lock(&tx, false).await.unwrap();
        let ctx = self
            .app
            .auth
            .authenticate(
                &tx,
                &self.token,
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap();
        let manifest:Value=tx.query_one("SELECT t.manifest FROM deployments d JOIN installations t ON t.id=d.installation_id WHERE d.id=$1",&[&deployment]).await.unwrap().get(0);
        let call = Uuid::new_v4();
        let deadline = chrono::Utc::now() + chrono::Duration::seconds(120);
        let input = json!({"source":source});
        let contract = &manifest["capabilities"][0]["contract"];
        tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'running',$8,now())",&[&call,&ctx.subject,&ctx.credential,&instance,&contract,&contracts::digest(&serde_json::to_vec(&input).unwrap()),&Uuid::new_v4().to_string(),&deadline]).await.unwrap();
        if let Some(skill) = skill {
            tx.execute("UPDATE calls SET skill_id=$2,skill_definition_digest=(SELECT definition_digest FROM skills WHERE id=$2) WHERE id=$1", &[&call,&skill]).await.unwrap();
        }
        let mut charged = crate::identity::call_sources(
            &tx,
            &ctx,
            &auth::Resource::new(
                "capability",
                format!(
                    "{}@{}",
                    contract["name"].as_str().unwrap(),
                    contract["version"].as_str().unwrap()
                ),
                None,
            ),
        )
        .await
        .unwrap();
        if let Some(skill) = skill {
            charged.extend(
                crate::identity::call_sources_for(
                    &tx,
                    &ctx,
                    "skill.invoke",
                    &auth::Resource::new("skill", skill, Some(ctx.subject)),
                )
                .await
                .unwrap(),
            );
        }
        for credential in charged {
            tx.execute(
                "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
                &[&call, &credential],
            )
            .await
            .unwrap();
        }
        let token = self
            .app
            .auth
            .issue_rpc(&tx, &ctx, call, instance, 7, deadline)
            .await
            .unwrap();
        super::grant_skill_call(
            &tx,
            &self.app,
            &ctx,
            call,
            deployment,
            &manifest,
            &manifest["capabilities"][0],
            &input,
            input_blobs,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (
            call,
            rpc::RpcContext {
                request_id: Uuid::new_v4().to_string(),
                delegation_token: token,
                deadline_unix_ms: deadline.timestamp_millis(),
                idempotency_key: Uuid::new_v4().to_string(),
                subject_id: ctx.subject.to_string(),
                actor_instance_id: instance.to_string(),
            },
        )
    }
}
fn frames(context: rpc::RpcContext, data: &[u8]) -> Vec<rpc::WriteBlobRequest> {
    use rpc::write_blob_request::Body;
    let mut frames = vec![rpc::WriteBlobRequest {
        body: Some(Body::Header(rpc::WriteBlobHeader {
            context: Some(context),
            content_type: "application/octet-stream".into(),
            size_bytes: data.len() as u64,
            digest: contracts::digest(data),
        })),
    }];
    let mut offset = 0;
    for data in data.chunks(65536) {
        frames.push(rpc::WriteBlobRequest {
            body: Some(Body::Data(rpc::BlobChunk {
                offset,
                data: data.to_vec(),
            })),
        });
        offset += data.len() as u64;
    }
    frames
}
#[tokio::test]
async fn real_mtls_blob_framing_digest_replay_scope_and_terminal_call_fences() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let source_bytes = vec![37; 200000];
    let source = f.source(source_bytes.clone()).await;
    let (call, context) = f.delegation(deployment, instance, source).await;
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let identity = format!("spiffe://xelab.local/instance/{instance}/generation/7");
    let mut client = f.client(f.client_identity(&identity)).await;
    let payload = vec![62; 131077];
    let valid = frames(context.clone(), &payload);
    let reference = client
        .write_blob(tokio_stream::iter(valid.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(reference.digest, contracts::digest(&payload));
    assert_eq!(reference.size_bytes, payload.len() as u64);
    assert_eq!(
        client
            .write_blob(tokio_stream::iter(valid.clone()))
            .await
            .unwrap()
            .into_inner(),
        reference
    );
    let changed = frames(context.clone(), b"changed fingerprint");
    assert_eq!(
        client
            .write_blob(tokio_stream::iter(changed))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::AlreadyExists
    );
    let mut corrupt = valid.clone();
    if let Some(rpc::write_blob_request::Body::Data(c)) = &mut corrupt[1].body {
        c.data[0] ^= 1;
    }
    assert_eq!(
        client
            .write_blob(tokio_stream::iter(corrupt))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let zero = frames(
        rpc::RpcContext {
            idempotency_key: "empty".into(),
            ..context.clone()
        },
        b"",
    );
    assert_eq!(
        client
            .write_blob(tokio_stream::iter(zero))
            .await
            .unwrap()
            .into_inner()
            .size_bytes,
        0
    );
    let mut duplicate = valid.clone();
    duplicate.insert(1, valid[0].clone());
    let mut gap = valid.clone();
    if let Some(rpc::write_blob_request::Body::Data(c)) = &mut gap[1].body {
        c.offset = 1;
    }
    let mut over = valid.clone();
    if let Some(rpc::write_blob_request::Body::Header(h)) = &mut over[0].body {
        h.size_bytes = 67108865;
    }
    for invalid in [
        vec![],
        valid[1..].to_vec(),
        duplicate,
        gap,
        over,
        valid[..valid.len() - 1].to_vec(),
    ] {
        assert_eq!(
            client
                .write_blob(tokio_stream::iter(invalid))
                .await
                .unwrap_err()
                .code(),
            tonic::Code::InvalidArgument
        );
    }
    // The inclusive 64 MiB boundary is transferred over the real gRPC stream.
    let max_data = vec![19; 67108864];
    let max_frames = frames(
        rpc::RpcContext {
            idempotency_key: "maximum".into(),
            ..context.clone()
        },
        &max_data,
    );
    let maximum = client
        .write_blob(tokio_stream::iter(max_frames))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(maximum.size_bytes, 67108864);
    assert_eq!(maximum.digest, contracts::digest(&max_data));
    let c = f.app.db.connect().await.unwrap();
    assert_eq!(
        c.query_one(
            "SELECT count(*) FROM host_blob_outputs WHERE call_id=$1",
            &[&call]
        )
        .await
        .unwrap()
        .get::<_, i64>(0),
        3
    );
    let mut stream = client
        .read_blob(rpc::ReadBlobRequest {
            context: Some(context.clone()),
            blob_id: source.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let mut actual = Vec::new();
    while let Some(chunk) = stream.message().await.unwrap() {
        assert_eq!(chunk.offset, actual.len() as u64);
        assert!(chunk.data.len() <= 262144);
        actual.extend(chunk.data);
    }
    assert_eq!(actual, source_bytes);
    // The new output belongs to this same user, but was not an input resource.
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(context.clone()),
                blob_id: reference.blob_id.clone()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied
    );
    for spoof in [
        rpc::RpcContext {
            subject_id: Uuid::new_v4().to_string(),
            ..context.clone()
        },
        rpc::RpcContext {
            actor_instance_id: Uuid::new_v4().to_string(),
            ..context.clone()
        },
    ] {
        assert_eq!(
            client
                .read_blob(rpc::ReadBlobRequest {
                    context: Some(spoof),
                    blob_id: source.to_string()
                })
                .await
                .err()
                .unwrap()
                .code(),
            tonic::Code::PermissionDenied
        );
    }
    let mut stale = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/6"
        )))
        .await;
    assert_eq!(
        stale
            .read_blob(rpc::ReadBlobRequest {
                context: Some(context.clone()),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied
    );
    for row in c
        .query("SELECT to_jsonb(a)::text FROM audit_events a", &[])
        .await
        .unwrap()
    {
        assert!(!row.get::<_, String>(0).contains(&context.delegation_token));
    }
    c.execute("UPDATE calls SET state='unknown' WHERE id=$1", &[&call])
        .await
        .unwrap();
    assert_eq!(
        client
            .write_blob(tokio_stream::iter(valid))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::PermissionDenied
    );
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(context),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn live_stream_revocation_and_original_grant_sources_cannot_be_replaced() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let source = f.source(vec![17; 4 * 1024 * 1024]).await;
    let (_, context) = f.delegation(deployment, instance, source).await;
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    let mut stream = client
        .read_blob(rpc::ReadBlobRequest {
            context: Some(context.clone()),
            blob_id: source.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!stream.message().await.unwrap().unwrap().data.is_empty());
    let c = f.app.db.connect().await.unwrap();
    let source_grant: Uuid = c
        .query_one(
            "SELECT id FROM grants WHERE scope->>'kind'='blob' AND role_id='Owner'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    c.execute(
        "UPDATE grants SET status='revoked' WHERE id=$1",
        &[&source_grant],
    )
    .await
    .unwrap();
    let mut ended = false;
    for _ in 0..100 {
        match stream.message().await {
            Err(e) => {
                assert!(matches!(
                    e.code(),
                    tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
                ));
                ended = true;
                break;
            }
            Ok(Some(_)) => (),
            Ok(None) => break,
        }
    }
    assert!(ended, "revocation must interrupt the source stream");
    // A separate currently valid fixture grant proves this is a frozen-source
    // check; general current permission alone would now allow the same object.
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) SELECT $2,subject_id,issuer_id,role_id,scope,'active',created_at,now() FROM grants WHERE id=$1",&[&source_grant,&Uuid::new_v4()]).await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    auth::authorize(
        &c,
        &ctx,
        "blob.read",
        &auth::Resource::new("blob", source, Some(ctx.subject)),
    )
    .await
    .unwrap();
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(context),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[test]
fn resource_scope_requires_explicit_structural_markers() {
    let id = Uuid::new_v4();
    let mut out = std::collections::BTreeSet::new();
    let schema = json!({"type":"object","properties":{"marked":{"type":"array","items":{"type":"string","x-xelab-resource-kind":"blob"}},"unmarked":{"type":"string"}}});
    super::resources(
        &schema,
        &json!({"marked":[id,id],"unmarked":Uuid::new_v4()}),
        &mut out,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert!(out.contains(&id));
    assert!(super::resources(
        &json!({"anyOf":[{"type":"string","x-xelab-resource-kind":"blob"},{"type":"string"}]}),
        &json!(id),
        &mut out
    )
    .is_err());
    assert!(super::resources(
        &json!({"type":"string","x-xelab-resource-kind":"blob"}),
        &json!("not an ID"),
        &mut out
    )
    .is_err());
}
#[tokio::test]
async fn interrupted_upload_and_revoked_source_never_publish_metadata_or_hold_auth_fence() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let source = f.source(b"source".to_vec()).await;
    let (_, context) = f.delegation(deployment, instance, source).await;
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    let data = vec![7; 131072];
    let all = frames(context.clone(), &data);
    let (send, recv) = tokio::sync::mpsc::channel(4);
    send.send(all[0].clone()).await.unwrap();
    send.send(all[1].clone()).await.unwrap();
    let upload = tokio::spawn(async move {
        client
            .write_blob(tokio_stream::wrappers::ReceiverStream::new(recv))
            .await
    });
    let mut started = false;
    for _ in 0..200 {
        if fs::read_dir(f.app.config.root().join("blobs")).is_ok_and(|r| {
            r.filter_map(|r| r.ok())
                .any(|e| e.file_name().to_string_lossy().starts_with("upload-"))
        }) {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(started, "streaming upload must reach private staging");
    let mut c = f.app.db.connect().await.unwrap();
    tokio::time::timeout(Duration::from_secs(1),async {
        let tx=c.transaction().await.unwrap();crate::db::auth_lock(&tx,true).await.unwrap();
        tx.execute("UPDATE credentials SET status='revoked' WHERE id=(SELECT issuer_credential_id FROM rpc_delegations WHERE verification_digest=$1)",&[&f.app.auth.token_digest(&context.delegation_token)]).await.unwrap();tx.commit().await.unwrap();
    }).await.expect("slow upload must not retain the authorization fence");
    send.send(all[2].clone()).await.unwrap();
    drop(send);
    let error = upload.await.unwrap().unwrap_err();
    assert_eq!(error.code(), tonic::Code::Unauthenticated);
    assert_eq!(
        c.query_one("SELECT count(*) FROM blobs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM host_blob_outputs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}
#[tokio::test]
async fn withdrawn_approval_source_is_not_replaced_by_a_new_permission_grant() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let source = f.source(b"scope".to_vec()).await;
    let (_, context) = f.delegation(deployment, instance, source).await;
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    let c = f.app.db.connect().await.unwrap();
    let id: Uuid = c
        .query_one(
            "SELECT id FROM grants WHERE scope->>'kind'='deployment' AND role_id='Owner'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    c.execute("UPDATE grants SET status='revoked' WHERE id=$1", &[&id])
        .await
        .unwrap();
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) SELECT $2,subject_id,issuer_id,role_id,scope,'active',created_at,now() FROM grants WHERE id=$1",&[&id,&Uuid::new_v4()]).await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    auth::authorize(
        &c,
        &ctx,
        "plugin.permission.grant",
        &auth::Resource::new("deployment", deployment, Some(ctx.subject)),
    )
    .await
    .unwrap();
    let error = client
        .read_blob(rpc::ReadBlobRequest {
            context: Some(context.clone()),
            blob_id: source.to_string(),
        })
        .await
        .err()
        .unwrap();
    assert_eq!(error.code(), tonic::Code::PermissionDenied);
    assert!(error.metadata().get_bin("xelab-error-bin").is_some());
    let expired = rpc::RpcContext {
        deadline_unix_ms: chrono::Utc::now().timestamp_millis() - 1,
        ..context
    };
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(expired),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::DeadlineExceeded
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn skill_blob_inputs_and_live_original_skill_authority_are_enforced_over_mtls() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let (deployment, instance) = f.workload().await;
    let source = f.source(b"approved source".to_vec()).await;
    let c = f.app.db.connect().await.unwrap();
    let row = c.query_one("SELECT d.installation_id,d.owner_id,n.manifest FROM deployments d JOIN installations n ON n.id=d.installation_id WHERE d.id=$1", &[&deployment]).await.unwrap();
    let installation: Uuid = row.get("installation_id");
    let owner: Uuid = row.get("owner_id");
    let mut manifest: Value = row.get("manifest");
    let cap = manifest["capabilities"][0].clone();
    let definition = json!({"name":"org.xelab.echo-python.blob","version":"1.0.0","description":"Read only explicitly delegated inputs","input_schema":cap["input_schema"],"output_schema":cap["output_schema"],"stream_item_schema":null,"target":{"kind":"capability","plugin_id":manifest["plugin_id"],"contract":cap["contract"]},"mapping":"identity","maximum_timeout_ms":120000,"input_blobs":true,"tags":[]});
    manifest["skills"] = json!([definition]);
    crate::artifact::validate_profile(&manifest).unwrap();
    c.execute(
        "UPDATE installations SET manifest=$2 WHERE id=$1",
        &[&installation, &manifest],
    )
    .await
    .unwrap();
    c.execute(
        "INSERT INTO capability_contracts(name,version,schema_digest,spec) VALUES($1,$2,$3,$4)",
        &[
            &cap["contract"]["name"].as_str().unwrap(),
            &cap["contract"]["version"].as_str().unwrap(),
            &cap["contract"]["schema_digest"].as_str().unwrap(),
            &cap,
        ],
    )
    .await
    .unwrap();
    let skill = Uuid::new_v4();
    c.execute("INSERT INTO skills(id,name,version,installation_id,created_by,definition,definition_digest,enabled,created_at,updated_at) VALUES($1,$2,'1.0.0',$3,$4,$5,$6,true,now(),now())", &[&skill,&definition["name"].as_str().unwrap(),&installation,&owner,&definition,&crate::skills::definition_digest(&definition).unwrap()]).await.unwrap();
    let (_, no_inputs) = f
        .skill_delegation(deployment, instance, source, Some(skill), false)
        .await;
    let (skill_call, context) = f
        .skill_delegation(deployment, instance, source, Some(skill), true)
        .await;
    let server = tokio::spawn(crate::host::serve(f.app.clone()));
    let mut client = f
        .client(f.client_identity(&format!(
            "spiffe://xelab.local/instance/{instance}/generation/7"
        )))
        .await;
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(no_inputs.clone()),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied
    );
    // Disabling Blob input delegation does not remove independent output authority.
    client
        .write_blob(tokio_stream::iter(frames(
            no_inputs,
            b"output remains approved",
        )))
        .await
        .unwrap();
    let mut stream = client
        .read_blob(rpc::ReadBlobRequest {
            context: Some(context.clone()),
            blob_id: source.to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        stream.message().await.unwrap().unwrap().data,
        b"approved source"
    );
    let disabled = f
        .call(
            "setSkillEnabled",
            &[("skill_id", skill)],
            json!({"enabled":false}),
            Some(1),
            "disable-live-host-skill",
            None,
        )
        .await;
    assert_eq!(disabled.0, 200, "{}", disabled.1);
    assert!(
        client
            .write_blob(tokio_stream::iter(frames(
                context.clone(),
                b"must be refused"
            )))
            .await
            .is_err(),
        "disable must fence HostService before Broker monitor runs"
    );
    let enabled = f
        .call(
            "setSkillEnabled",
            &[("skill_id", skill)],
            json!({"enabled":true}),
            Some(2),
            "enable-live-host-skill",
            None,
        )
        .await;
    assert_eq!(enabled.0, 200, "{}", enabled.1);
    let source_grant: Uuid = c
        .query_one(
            "SELECT id FROM grants WHERE scope->>'kind'='skill' AND role_id='Owner'",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    c.execute(
        "UPDATE grants SET status='revoked' WHERE id=$1",
        &[&source_grant],
    )
    .await
    .unwrap();
    c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) SELECT $2,subject_id,issuer_id,role_id,scope,'active',created_at,now() FROM grants WHERE id=$1", &[&source_grant,&Uuid::new_v4()]).await.unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, "127.0.0.1".parse().unwrap(), Uuid::new_v4())
        .await
        .unwrap();
    auth::authorize(
        &c,
        &ctx,
        "skill.invoke",
        &auth::Resource::new("skill", skill, Some(owner)),
    )
    .await
    .unwrap();
    assert_eq!(
        super::check_call_sources(&c, &ctx, skill_call)
            .await
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert_eq!(
        client
            .read_blob(rpc::ReadBlobRequest {
                context: Some(context),
                blob_id: source.to_string()
            })
            .await
            .err()
            .unwrap()
            .code(),
        tonic::Code::PermissionDenied,
        "replacement grants cannot revive the frozen original Skill source"
    );
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn skill_frozen_business_proofs_exclude_exhausted_uncharged_alternatives() {
    use std::collections::BTreeSet;
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let peer = "127.0.0.1".parse().unwrap();
    let ctx = f
        .app
        .auth
        .authenticate(&c, &f.token, peer, Uuid::new_v4())
        .await
        .unwrap();
    let keys = [Uuid::new_v4(), Uuid::new_v4()];
    let grants = [Uuid::new_v4(), Uuid::new_v4()];
    for (key, grant) in keys.iter().zip(grants) {
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,status,inheritance_mode,conditions,expires_at,created_at,updated_at) VALUES($1,$2,'api_key','fixture',$3,'active','follow_parent','{\"max_calls\":1}',now()+interval '1 hour',now(),now())", &[&key,&ctx.subject,&&key.as_bytes()[..]]).await.unwrap();
        c.execute("INSERT INTO grants(id,subject_id,issuer_id,issuer_credential_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$2,$3,'Owner','{\"kind\":\"capability\",\"match\":\"all\"}','active',now(),now())", &[&grant,&ctx.subject,&key]).await.unwrap();
    }
    let prior = Uuid::new_v4();
    let call = Uuid::new_v4();
    // Shared RPC/HTTP source freezing resolves the action from the fixed
    // persisted Call target; even a ledger unit fixture needs a real ContractRef.
    let contract = json!({"name":"fixture","version":"1.0.0","schema_digest":crate::contracts::digest(b"fixture contract")});
    crate::contracts::validate("ContractRef", &contract).unwrap();
    for (id, credential) in [(prior, keys[0]), (call, ctx.credential)] {
        c.execute("INSERT INTO calls(id,owner_id,credential_id,contract,input_digest,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,'succeeded',now()+interval '1 minute',now())", &[&id,&ctx.subject,&credential,&contract,&crate::contracts::digest(b"fixture input")]).await.unwrap();
    }
    assert_eq!(
        crate::identity::key_calls(&c, keys[0]).await.unwrap(),
        1,
        "the alternative was exhausted before this call"
    );
    for credential in ctx.chain.iter().copied().chain([keys[1]]) {
        c.execute(
            "INSERT INTO call_credential_charges(call_id,credential_id) VALUES($1,$2)",
            &[&call, &credential],
        )
        .await
        .unwrap();
    }
    let mut paths = Vec::new();
    for (action, kind) in [
        ("capability.invoke", "capability"),
        ("skill.invoke", "skill"),
    ] {
        for grant in grants {
            paths.push(crate::policy::Path {
                rule: json!({"action":action,"resource":{"kind":kind,"match":"all"}}),
                sources: BTreeSet::from([grant]),
            });
        }
    }
    let filtered = super::charged_business_paths(&c, &ctx, call, paths)
        .await
        .unwrap();
    assert_eq!(filtered.len(), 2);
    assert!(
        filtered
            .iter()
            .all(|path| path.sources == BTreeSet::from([grants[1]])),
        "exhausted uncharged alternatives must never enter frozen business authority"
    );
    // The currently charged call may fill its issuer's last slot. Rechecking
    // admission budgets here would incorrectly withdraw its own valid proof.
    assert_eq!(crate::identity::key_calls(&c, keys[1]).await.unwrap(), 1);
    let repeated = super::charged_business_paths(&c, &ctx, call, filtered)
        .await
        .unwrap();
    assert_eq!(repeated.len(), 2);
    let live = crate::policy::Policy::new(&c, peer)
        .revalidate(&repeated, ctx.credential)
        .await
        .unwrap();
    assert!(auth::rules_allow(
        &crate::policy::rules(&live),
        "capability.invoke",
        &auth::Resource::new("capability", "fixture@1.0.0", None),
        peer
    ));
    c.execute(
        "UPDATE grants SET status='revoked' WHERE id=$1",
        &[&grants[1]],
    )
    .await
    .unwrap();
    auth::authorize(
        &c,
        &ctx,
        "capability.invoke",
        &auth::Resource::new("capability", "fixture@1.0.0", None),
    )
    .await
    .unwrap();
    let withdrawn = crate::policy::Policy::new(&c, peer)
        .revalidate(&repeated, ctx.credential)
        .await
        .unwrap();
    assert!(!auth::rules_allow(&crate::policy::rules(&withdrawn),"capability.invoke",&auth::Resource::new("capability","fixture@1.0.0",None),peer),"an uncharged current alternative must not sustain the admitted work after its charged proof is revoked");
}

#[path = "m3_result_authority_tests.rs"]
mod result_authority;

#[path = "host_calls_tests.rs"]
mod host_calls_tests;

#[path = "host_events_tests.rs"]
mod host_events_tests;
