//! PostgreSQL and real loopback workload mTLS. Installed manifests and runtime
//! readiness are component fixtures, with no OCI or host privileges required.
use crate::{
    api::{self, App},
    auth::{self, Auth},
    config::Config,
    contracts::{digest, id, validate},
    db::Db,
    rpc,
    runtime::Runtime,
    task_engine::{self, AttemptLease},
    tls,
};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, SanType,
};
use serde_json::{json, Value};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify, Semaphore},
    task::JoinHandle,
};
use tokio_postgres::Client;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    transport::{Certificate as TonicCertificate, Identity, Server, ServerTlsConfig},
    Response, Status,
};
use tower::ServiceExt;
use uuid::Uuid;

const GENERATION: i64 = 7;
const PASSWORD: &str = "correct horse task dispatch fixture password";

struct Fixture {
    app: Arc<App>,
    router: Router,
    owner: Uuid,
    token: String,
    _directory: tempfile::TempDir,
    ca: Certificate,
    ca_key: KeyPair,
    installation: Uuid,
    contract: Value,
    deployment: Uuid,
    instance: Uuid,
}

impl Fixture {
    async fn new() -> Option<Self> {
        crate::init_crypto();
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: signed Task dispatch requires XELAB_TEST_DATABASE_URL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("task_dispatch_{}", Uuid::new_v4().simple());
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
        let mut params = CertificateParams::new(vec!["task-dispatch-ca".into()]).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "task-dispatch-ca");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca = params.self_signed(&ca_key).unwrap();
        fs::write(root.join("ca.crt"), ca.pem()).unwrap();
        tls::atomic_file(
            &root.join("ca.key"),
            ca_key.serialize_pem().as_bytes(),
            0o600,
        )
        .unwrap();
        tls::atomic_file(&root.join("master.key"), &[61; 32], 0o600).unwrap();
        let source: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let mut config = serde_json::to_value(source).unwrap();
        config["storage"]["root_path"] = json!(root.join("state"));
        config["security"]["master_key_file"] = json!(root.join("master.key"));
        config["security"]["workload_ca_cert_file"] = json!(root.join("ca.crt"));
        config["security"]["workload_ca_key_file"] = json!(root.join("ca.key"));
        let config = Config::from_value(config).unwrap();
        fs::create_dir_all(config.root()).unwrap();
        fs::set_permissions(config.root(), fs::Permissions::from_mode(0o700)).unwrap();
        tls::issue(&config, None, &config.root().join("kernel-identity")).unwrap();

        let mut connection = db.connect().await.unwrap();
        let tx = connection.transaction().await.unwrap();
        let owner = auth::bootstrap(&tx, "Owner", PASSWORD, &config.value["quota_defaults"])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let auth = Auth::new(vec![63; 32]).unwrap();
        let token = auth
            .login(
                &connection,
                owner,
                PASSWORD.into(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_owned();

        let publisher = Uuid::new_v4();
        let pem = SigningKey::from_bytes(&[15; 32])
            .verifying_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        connection
            .execute(
                "INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at)
            VALUES($1,'Task dispatch fixture',$2,$3,'trusted',now(),now())",
                &[&publisher, &publisher.to_string(), &pem],
            )
            .await
            .unwrap();
        let mut manifest: Value =
            serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
        manifest["publisher_id"] = json!(publisher);
        manifest["requested_rules"] = json!([]);
        crate::artifact::validate_profile(&manifest).unwrap();
        let installation = Uuid::new_v4();
        let manifest_digest = digest(&serde_json::to_vec(&manifest).unwrap());
        connection
            .execute(
                "INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,
                  manifest_digest,manifest,state,created_at,updated_at)
             VALUES($1,$2,'1.0.0',$3,$4,$4,$5,'installed',now(),now())",
                &[
                    &installation,
                    &manifest["plugin_id"].as_str().unwrap(),
                    &publisher,
                    &manifest_digest,
                    &manifest,
                ],
            )
            .await
            .unwrap();
        let capability = &manifest["capabilities"][0];
        let contract = capability["contract"].clone();
        connection
            .execute(
                "INSERT INTO capability_contracts(name,version,schema_digest,spec)
             VALUES($1,$2,$3,$4)",
                &[
                    &contract["name"].as_str().unwrap(),
                    &contract["version"].as_str().unwrap(),
                    &contract["schema_digest"].as_str().unwrap(),
                    &capability,
                ],
            )
            .await
            .unwrap();
        let (deployment, instance) =
            install_provider(&connection, owner, installation, &contract).await;
        let leader = Arc::new(db.leader().await.unwrap());
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db,
            auth,
            leader,
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: Semaphore::new(4),
        });
        Some(Self {
            router: api::router(app.clone()),
            app,
            owner,
            token,
            _directory: directory,
            ca,
            ca_key,
            installation,
            contract,
            deployment,
            instance,
        })
    }

    async fn post(&self, uri: &str, key: &str, body: Value) -> Value {
        let mut request = Request::post(uri)
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.app.config).unwrap(),
            )
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .header("idempotency-key", key)
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.as_u16(), 201, "{uri}: {value}");
        value
    }

    async fn start_task(&self, downstream: bool) -> Uuid {
        let mut steps = vec![json!({
            "id":"echo","contract":self.contract,"provider_deployment_id":self.deployment,
            "depends_on":[],"input_mapping":{"message":{"task_input":"/message"}},
            "timeout_ms":10_000
        })];
        if downstream {
            steps.push(json!({
                "id":"next","contract":self.contract,"provider_deployment_id":self.deployment,
                "depends_on":["echo"],"input_mapping":{"message":{
                    "step_output":{"step":"echo","pointer":"/message"}
                }},"timeout_ms":10_000
            }));
        }
        let workflow = self.post("/api/platform/v1/workflows", "signed-dispatch-workflow", json!({
            "name":"signed-dispatch-workflow","input_schema":{
                "type":"object","required":["message"],"properties":{"message":{"type":"string"}}
            },"steps":steps
        })).await;
        let task = self
            .post(
                "/api/platform/v1/tasks",
                "signed-dispatch-task",
                json!({
                    "workflow_id": workflow["id"], "input":{"message":"from workflow"}
                }),
            )
            .await;
        id(&task, "id").unwrap()
    }

    async fn serve(
        &self,
        instance: Uuid,
        certificate_generation: i64,
        provider: Provider,
    ) -> ServerGuard {
        let listener = listener().await;
        let root = self
            .app
            .config
            .root()
            .join("runtime")
            .join(instance.to_string());
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("network.json"),
            serde_json::to_vec(&json!({
                "instance":instance,"peer":listener.local_addr().unwrap().ip()
            }))
            .unwrap(),
        )
        .unwrap();
        let key = KeyPair::generate().unwrap();
        let name = format!("i-{instance}-g-{certificate_generation}.xelab.local");
        let mut params = CertificateParams::new(vec![name.clone()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.subject_alt_names.push(SanType::URI(
            format!("spiffe://xelab.local/instance/{instance}/generation/{certificate_generation}")
                .try_into()
                .unwrap(),
        ));
        let leaf = params.signed_by(&key, &self.ca, &self.ca_key).unwrap();
        let identity = Identity::from_pem(leaf.pem(), key.serialize_pem());
        let ca = TonicCertificate::from_pem(self.ca.pem());
        let server = Server::builder()
            .tls_config(ServerTlsConfig::new().identity(identity).client_ca_root(ca))
            .unwrap()
            .add_service(rpc::capability_service_server::CapabilityServiceServer::new(provider));
        ServerGuard(tokio::spawn(async move {
            server
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        }))
    }
}

async fn install_provider(
    c: &Client,
    owner: Uuid,
    installation: Uuid,
    contract: &Value,
) -> (Uuid, Uuid) {
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    c.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,observed_generation,created_at,updated_at)
             SELECT $1,$2,id,jsonb_build_object('installation_id',id,'enabled',true,
                          'limits',manifest->'minimum_resources','volume_ids','[]'::jsonb,'replicas',1),
                    'ready',$4,now(),now() FROM installations WHERE id=$3",
        &[&deployment, &owner, &installation, &GENERATION]).await.unwrap();
    c.execute(
        "INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,runtime_id,
                  last_heartbeat_at,created_at,updated_at)
             SELECT $1,$2,$3,0,'ready','alive',spec->'limits',$4,now(),now(),now()
               FROM deployments WHERE id=$2",
        &[&instance, &deployment, &GENERATION, &instance.to_string()],
    )
    .await
    .unwrap();
    c.execute("INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at)
             VALUES($1,$2,$3,'ready','[]',$4,now())",
        &[&instance, &contract["name"].as_str().unwrap(), &contract["version"].as_str().unwrap(),
          &GENERATION]).await.unwrap();
    (deployment, instance)
}

async fn listener() -> TcpListener {
    for _ in 0..16 {
        let b = *Uuid::new_v4().as_bytes();
        let address = Ipv4Addr::new(127, 30 + b[0] % 150, b[1], 2 + b[2] % 250);
        if let Ok(listener) = TcpListener::bind((address, 7000)).await {
            return listener;
        }
    }
    panic!("could not bind the workload fixture's loopback RPC port");
}

struct ServerGuard(JoinHandle<()>);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone, Default)]
struct Provider {
    requests: Arc<Mutex<Vec<rpc::InvokeRequest>>>,
    entered: Arc<Notify>,
    gate: Option<Arc<Semaphore>>,
    reply: ProviderReply,
}

#[derive(Clone, Copy, Default)]
enum ProviderReply {
    #[default]
    Echo,
    Unavailable,
    Rejected,
}

#[tonic::async_trait]
impl rpc::capability_service_server::CapabilityService for Provider {
    type InvokeStreamStream = tokio_stream::Empty<Result<rpc::InvokeChunk, Status>>;

    async fn invoke(
        &self,
        request: tonic::Request<rpc::InvokeRequest>,
    ) -> Result<Response<rpc::InvokeResponse>, Status> {
        let request = tls::require_kernel(request)?.into_inner();
        self.requests.lock().await.push(request.clone());
        self.entered.notify_one();
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        match self.reply {
            ProviderReply::Unavailable => {
                return Err(Status::unavailable(
                    "workload interrupted before publishing a result",
                ));
            }
            ProviderReply::Rejected => {
                return Ok(Response::new(rpc::InvokeResponse {
                    call_id: request.call_id,
                    terminal_failure: Some(rpc::InvokeFailure {
                        version: request.terminal_failure_version,
                        contract: request.contract,
                        code: "INVALID_ARGUMENT".into(),
                    }),
                    ..rpc::InvokeResponse::default()
                }));
            }
            ProviderReply::Echo => (),
        }
        let input: Value = serde_json::from_slice(&request.input_json).unwrap();
        let message = input["message"].as_str().unwrap();
        let bytes = serde_json::to_vec(&json!({"message":format!("echo {message}")})).unwrap();
        Ok(Response::new(rpc::InvokeResponse {
            call_id: request.call_id,
            output_digest: digest(&bytes),
            output_json: bytes,
            terminal_failure: None,
        }))
    }

    async fn invoke_stream(
        &self,
        _: tonic::Request<rpc::InvokeRequest>,
    ) -> Result<Response<Self::InvokeStreamStream>, Status> {
        Err(Status::unimplemented("unary fixture"))
    }

    async fn cancel(
        &self,
        _: tonic::Request<rpc::CancelCallRequest>,
    ) -> Result<Response<rpc::Ack>, Status> {
        Err(Status::unimplemented("fixture cancellation"))
    }

    async fn query_result(
        &self,
        _: tonic::Request<rpc::QueryResultRequest>,
    ) -> Result<Response<rpc::QueryResultResponse>, Status> {
        Err(Status::unimplemented("fixture reconciliation"))
    }
}

async fn claim(fixture: &Fixture) -> AttemptLease {
    let worker_id = Uuid::new_v4();
    let attempt = task_engine::claim(&fixture.app.db, &fixture.app.auth, worker_id)
        .await
        .unwrap()
        .expect("ready workflow step");
    AttemptLease {
        worker_id,
        attempt_id: id(&attempt, "id").unwrap(),
        execution_id: id(&attempt, "execution_id").unwrap(),
        fence: attempt["fence"].as_i64().unwrap(),
    }
}

async fn task_state(c: &Client, task: Uuid) -> (String, Value) {
    let row = c
        .query_one(
            "SELECT state,coalesce(output,'null') FROM tasks WHERE id=$1",
            &[&task],
        )
        .await
        .unwrap();
    (row.get(0), row.get(1))
}

async fn check_settlement(
    c: &Client,
    task: Uuid,
    request: &rpc::InvokeRequest,
    instance: Uuid,
    expected: &str,
) -> Uuid {
    let call = Uuid::parse_str(&request.call_id).unwrap();
    let row = c
        .query_one(
            "SELECT c.state,c.provider_instance_id,r.status,e.status,t.status,
                d.status,d.generation,c.owner_id=c2.owner_id AS same_owner,
                c.deadline_at=r.expires_at AND c.deadline_at=e.expires_at
                  AND c.deadline_at=d.expires_at AS uniform_deadline,
                h.proxy_source_paths='[]' AND h.proxy_ceilings='[]' AND h.read_blob_ids='[]'
                  AS no_proxy_authority, e.maximum_rules AS call_maximum_rules
           FROM calls c JOIN reservations r ON r.id=c.id JOIN rpc_delegations d ON d.call_id=c.id
           JOIN execution_delegations e ON e.id=c.delegation_id
           JOIN host_rpc_contexts h ON h.call_id=c.id
           JOIN tasks c2 ON c2.id=$1 JOIN execution_delegations t ON t.id=c2.delegation_id
          WHERE c.id=$2",
            &[&task, &call],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), expected);
    assert_eq!(row.get::<_, Uuid>(1), instance);
    let reservation = if expected == "unknown" {
        "active"
    } else {
        "released"
    };
    assert_eq!(row.get::<_, &str>(2), reservation);
    assert_eq!(
        row.get::<_, &str>(3),
        "revoked",
        "Call execution delegation was left active"
    );
    assert_eq!(
        row.get::<_, &str>(4),
        "active",
        "Call settlement revoked the original task delegation"
    );
    assert_eq!(
        row.get::<_, &str>(5),
        "revoked",
        "workload invocation handle was left active"
    );
    assert_eq!(row.get::<_, i64>(6), GENERATION);
    assert!(row.get::<_, bool>(7));
    assert!(row.get::<_, bool>("uniform_deadline"));
    assert!(row.get::<_, bool>("no_proxy_authority"));
    let contract = request.contract.as_ref().unwrap();
    assert_eq!(
        row.get::<_, Value>("call_maximum_rules"),
        json!([{
            "action":"capability.invoke","resource":{"kind":"capability","match":"ids",
                "ids":[format!("{}@{}", contract.name, contract.version)]},"conditions":{}
        }])
    );
    call
}

#[tokio::test]
async fn admitted_task_connects_with_mtls_and_reentry_sends_only_once() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(true).await;
    let lease = claim(&f).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .expect("the admitted lease must reach its mTLS provider");
    let reentry = tokio::time::timeout(
        Duration::from_secs(3),
        task_engine::dispatch(&f.app, &lease),
    )
    .await
    .expect("reentry must not wait for the held provider")
    .unwrap_err();
    assert_eq!(reentry.code, "OUTCOME_UNKNOWN", "{reentry}");
    let first = provider.requests.lock().await[0].clone();
    assert_eq!(provider.requests.lock().await.len(), 1);
    assert_eq!(reentry.resource_id.as_deref(), Some(first.call_id.as_str()));
    assert_eq!(first.input_json, br#"{"message":"from workflow"}"#);
    let ctx = first.context.as_ref().unwrap();
    assert!(!ctx.delegation_token.is_empty());
    assert_eq!(ctx.subject_id, f.owner.to_string());
    assert!(Uuid::parse_str(&ctx.request_id).is_ok());
    assert_eq!(first.input_digest, digest(&first.input_json));
    let c = f.app.db.connect().await.unwrap();
    let pinned = c
        .query_one(
            "SELECT b.call_id,a.idempotency_key,(SELECT count(*) FROM calls)
         FROM attempts a JOIN task_attempt_calls b ON b.attempt_id=a.id WHERE a.id=$1",
            &[&lease.attempt_id],
        )
        .await
        .unwrap();
    assert_eq!(pinned.get::<_, Uuid>(0).to_string(), first.call_id);
    assert_eq!(pinned.get::<_, &str>(1), ctx.idempotency_key);
    assert_eq!(pinned.get::<_, i64>(2), 1);
    gate.add_permits(1);
    let applied = in_flight.await.unwrap().unwrap();
    validate("TaskStep", &applied).unwrap();
    assert_eq!(applied["state"], "succeeded");
    let output: Value = c
        .query_one(
            "SELECT output FROM task_steps WHERE id=$1",
            &[&id(&applied, "id").unwrap()],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(output, json!({"message":"echo from workflow"}));
    let first_call = check_settlement(&c, task, &first, f.instance, "succeeded").await;
    assert_eq!(
        task_engine::dispatch(&f.app, &lease).await.unwrap(),
        applied
    );
    assert_eq!(provider.requests.lock().await.len(), 1);
    let next = claim(&f).await;
    gate.add_permits(1);
    let step = task_engine::dispatch(&f.app, &next).await.unwrap();
    assert_eq!(step["state"], "succeeded");
    assert_eq!(
        task_state(&c, task).await,
        (
            "succeeded".into(),
            json!({"message":"echo echo from workflow"})
        )
    );
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[1].call_id, first_call.to_string());
    assert_eq!(
        requests[1].input_json,
        br#"{"message":"echo from workflow"}"#
    );
}

#[tokio::test]
async fn revoked_authority_and_stale_worker_send_nothing() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    let stale = AttemptLease {
        fence: lease.fence + 1,
        ..lease
    };
    let error = task_engine::dispatch(&f.app, &stale).await.unwrap_err();
    assert_eq!(error.code, "STATE_CONFLICT", "{error}");
    c.execute(
        "UPDATE execution_delegations SET status='revoked'
                WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        &[&task],
    )
    .await
    .unwrap();
    let error = task_engine::dispatch(&f.app, &lease).await.unwrap_err();
    assert_eq!(error.code, "PERMISSION_DENIED", "{error}");
    assert!(provider.requests.lock().await.is_empty());
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn dispatch_refuses_an_old_generation_certificate_without_trying_another_deployment() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION - 1, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    let (_, spare) = install_provider(&c, f.owner, f.installation, &f.contract).await;
    let replacement = Provider::default();
    let _spare_server = f.serve(spare, GENERATION, replacement.clone()).await;
    let error = task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("old generation TLS must not reach a workload provider");
    assert_eq!(error.message, "Workload TLS connection failed", "{error}");
    let call = Uuid::parse_str(error.resource_id.as_deref().unwrap()).unwrap();
    let state = c
        .query_one(
            "SELECT state,provider_instance_id FROM calls WHERE id=$1",
            &[&call],
        )
        .await
        .unwrap();
    assert_eq!(state.get::<_, &str>(0), "unknown");
    assert_eq!(state.get::<_, Uuid>(1), f.instance);
    assert_eq!(
        task_state(&c, task).await,
        ("unknown".to_owned(), Value::Null)
    );
    assert!(provider.requests.lock().await.is_empty());
    assert!(replacement.requests.lock().await.is_empty());
}

#[tokio::test]
async fn replacing_the_admitted_installation_does_not_dispatch_to_its_replacement() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    let replacement = Uuid::new_v4();
    let mut manifest: Value = c
        .query_one(
            "SELECT manifest FROM installations WHERE id=$1",
            &[&f.installation],
        )
        .await
        .unwrap()
        .get(0);
    manifest["version"] = json!("1.0.1");
    crate::artifact::validate_profile(&manifest).unwrap();
    let hash = digest(&serde_json::to_vec(&manifest).unwrap());
    c.execute(
        "INSERT INTO installations(id,plugin_id,version,publisher_id,image_digest,manifest_digest,
                                        manifest,state,created_at,updated_at)
              SELECT $2,plugin_id,'1.0.1',publisher_id,$3,$3,$4,'installed',now(),now()
                FROM installations WHERE id=$1",
        &[&f.installation, &replacement, &hash, &manifest],
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE deployments SET installation_id=$2,
                    spec=jsonb_set(spec,'{installation_id}',to_jsonb($2::uuid)) WHERE id=$1",
        &[&f.deployment, &replacement],
    )
    .await
    .unwrap();
    task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("the original installation changed after claim");
    assert!(provider.requests.lock().await.is_empty());
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}

#[tokio::test]
async fn interrupted_provider_leaves_attempt_and_call_unconfirmed_without_replay() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider {
        reply: ProviderReply::Unavailable,
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let error = task_engine::dispatch(&f.app, &lease).await.unwrap_err();
    assert_eq!(error.code, "NOT_READY", "{error}");
    let first = provider.requests.lock().await[0].clone();
    assert_eq!(error.resource_id.as_deref(), Some(first.call_id.as_str()));
    let c = f.app.db.connect().await.unwrap();
    check_settlement(&c, task, &first, f.instance, "unknown").await;
    assert_eq!(
        task_state(&c, task).await,
        ("unknown".to_owned(), Value::Null)
    );
    assert_eq!(
        c.query_one(
            "SELECT state FROM attempts WHERE id=$1",
            &[&lease.attempt_id]
        )
        .await
        .unwrap()
        .get::<_, &str>(0),
        "unknown"
    );
    let replay = task_engine::dispatch(&f.app, &lease).await.unwrap_err();
    assert_eq!(replay.code, "OUTCOME_UNKNOWN", "{replay}");
    assert_eq!(replay.resource_id, error.resource_id);
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn result_after_current_task_deadline_remains_unconfirmed() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), provider.entered.notified())
        .await
        .expect("the admitted lease must reach its mTLS provider");
    let c = f.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE tasks SET deadline_at=clock_timestamp()-interval '1 millisecond' WHERE id=$1",
        &[&task],
    )
    .await
    .unwrap();
    gate.add_permits(1);
    let failure = in_flight
        .await
        .unwrap()
        .expect_err("late result cannot complete an expired task");
    let request = provider.requests.lock().await[0].clone();
    assert_eq!(
        failure.resource_id.as_deref(),
        Some(request.call_id.as_str())
    );
    check_settlement(&c, task, &request, f.instance, "unknown").await;
    let (state, output) = task_state(&c, task).await;
    assert_ne!(state, "succeeded");
    assert_eq!(output, Value::Null);
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn failing_attempt_link_rolls_back_admission_and_preserves_one_send_on_retry() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    let quota_sql = "SELECT to_jsonb(q) FROM quotas q WHERE account_id=$1";
    let quota_before: Value = c.query_one(quota_sql, &[&f.owner]).await.unwrap().get(0);
    let attempt_sql = "SELECT to_jsonb(a) FROM attempts a WHERE a.id=$1";
    let attempt_before: Value = c
        .query_one(attempt_sql, &[&lease.attempt_id])
        .await
        .unwrap()
        .get(0);
    c.batch_execute(
        "CREATE FUNCTION reject_fixture_link() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'fixture link insert rejected'; END $$;
        CREATE TRIGGER reject_fixture_link BEFORE INSERT ON task_attempt_calls
        FOR EACH ROW EXECUTE FUNCTION reject_fixture_link();",
    )
    .await
    .unwrap();
    task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("failed link must abort admission");
    let unchanged = c
        .query_one(
            "SELECT NOT EXISTS(SELECT FROM calls)
        AND NOT EXISTS(SELECT FROM task_attempt_calls)
        AND NOT EXISTS(SELECT FROM reservations WHERE kind='call')
        AND NOT EXISTS(SELECT FROM call_credential_charges)
        AND NOT EXISTS(SELECT FROM rpc_delegations)",
            &[],
        )
        .await
        .unwrap()
        .get::<_, bool>(0);
    assert!(
        unchanged,
        "a partial Call admission or charge survived rollback"
    );
    assert_eq!(
        c.query_one(quota_sql, &[&f.owner])
            .await
            .unwrap()
            .get::<_, Value>(0),
        quota_before
    );
    assert_eq!(
        c.query_one(attempt_sql, &[&lease.attempt_id])
            .await
            .unwrap()
            .get::<_, Value>(0),
        attempt_before
    );
    assert!(provider.requests.lock().await.is_empty());

    c.batch_execute(
        "DROP TRIGGER reject_fixture_link ON task_attempt_calls;
                     DROP FUNCTION reject_fixture_link();",
    )
    .await
    .unwrap();
    let step = task_engine::dispatch(&f.app, &lease).await.unwrap();
    assert_eq!(step["state"], "succeeded");
    assert_eq!(task_engine::dispatch(&f.app, &lease).await.unwrap(), step);
    assert_eq!(task_state(&c, task).await.0, "succeeded");
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn result_audit_past_the_deadline_rolls_back_success_and_retains_capacity() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    // A PostgreSQL sequence records that the success path reached the final
    // audit write even when its subtransaction is later rolled back.
    c.batch_execute(
        "CREATE SEQUENCE fixture_success_audit;
        CREATE FUNCTION delay_fixture_success_audit() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
          PERFORM nextval('fixture_success_audit');
          PERFORM pg_sleep_until(deadline_at + interval '50 milliseconds') FROM tasks;
          RETURN NEW;
        END $$;
        CREATE TRIGGER delay_fixture_success_audit BEFORE INSERT ON audit_events
        FOR EACH ROW WHEN (NEW.reason_code = 'CALL_COMPLETED')
        EXECUTE FUNCTION delay_fixture_success_audit();",
    )
    .await
    .unwrap();
    c.execute(
        "UPDATE tasks SET deadline_at=clock_timestamp()+interval '3 seconds' WHERE id=$1",
        &[&task],
    )
    .await
    .unwrap();
    let quota_sql = "SELECT concurrent_calls_used FROM quotas WHERE account_id=$1";
    let quota_before: i64 = c.query_one(quota_sql, &[&f.owner]).await.unwrap().get(0);
    let error = task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("late settlement cannot confirm success");
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        error.resource_id.as_deref(),
        Some(requests[0].call_id.as_str())
    );
    assert!(
        c.query_one("SELECT is_called FROM fixture_success_audit", &[])
            .await
            .unwrap()
            .get::<_, bool>(0),
        "the test did not enter success settlement"
    );
    check_settlement(&c, task, &requests[0], f.instance, "unknown").await;
    assert_eq!(
        c.query_one(quota_sql, &[&f.owner])
            .await
            .unwrap()
            .get::<_, i64>(0),
        quota_before + 1
    );
    assert!(
        c.query_one(
            "SELECT NOT EXISTS(SELECT FROM audit_events WHERE reason_code='CALL_COMPLETED')",
            &[]
        )
        .await
        .unwrap()
        .get::<_, bool>(0),
        "confirmed audit survived its rolled-back result"
    );
    let (state, output) = task_state(&c, task).await;
    assert_ne!(state, "succeeded");
    assert_eq!(output, Value::Null);
}

#[tokio::test]
async fn reentry_recovers_only_the_saved_call_receipt_after_a_task_commit_failure() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let c = f.app.db.connect().await.unwrap();
    c.batch_execute(
        "CREATE FUNCTION reject_fixture_commit() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'fixture Task receipt insert rejected'; END $$;
        CREATE TRIGGER reject_fixture_commit BEFORE INSERT ON attempt_commit_receipts
        FOR EACH ROW EXECUTE FUNCTION reject_fixture_commit();",
    )
    .await
    .unwrap();
    let error = task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("the Call completed but its Task receipt did not commit");
    let first = provider.requests.lock().await[0].clone();
    assert_eq!(error.resource_id.as_deref(), Some(first.call_id.as_str()));
    let call = check_settlement(&c, task, &first, f.instance, "succeeded").await;
    assert_eq!(task_state(&c, task).await, ("running".into(), Value::Null));
    c.batch_execute(
        "DROP TRIGGER reject_fixture_commit ON attempt_commit_receipts;
                    DROP FUNCTION reject_fixture_commit();",
    )
    .await
    .unwrap();

    let result = |message: &str| {
        json!({
            "attempt_id":lease.attempt_id,"execution_id":lease.execution_id,"fence":lease.fence,
            "state":"succeeded","output":{"message":message},"error":null
        })
    };
    task_engine::commit(
        &f.app.db,
        &f.app.auth,
        lease.worker_id,
        &result("forged output"),
    )
    .await
    .expect_err("a live fence alone cannot change the saved Call output");

    let proof: Value = c
        .query_one(
            "SELECT source_paths FROM host_rpc_contexts WHERE call_id=$1",
            &[&call],
        )
        .await
        .unwrap()
        .get(0);
    c.execute(
        "UPDATE host_rpc_contexts SET source_paths='[]' WHERE call_id=$1",
        &[&call],
    )
    .await
    .unwrap();
    task_engine::commit(
        &f.app.db,
        &f.app.auth,
        lease.worker_id,
        &result("echo from workflow"),
    )
    .await
    .expect_err("exact output without the Call's saved charged source cannot commit");
    c.execute(
        "UPDATE host_rpc_contexts SET source_paths=$2 WHERE call_id=$1",
        &[&call, &proof],
    )
    .await
    .unwrap();
    assert_eq!(task_state(&c, task).await, ("running".into(), Value::Null));
    assert!(c
        .query_one(
            "SELECT NOT EXISTS(SELECT FROM attempt_commit_receipts WHERE attempt_id=$1)",
            &[&lease.attempt_id]
        )
        .await
        .unwrap()
        .get::<_, bool>(0));

    let recovered = task_engine::dispatch(&f.app, &lease).await.unwrap();
    assert_eq!(recovered["state"], "succeeded");
    assert_eq!(
        task_state(&c, task).await,
        ("succeeded".into(), json!({"message":"echo from workflow"}))
    );
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn signed_terminal_rejection_commits_and_replays_without_resending() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let provider = Provider {
        reply: ProviderReply::Rejected,
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let returned = task_engine::dispatch(&f.app, &lease).await.unwrap();
    validate("TaskStep", &returned).unwrap();
    assert_eq!(returned["state"], "failed");
    let first = provider.requests.lock().await[0].clone();
    assert_eq!(first.terminal_failure_version, 1);
    let c = f.app.db.connect().await.unwrap();
    let call = check_settlement(&c, task, &first, f.instance, "failed").await;
    let row = c
        .query_one(
            "SELECT c.error->>'code',r.state,c.error=r.error AS exact_receipt
              FROM calls c JOIN task_attempt_calls l ON l.call_id=c.id
              JOIN attempt_commit_receipts r ON r.attempt_id=l.attempt_id
             WHERE c.id=$1 AND l.attempt_id=$2",
            &[&call, &lease.attempt_id],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), "INVALID_ARGUMENT");
    assert_eq!(row.get::<_, &str>(1), "failed");
    assert!(row.get::<_, bool>("exact_receipt"));
    assert_eq!(task_state(&c, task).await, ("failed".into(), Value::Null));
    assert_eq!(
        task_engine::dispatch(&f.app, &lease).await.unwrap(),
        returned
    );
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn task_admission_selects_live_replica_inside_the_fixed_deployment() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f.start_task(false).await;
    let lease = claim(&f).await;
    let c = f.app.db.connect().await.unwrap();
    for (id, ordinal, health, observation) in [
        (Uuid::nil(), 1_i32, "unhealthy", GENERATION),
        (Uuid::from_u128(1), 2_i32, "alive", GENERATION - 1),
    ] {
        assert!(id < f.instance);
        c.execute(
            "INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,runtime_id,
                                   last_heartbeat_at,created_at,updated_at)
             SELECT $2,deployment_id,generation,$3,'ready',$4,limits,$5,now(),now(),now()
               FROM instances WHERE id=$1",
            &[&f.instance, &id, &ordinal, &health, &id.to_string()],
        )
        .await
        .unwrap();
        c.execute(
            "INSERT INTO provider_records(instance_id,name,version,availability,features,observed_generation,observed_at)
             SELECT $2,name,version,'ready',features,$3,clock_timestamp() FROM provider_records WHERE instance_id=$1",
            &[&f.instance, &id, &observation],
        )
        .await
        .unwrap();
    }
    let healthy = Provider::default();
    let _server = f.serve(f.instance, GENERATION, healthy.clone()).await;
    let step = task_engine::dispatch(&f.app, &lease).await.unwrap();
    assert_eq!(step["state"], "succeeded");
    let requests = healthy.requests.lock().await;
    assert_eq!(requests.len(), 1);
    check_settlement(&c, task, &requests[0], f.instance, "succeeded").await;
}
