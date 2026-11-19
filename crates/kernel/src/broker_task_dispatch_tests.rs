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
use chrono::{DateTime, Utc};
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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
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
        self.post_with_revision(uri, key, body, None, 201).await
    }

    async fn post_with_revision(
        &self,
        uri: &str,
        key: &str,
        body: Value,
        revision: Option<i64>,
        expected_status: u16,
    ) -> Value {
        self.post_as(&self.token, uri, key, body, revision, expected_status)
            .await
    }

    async fn post_as(
        &self,
        token: &str,
        uri: &str,
        key: &str,
        body: Value,
        revision: Option<i64>,
        expected_status: u16,
    ) -> Value {
        let mut builder = Request::post(uri)
            .header(
                "host",
                crate::plugin_http_gateway::core_authority(&self.app.config).unwrap(),
            )
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("idempotency-key", key);
        if let Some(revision) = revision {
            builder = builder.header("if-match", format!("\"rev-{revision}\""));
        }
        let mut request = builder
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.as_u16(), expected_status, "{uri}: {value}");
        value
    }

    async fn start_task(&self, downstream: bool) -> Uuid {
        let mut steps = vec![self.direct_step(self.deployment, 10_000)];
        if downstream {
            steps.push(json!({
                "id":"next","contract":self.contract,"provider_deployment_id":self.deployment,
                "depends_on":["echo"],"input_mapping":{"message":{
                    "step_output":{"step":"echo","pointer":"/message"}
                }},"timeout_ms":10_000
            }));
        }
        self.submit_workflow("signed-dispatch", steps).await
    }

    fn direct_step(&self, deployment: Uuid, timeout_ms: u32) -> Value {
        json!({
            "id":"echo","contract":self.contract,"provider_deployment_id":deployment,
            "depends_on":[],"input_mapping":{"message":{"task_input":"/message"}},
            "timeout_ms":timeout_ms
        })
    }

    async fn submit_workflow(&self, name: &str, steps: Vec<Value>) -> Uuid {
        let workflow = self.publish_workflow(name, steps).await;
        self.submit_task(&self.token, name, workflow).await
    }

    async fn publish_workflow(&self, name: &str, steps: Vec<Value>) -> Uuid {
        let workflow = self.post("/api/platform/v1/workflows", &format!("{name}-workflow"), json!({
            "name":name,"input_schema":{
                "type":"object","required":["message"],"properties":{"message":{"type":"string"}}
            },"steps":steps
        })).await;
        id(&workflow, "id").unwrap()
    }

    async fn submit_task(&self, token: &str, name: &str, workflow: Uuid) -> Uuid {
        let task = self
            .post_as(
                token,
                "/api/platform/v1/tasks",
                &format!("{name}-task"),
                json!({
                    "workflow_id": workflow, "input":{"message":"from workflow"}
                }),
                None,
                201,
            )
            .await;
        id(&task, "id").unwrap()
    }

    async fn register_skill(&self, c: &Client, entry: &str, input_blobs: bool) -> Uuid {
        let mut manifest: Value = c
            .query_one(
                "SELECT manifest FROM installations WHERE id=$1",
                &[&self.installation],
            )
            .await
            .unwrap()
            .get(0);
        let cap = &manifest["capabilities"][0];
        let definition = json!({
            "name":format!("{}.runner-skill", manifest["plugin_id"].as_str().unwrap()),
            "version":"1.0.0","description":"Fixed signed capability Skill",
            "input_schema":cap["input_schema"],"output_schema":cap["output_schema"],
            "stream_item_schema":null,"target":{"kind":"capability","plugin_id":manifest["plugin_id"],
                "contract":cap["contract"]},"mapping":"identity","maximum_timeout_ms":20_000,
            "input_blobs":input_blobs,"tags":[]
        });
        manifest["business_entry"] = json!(entry);
        manifest["skills"] = json!([definition]);
        update_manifest(c, self.installation, &manifest).await;
        let skill = self
            .post(
                "/api/platform/v1/skills",
                "register-runner-skill",
                json!({
                    "installation_id":self.installation,"definition":definition,"enabled":true
                }),
            )
            .await;
        id(&skill, "id").unwrap()
    }

    async fn grant(&self, subject: Uuid, role: &str, scope: Value, key: &str) -> Uuid {
        let grant = self
            .post(
                &format!("/api/platform/v1/accounts/{subject}/grants"),
                key,
                json!({"source":"role","role_id":role,"scope":scope}),
            )
            .await;
        id(&grant, "id").unwrap()
    }

    async fn set_provider_concurrency(&self, c: &Client, limit: u32) {
        let mut manifest: Value = c
            .query_one(
                "SELECT manifest FROM installations WHERE id=$1",
                &[&self.installation],
            )
            .await
            .unwrap()
            .get(0);
        manifest["concurrency"]["max_active_requests"] = json!(limit);
        update_manifest(c, self.installation, &manifest).await;
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

async fn update_manifest(c: &Client, installation: Uuid, manifest: &Value) {
    crate::artifact::validate_profile(manifest).unwrap();
    let hash = digest(&serde_json::to_vec(manifest).unwrap());
    c.execute(
        "UPDATE installations SET manifest=$2,manifest_digest=$3 WHERE id=$1",
        &[&installation, &manifest, &hash],
    )
    .await
    .unwrap();
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

async fn never_claimed(c: &Client, task: Uuid) {
    let row = c
        .query_one(
            "SELECT t.state,s.state,s.attempt_count,
        NOT EXISTS(SELECT FROM attempts a WHERE a.step_id=s.id)
        FROM tasks t JOIN task_steps s ON s.task_id=t.id WHERE t.id=$1",
            &[&task],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), "queued");
    assert_eq!(row.get::<_, &str>(1), "ready");
    assert_eq!(row.get::<_, i32>(2), 0);
    assert!(row.get::<_, bool>(3));
}

async fn wait_task(c: &Client, task: Uuid, expected: &str) {
    tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let state = task_state(c, task).await.0;
            if state == expected {
                return;
            }
            assert!(
                !matches!(
                    state.as_str(),
                    "succeeded" | "failed" | "unknown" | "canceled"
                ),
                "task reached terminal state {state} while awaiting {expected}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("task {task} never became {expected}"));
}

async fn wait_invocations(provider: &Provider, count: usize) {
    loop {
        if provider.requests.lock().await.len() >= count {
            return;
        }
        provider.entered.notified().await;
    }
}

struct LinkedLease {
    attempt: AttemptLease,
    call: Uuid,
    rpc: Uuid,
    issued: DateTime<Utc>,
    until: DateTime<Utc>,
    hard: DateTime<Utc>,
}

async fn linked_lease(c: &Client, request: &rpc::InvokeRequest) -> LinkedLease {
    let call = Uuid::parse_str(&request.call_id).unwrap();
    let row = c
        .query_one(
            "SELECT a.id AS attempt_id,a.execution_id,a.worker_id,a.fence,
           a.created_at,a.lease_expires_at,c.deadline_at,r.id AS rpc_id,
           a.lease_expires_at=slot.expires_at AND slot.expires_at=e.expires_at
               AND e.expires_at=r.expires_at AS consistent
      FROM calls c JOIN task_attempt_calls b ON b.call_id=c.id
      JOIN attempts a ON a.id=b.attempt_id JOIN reservations slot ON slot.id=c.id
      JOIN execution_delegations e ON e.id=c.delegation_id
      JOIN rpc_delegations r ON r.call_id=c.id AND r.execution_delegation_id=e.id
      WHERE c.id=$1",
            &[&call],
        )
        .await
        .unwrap();
    assert!(
        row.get::<_, bool>("consistent"),
        "linked lease expiries must move together"
    );
    let until: DateTime<Utc> = row.get("lease_expires_at");
    let hard: DateTime<Utc> = row.get("deadline_at");
    assert!(
        until <= hard,
        "the rolling lease exceeded the hard Call deadline"
    );
    LinkedLease {
        attempt: AttemptLease {
            worker_id: row.get("worker_id"),
            attempt_id: row.get("attempt_id"),
            execution_id: row.get("execution_id"),
            fence: row.get("fence"),
        },
        call,
        rpc: row.get("rpc_id"),
        issued: row.get("created_at"),
        until,
        hard,
    }
}

// The mTLS provider is a live component fixture. Maintain its simulated
// controller observations through the 30-second test without changing health,
// identity, generation or readiness.
async fn observe_fixture_health(f: &Fixture) -> ServerGuard {
    let c = f.app.db.connect().await.unwrap();
    let instance = f.instance;
    ServerGuard(tokio::spawn(async move {
        loop {
            c.execute(
                "UPDATE instances SET last_heartbeat_at=clock_timestamp()
                         WHERE id=$1 AND phase='ready' AND generation=$2",
                &[&instance, &GENERATION],
            )
            .await
            .unwrap();
            c.execute(
                "UPDATE provider_records SET observed_at=clock_timestamp()
                         WHERE instance_id=$1 AND availability='ready' AND observed_generation=$2",
                &[&instance, &GENERATION],
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }))
}

async fn stop_runner(f: &Fixture, handle: JoinHandle<()>) {
    f.app.ready.store(false, Ordering::SeqCst);
    f.app.shutdown.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("task worker did not stop")
        .unwrap();
}

fn skill_step(skill: Uuid) -> Value {
    json!({
        "id":"echo","skill_id":skill,"depends_on":[],
        "input_mapping":{"message":{"task_input":"/message"}},"timeout_ms":20_000
    })
}

struct SkillTask {
    task: Uuid,
    skill: Uuid,
    submitter: auth::Context,
    common_issuer: Uuid,
    skill_grant: Uuid,
    capability_grant: Uuid,
}

async fn submit_signed_skill(f: &Fixture, c: &Client, entry: &str) -> SkillTask {
    let skill = f.register_skill(c, entry, false).await;
    let workflow = f
        .publish_workflow("signed-capability-skill", vec![skill_step(skill)])
        .await;
    let subject = Uuid::new_v4();
    // An unrelated account receives only the four grants needed to submit
    // this Task. None of its permissions is inherited from the Owner.
    c.execute(
        "INSERT INTO accounts(id,kind,status,display_name,password_hash,created_at,updated_at)
         SELECT $1,'user','active','Skill task submitter',password_hash,now(),now()
           FROM accounts WHERE id=$2",
        &[&subject, &f.owner],
    )
    .await
    .unwrap();
    c.execute("INSERT INTO account_closure VALUES($1,$1,0)", &[&subject])
        .await
        .unwrap();
    c.execute(
        "INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at)
         SELECT $1,spec,current_date,now(),now() FROM quotas WHERE account_id=$2",
        &[&subject, &f.owner],
    )
    .await
    .unwrap();
    f.grant(
        subject,
        "Member",
        json!({"kind":"task","match":"owned_by","owner_id":subject}),
        "skill-task-create",
    )
    .await;
    f.grant(
        subject,
        "Member",
        json!({"kind":"workflow","match":"ids","ids":[workflow]}),
        "skill-workflow-read",
    )
    .await;
    let skill_grant = f
        .grant(
            subject,
            "Owner",
            json!({"kind":"skill","match":"ids","ids":[skill]}),
            "skill-invoke-only",
        )
        .await;
    let capability_grant = f
        .grant(
            subject,
            "Guest",
            json!({"kind":"capability","match":"ids","ids":[capability_identifier(&f.contract)]}),
            "capability-invoke-only",
        )
        .await;
    let common_issuer = c
        .query_one(
            "SELECT s.issuer_credential_id,c.issuer_credential_id
           FROM grants s,grants c WHERE s.id=$1 AND c.id=$2",
            &[&skill_grant, &capability_grant],
        )
        .await
        .unwrap();
    let issuer: Uuid = common_issuer.get(0);
    assert_eq!(issuer, common_issuer.get::<_, Uuid>(1));
    let peer = "127.0.0.1".parse().unwrap();
    let token = f
        .app
        .auth
        .login(c, subject, PASSWORD.into(), peer, Uuid::new_v4())
        .await
        .unwrap();
    let bearer = token["access_token"].as_str().unwrap();
    let context = f
        .app
        .auth
        .authenticate(c, bearer, peer, Uuid::new_v4())
        .await
        .unwrap();
    let task = f
        .submit_task(bearer, "signed-capability-skill", workflow)
        .await;
    SkillTask {
        task,
        skill,
        submitter: context,
        common_issuer: issuer,
        skill_grant,
        capability_grant,
    }
}

fn capability_identifier(contract: &Value) -> String {
    format!(
        "{}@{}",
        contract["name"].as_str().unwrap(),
        contract["version"].as_str().unwrap()
    )
}

async fn revoke_skill_only(c: &mut Client, f: &Fixture, signed: &SkillTask) {
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, true).await.unwrap();
    tx.execute(
        "UPDATE grants SET status='revoked',revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1",
        &[&signed.skill_grant],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    auth::authorize(
        c,
        &signed.submitter,
        "capability.invoke",
        &auth::Resource::new("capability", capability_identifier(&f.contract), None),
    )
    .await
    .expect("underlying Capability permission must still be live");
    let denial = auth::authorize(
        c,
        &signed.submitter,
        "skill.invoke",
        &auth::Resource::new("skill", signed.skill, Some(f.owner)),
    )
    .await
    .unwrap_err();
    assert_eq!(denial.code, "PERMISSION_DENIED");
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
                a.lease_expires_at=r.expires_at AND r.expires_at=e.expires_at
                  AND e.expires_at=d.expires_at AND d.expires_at<=c.deadline_at AS uniform_lease,
                h.proxy_source_paths='[]' AND h.proxy_ceilings='[]' AND h.read_blob_ids='[]'
                  AS no_proxy_authority, e.maximum_rules AS call_maximum_rules
           FROM calls c JOIN reservations r ON r.id=c.id JOIN rpc_delegations d ON d.call_id=c.id
           JOIN execution_delegations e ON e.id=c.delegation_id
           JOIN host_rpc_contexts h ON h.call_id=c.id
           JOIN task_attempt_calls b ON b.call_id=c.id JOIN attempts a ON a.id=b.attempt_id
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
    assert!(row.get::<_, bool>("uniform_lease"));
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
async fn automatic_fixed_signed_skill_records_exact_dual_provenance_and_invokes_once() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let signed = submit_signed_skill(&f, &c, "skill_required").await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the automatic worker must reach the exact Skill on its mTLS provider");
    let wire = provider.requests.lock().await[0].clone();
    let linked = linked_lease(&c, &wire).await;
    assert_eq!(wire.input_json, br#"{"message":"from workflow"}"#);
    let ctx = wire.context.as_ref().unwrap();
    assert_eq!(ctx.subject_id, signed.submitter.subject.to_string());
    assert!(!ctx.delegation_token.is_empty());

    let row = c.query_one(
        "SELECT c.skill_id,c.skill_definition_digest,k.definition_digest,c.contract,c.provider_instance_id,
                l.installation_id,t.admission->'steps'->0->'binding' AS pinned,
                e.maximum_rules,h.source_paths,
                h.proxy_source_paths='[]' AND h.proxy_ceilings='[]' AND h.read_blob_ids='[]'
                  AS no_proxy,
                (SELECT array_agg(credential_id ORDER BY credential_id)
                   FROM call_credential_charges WHERE call_id=c.id) AS charges,
                a.number,a.idempotency_key=c.idempotency_key AS stable_key
           FROM calls c JOIN skills k ON k.id=c.skill_id
           JOIN task_attempt_calls l ON l.call_id=c.id JOIN attempts a ON a.id=l.attempt_id
           JOIN task_steps s ON s.id=a.step_id JOIN tasks t ON t.id=s.task_id
           JOIN execution_delegations e ON e.id=c.delegation_id
           JOIN host_rpc_contexts h ON h.call_id=c.id
          WHERE c.id=$1 AND t.id=$2",
        &[&linked.call,&signed.task],
    ).await.unwrap();
    assert_eq!(row.get::<_, Uuid>("skill_id"), signed.skill);
    let definition_digest: String = row.get("skill_definition_digest");
    assert_eq!(definition_digest, row.get::<_, String>("definition_digest"));
    assert_eq!(row.get::<_, Value>("contract"), f.contract);
    assert_eq!(row.get::<_, Uuid>("provider_instance_id"), f.instance);
    assert_eq!(row.get::<_, Uuid>("installation_id"), f.installation);
    let pinned: Value = row.get("pinned");
    assert_eq!(pinned["kind"], "skill_capability");
    assert_eq!(pinned["skill_id"], json!(signed.skill));
    assert_eq!(pinned["definition_digest"], definition_digest);
    assert_eq!(pinned["contract"], f.contract);
    assert_eq!(row.get::<_, i32>("number"), 1);
    assert!(row.get::<_, bool>("stable_key"));
    assert!(row.get::<_, bool>("no_proxy"));
    let mut credentials = vec![signed.submitter.credential, signed.common_issuer];
    credentials.sort_unstable();
    assert_eq!(row.get::<_, Vec<Uuid>>("charges"), credentials);

    let sources: Value = row.get("source_paths");
    let maximum: Value = row.get("maximum_rules");
    let expected = [
        (
            "skill.invoke",
            json!({"kind":"skill","match":"ids","ids":[signed.skill]}),
            signed.skill_grant,
        ),
        (
            "capability.invoke",
            json!({"kind":"capability","match":"ids",
                                     "ids":[capability_identifier(&f.contract)]}),
            signed.capability_grant,
        ),
    ];
    assert_eq!(sources.as_array().unwrap().len(), expected.len());
    assert_eq!(maximum.as_array().unwrap().len(), expected.len());
    for (action, resource, grant) in expected {
        let path = sources
            .as_array()
            .unwrap()
            .iter()
            .find(|path| path["rule"]["action"] == action)
            .expect("the action's original provenance must be stored");
        assert_eq!(path["rule"]["resource"], resource);
        assert!(path["sources"].as_array().unwrap().contains(&json!(grant)));
        assert!(maximum.as_array().unwrap().contains(&path["rule"]));
    }

    gate.add_permits(1);
    wait_task(&c, signed.task, "succeeded").await;
    assert_eq!(
        task_state(&c, signed.task).await.1,
        json!({"message":"echo from workflow"})
    );
    let call = c
        .query_one(
            "SELECT result, state FROM calls WHERE id=$1",
            &[&linked.call],
        )
        .await
        .unwrap();
    assert_eq!(call.get::<_, &str>("state"), "succeeded");
    let call: Value = call.get("result");
    assert_eq!(call["skill_id"], json!(signed.skill));
    assert_eq!(call["skill_definition_digest"], definition_digest);
    let replay = task_engine::dispatch(&f.app, &linked.attempt)
        .await
        .unwrap();
    assert_eq!(replay["state"], "succeeded");
    revoke_skill_only(&mut c, &f, &signed).await;
    task_engine::dispatch(&f.app, &linked.attempt)
        .await
        .expect_err("old result replay still requires the original Skill right");
    assert_eq!(provider.requests.lock().await.len(), 1);
    assert_eq!(
        c.query_one("SELECT count(*) FROM calls", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    stop_runner(&f, worker).await;
}

#[tokio::test]
async fn withdrawing_skill_only_during_automatic_dispatch_bars_settlement_and_replay() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut c = f.app.db.connect().await.unwrap();
    let signed = submit_signed_skill(&f, &c, "direct").await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the exact Skill must already be in-flight before removing its grant");
    let wire = provider.requests.lock().await[0].clone();
    let linked = linked_lease(&c, &wire).await;
    revoke_skill_only(&mut c, &f, &signed).await;
    gate.add_permits(1);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state: String = c
                .query_one("SELECT state FROM calls WHERE id=$1", &[&linked.call])
                .await
                .unwrap()
                .get(0);
            if state == "unknown" {
                break;
            }
            assert_eq!(
                state, "running",
                "late result must never become a terminal receipt"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("late output must be fenced without waiting for the original deadline");
    let row = c
        .query_one(
            "SELECT c.result IS NULL AS no_result,res.status AS reservation,r.status AS rpc,
                NOT EXISTS (SELECT FROM attempt_commit_receipts WHERE attempt_id=$2) AS no_receipt
           FROM calls c JOIN reservations res ON res.id=c.id
          JOIN rpc_delegations r ON r.call_id=c.id WHERE c.id=$1",
            &[&linked.call, &linked.attempt.attempt_id],
        )
        .await
        .unwrap();
    assert!(row.get::<_, bool>("no_result"));
    assert!(row.get::<_, bool>("no_receipt"));
    assert_eq!(row.get::<_, &str>("reservation"), "active");
    assert_eq!(row.get::<_, &str>("rpc"), "revoked");
    let (state, output) = task_state(&c, signed.task).await;
    assert_ne!(state, "succeeded");
    assert_eq!(output, Value::Null);
    task_engine::dispatch(&f.app, &linked.attempt)
        .await
        .expect_err("retained Capability permission cannot reenter a withdrawn Skill Call");
    assert_eq!(provider.requests.lock().await.len(), 1);
    stop_runner(&f, worker).await;
}

#[tokio::test]
async fn automatic_worker_waits_for_readiness_and_skips_older_unsupported_targets() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let skill = f.register_skill(&c, "direct", true).await;
    let old_skill = f
        .submit_workflow("runner-older-blob-skill", vec![skill_step(skill)])
        .await;
    let old_long = f
        .submit_workflow(
            "runner-older-long",
            vec![f.direct_step(f.deployment, 300_001)],
        )
        .await;
    let supported = f
        .submit_workflow(
            "runner-supported",
            vec![f.direct_step(f.deployment, 20_000)],
        )
        .await;
    let provider = Provider::default();
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    f.app.ready.store(false, Ordering::SeqCst);
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    assert!(
        tokio::time::timeout(Duration::from_millis(600), wait_invocations(&provider, 1))
            .await
            .is_err(),
        "worker sent an invocation before readiness"
    );
    never_claimed(&c, supported).await;

    f.app.ready.store(true, Ordering::SeqCst);
    wait_task(&c, supported, "succeeded").await;
    assert_eq!(
        task_state(&c, supported).await.1,
        json!({"message":"echo from workflow"})
    );
    never_claimed(&c, old_skill).await;
    never_claimed(&c, old_long).await;
    assert_eq!(provider.requests.lock().await.len(), 1);
    stop_runner(&f, worker).await;
}

#[tokio::test]
async fn automatic_worker_bounds_parallel_claims_and_leaves_ready_work_on_shutdown() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    for name in ["runner-first", "runner-second"] {
        f.submit_workflow(name, vec![f.direct_step(f.deployment, 20_000)])
            .await;
    }
    let third = f
        .submit_workflow("runner-third", vec![f.direct_step(f.deployment, 20_000)])
        .await;
    let fourth = f
        .submit_workflow("runner-fourth", vec![f.direct_step(f.deployment, 20_000)])
        .await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    tokio::time::timeout(Duration::from_secs(6), wait_invocations(&provider, 2))
        .await
        .expect("the two standard worker slots must run independently");
    assert!(
        tokio::time::timeout(Duration::from_millis(600), wait_invocations(&provider, 3))
            .await
            .is_err(),
        "worker exceeded its two in-flight slots"
    );
    never_claimed(&c, third).await;
    never_claimed(&c, fourth).await;

    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(6), wait_invocations(&provider, 3))
        .await
        .expect("a completed invocation did not release the next worker slot");
    stop_runner(&f, worker).await;
    gate.add_permits(3);
    never_claimed(&c, fourth).await;
    assert_eq!(provider.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn automatic_claim_keeps_busy_and_quota_blocked_work_unclaimed() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    f.set_provider_concurrency(&c, 1).await;
    let held_task = f
        .submit_workflow("runner-held", vec![f.direct_step(f.deployment, 25_000)])
        .await;
    let held_lease = claim(&f).await;
    let held_gate = Arc::new(Semaphore::new(0));
    let held_provider = Provider {
        gate: Some(held_gate.clone()),
        ..Provider::default()
    };
    let _held_server = f.serve(f.instance, GENERATION, held_provider.clone()).await;
    let app = f.app.clone();
    let held = tokio::spawn(async move { task_engine::dispatch(&app, &held_lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&held_provider, 1))
        .await
        .expect("the installed provider must hold its only business slot");
    let queued_busy = f
        .submit_workflow(
            "runner-provider-busy",
            vec![f.direct_step(f.deployment, 25_000)],
        )
        .await;
    let worker = Uuid::new_v4();
    assert!(crate::broker::dispatch_next_task(&f.app, worker)
        .await
        .unwrap()
        .is_none());
    never_claimed(&c, queued_busy).await;

    let (free_deployment, free_instance) =
        install_provider(&c, f.owner, f.installation, &f.contract).await;
    let free_provider = Provider::default();
    let _free_server = f
        .serve(free_instance, GENERATION, free_provider.clone())
        .await;
    let queued_quota = f
        .submit_workflow(
            "runner-quota-blocked",
            vec![f.direct_step(free_deployment, 25_000)],
        )
        .await;
    let set_quota =
        "UPDATE quotas SET spec=jsonb_set(spec,'{maximum_concurrent_calls}',to_jsonb($2::integer))
                         WHERE account_id=$1";
    c.execute(set_quota, &[&f.owner, &1_i32]).await.unwrap();
    assert!(crate::broker::dispatch_next_task(&f.app, worker)
        .await
        .unwrap()
        .is_none());
    never_claimed(&c, queued_busy).await;
    never_claimed(&c, queued_quota).await;
    assert!(free_provider.requests.lock().await.is_empty());
    let row = c
        .query_one(
            "SELECT concurrent_calls_used,(SELECT count(*) FROM calls),
                                  (SELECT count(*) FROM task_attempt_calls)
                             FROM quotas WHERE account_id=$1",
            &[&f.owner],
        )
        .await
        .unwrap();
    assert_eq!(
        (
            row.get::<_, i64>(0),
            row.get::<_, i64>(1),
            row.get::<_, i64>(2)
        ),
        (1, 1, 1)
    );

    c.execute(set_quota, &[&f.owner, &16_i32]).await.unwrap();
    let step = crate::broker::dispatch_next_task(&f.app, worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(step["task_id"], json!(queued_quota));
    assert_eq!(task_state(&c, queued_quota).await.0, "succeeded");
    assert_eq!(free_provider.requests.lock().await.len(), 1);
    never_claimed(&c, queued_busy).await;
    held_gate.add_permits(2);
    held.await.unwrap().unwrap();
    assert_eq!(task_state(&c, held_task).await.0, "succeeded");
    let unblocked = crate::broker::dispatch_next_task(&f.app, worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unblocked["task_id"], json!(queued_busy));
    assert_eq!(held_provider.requests.lock().await.len(), 2);
}

#[tokio::test]
async fn automatic_worker_reaches_later_ready_work_after_a_full_blocked_batch() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    f.set_provider_concurrency(&c, 1).await;
    f.submit_workflow(
        "runner-saturated",
        vec![f.direct_step(f.deployment, 30_000)],
    )
    .await;
    let held_lease = claim(&f).await;
    let held_gate = Arc::new(Semaphore::new(0));
    let held_provider = Provider {
        gate: Some(held_gate),
        ..Provider::default()
    };
    let _held_server = f.serve(f.instance, GENERATION, held_provider.clone()).await;
    let app = f.app.clone();
    let held = tokio::spawn(async move { task_engine::dispatch(&app, &held_lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&held_provider, 1))
        .await
        .expect("the original invocation must occupy its only provider slot");

    let mut blocked = Vec::new();
    for index in 0..17 {
        blocked.push(
            f.submit_workflow(
                &format!("runner-blocked-{index}"),
                vec![f.direct_step(f.deployment, 20_000)],
            )
            .await,
        );
    }
    let (free_deployment, free_instance) =
        install_provider(&c, f.owner, f.installation, &f.contract).await;
    let free_provider = Provider::default();
    let _free_server = f
        .serve(free_instance, GENERATION, free_provider.clone())
        .await;
    let younger = f
        .submit_workflow(
            "runner-after-blocked-batch",
            vec![f.direct_step(free_deployment, 20_000)],
        )
        .await;

    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    wait_task(&c, younger, "succeeded").await;
    assert_eq!(free_provider.requests.lock().await.len(), 1);
    assert_eq!(held_provider.requests.lock().await.len(), 1);
    for task in blocked {
        never_claimed(&c, task).await;
    }
    stop_runner(&f, worker).await;
    tokio::time::timeout(Duration::from_secs(3), held)
        .await
        .expect("the held fixture must observe shutdown")
        .unwrap()
        .expect_err("the stopped kernel cannot confirm the held invocation");
}

#[tokio::test]
async fn automatic_worker_does_not_reissue_an_unconfirmed_call_after_restart() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let first_task = f
        .submit_workflow(
            "runner-lost-result",
            vec![f.direct_step(f.deployment, 20_000)],
        )
        .await;
    let c = f.app.db.connect().await.unwrap();
    let provider = Provider {
        reply: ProviderReply::Unavailable,
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    wait_task(&c, first_task, "unknown").await;
    let first = provider.requests.lock().await[0].clone();
    check_settlement(&c, first_task, &first, f.instance, "unknown").await;
    stop_runner(&f, worker).await;

    f.app.shutdown.store(false, Ordering::SeqCst);
    f.app.ready.store(true, Ordering::SeqCst);
    let new_task = f
        .submit_workflow(
            "runner-after-restart",
            vec![f.direct_step(f.deployment, 20_000)],
        )
        .await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    wait_task(&c, new_task, "unknown").await;
    stop_runner(&f, worker).await;
    assert_eq!(
        task_state(&c, first_task).await,
        ("unknown".into(), Value::Null)
    );
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[1].call_id, first.call_id);
    let tried = c
        .query_one(
            "SELECT count(*) FROM attempts a JOIN task_steps s ON s.id=a.step_id
                             WHERE s.task_id=$1 AND a.state='unknown'",
            &[&first_task],
        )
        .await
        .unwrap();
    assert_eq!(tried.get::<_, i64>(0), 1);
}

#[tokio::test]
async fn long_unary_worker_renews_one_linked_call_beyond_its_initial_lease() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f
        .submit_workflow(
            "runner-long-renewal",
            vec![f.direct_step(f.deployment, 55_000)],
        )
        .await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let _health = observe_fixture_health(&f).await;
    let worker = tokio::spawn(task_engine::run(f.app.clone()));
    tokio::time::timeout(Duration::from_secs(6), wait_invocations(&provider, 1))
        .await
        .expect("the automatic worker must enter the long mTLS invocation");
    let request = provider.requests.lock().await[0].clone();
    let c = f.app.db.connect().await.unwrap();
    let first = linked_lease(&c, &request).await;
    let context = request.context.as_ref().unwrap();
    assert!(first.until <= first.issued + chrono::Duration::seconds(30));
    assert!(first.issued + chrono::Duration::seconds(30) < first.hard);
    assert_eq!(context.deadline_unix_ms, first.hard.timestamp_millis());

    // Use the database clock and an actual provider wait. Releasing the result
    // before this point would let the test pass without any live renewal.
    tokio::time::timeout(Duration::from_secs(38), async {
        let mut previous_until = first.until;
        loop {
            let now: DateTime<Utc> = c
                .query_one("SELECT clock_timestamp()", &[])
                .await
                .unwrap()
                .get(0);
            let advanced = linked_lease(&c, &request).await;
            assert!(advanced.until > now && advanced.until >= previous_until);
            assert_eq!(advanced.attempt, first.attempt);
            assert_eq!(advanced.rpc, first.rpc);
            assert_eq!(advanced.hard, first.hard);
            assert_eq!(advanced.call, first.call);
            if now >= first.issued + chrono::Duration::milliseconds(30_100) {
                assert!(advanced.until > first.until);
                break;
            }
            previous_until = advanced.until;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .expect("the one-call provider must remain live past its initial lease");
    assert_eq!(provider.requests.lock().await.len(), 1);
    gate.add_permits(1);
    wait_task(&c, task, "succeeded").await;
    check_settlement(&c, task, &request, f.instance, "succeeded").await;
    assert_eq!(
        task_state(&c, task).await.1,
        json!({"message":"echo from workflow"})
    );
    assert_eq!(provider.requests.lock().await.len(), 1);
    stop_runner(&f, worker).await;
}

#[tokio::test]
async fn long_unary_result_loses_authority_after_frozen_task_delegation_revocation() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f
        .submit_workflow(
            "runner-long-revoked",
            vec![f.direct_step(f.deployment, 60_000)],
        )
        .await;
    let lease = claim(&f).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the capability must already be running before revocation");
    let wire = provider.requests.lock().await[0].clone();
    let mut c = f.app.db.connect().await.unwrap();
    let admitted = linked_lease(&c, &wire).await;
    assert!(admitted.until < admitted.hard);
    let tx = c.transaction().await.unwrap();
    crate::db::auth_lock(&tx, true).await.unwrap();
    tx.execute(
        "UPDATE execution_delegations SET status='revoked'
                  WHERE id=(SELECT delegation_id FROM tasks WHERE id=$1)",
        &[&task],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    gate.add_permits(1);
    let failure = tokio::time::timeout(Duration::from_secs(5), in_flight)
        .await
        .expect("late output must be fenced promptly")
        .unwrap()
        .unwrap_err();
    assert_eq!(failure.resource_id.as_deref(), Some(wire.call_id.as_str()));
    let row = c
        .query_one(
            "SELECT c.state,c.result IS NULL,slot.status,r.status,
                                 c.deadline_at= $2 AND clock_timestamp()<c.deadline_at
      FROM calls c JOIN reservations slot ON slot.id=c.id JOIN rpc_delegations r ON r.call_id=c.id
      WHERE c.id=$1",
            &[&admitted.call, &admitted.hard],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), "unknown");
    assert!(row.get::<_, bool>(1));
    assert_eq!(row.get::<_, &str>(2), "active");
    assert_eq!(row.get::<_, &str>(3), "revoked");
    assert!(row.get::<_, bool>(4));
    let (state, output) = task_state(&c, task).await;
    assert_ne!(state, "succeeded");
    assert_eq!(output, Value::Null);
    task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("revocation bars any replay");
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn canceling_long_unary_task_stops_renewal_but_retains_a_live_terminal() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let mut child = f.direct_step(f.deployment, 20_000);
    child["id"] = json!("child");
    child["depends_on"] = json!(["echo"]);
    let task = f
        .submit_workflow(
            "runner-long-canceled",
            vec![f.direct_step(f.deployment, 60_000), child],
        )
        .await;
    let lease = claim(&f).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the long mTLS invocation must be running before HTTP cancellation");
    let wire = provider.requests.lock().await[0].clone();
    let c = f.app.db.connect().await.unwrap();
    let previous = linked_lease(&c, &wire).await;
    assert!(previous.until < previous.hard);
    let revision: i64 = c
        .query_one("SELECT revision FROM tasks WHERE id=$1", &[&task])
        .await
        .unwrap()
        .get(0);
    let canceled = f
        .post_with_revision(
            &format!("/api/platform/v1/tasks/{task}/cancel"),
            "cancel-held-long-task",
            json!({}),
            Some(revision),
            200,
        )
        .await;
    assert_eq!(canceled["state"], "cancel_requested");
    super::admission::renew_task_call(&f.app, &lease, previous.call)
        .await
        .expect_err("cancel intent forbids extending any linked fence");
    tokio::time::sleep(Duration::from_millis(250)).await;
    let current = linked_lease(&c, &wire).await;
    assert_eq!(current.attempt, previous.attempt);
    assert_eq!(current.rpc, previous.rpc);
    assert_eq!(current.until, previous.until);
    assert_eq!(current.hard, previous.hard);
    let now: DateTime<Utc> = c
        .query_one("SELECT clock_timestamp()", &[])
        .await
        .unwrap()
        .get(0);
    assert!(
        now < previous.until,
        "the provider's terminal must fall inside the original lease"
    );
    gate.add_permits(1);
    let finished = tokio::time::timeout(Duration::from_secs(5), in_flight)
        .await
        .expect("the live signed terminal must settle")
        .unwrap()
        .unwrap();
    assert_eq!(finished["state"], "succeeded");
    assert_eq!(task_state(&c, task).await, ("canceled".into(), Value::Null));
    check_settlement(&c, task, &wire, f.instance, "succeeded").await;
    let child = c
        .query_one(
            "SELECT state,attempt_count FROM task_steps
        WHERE task_id=$1 AND step_key='child'",
            &[&task],
        )
        .await
        .unwrap();
    assert_eq!(child.get::<_, &str>(0), "canceled");
    assert_eq!(child.get::<_, i32>(1), 0);
    assert!(task_engine::claim(&f.app.db, &f.app.auth, Uuid::new_v4())
        .await
        .unwrap()
        .is_none());
    assert!(crate::broker::dispatch_next_task(&f.app, Uuid::new_v4())
        .await
        .unwrap()
        .is_none());
    assert_eq!(provider.requests.lock().await.len(), 1);
}

async fn set_fixture_rolling_lease(c: &mut Client, lease: &LinkedLease, seconds: i32) {
    let tx = c.transaction().await.unwrap();
    crate::db::call_lock(&tx).await.unwrap();
    let until: DateTime<Utc> = tx
        .query_one(
            "SELECT clock_timestamp()+$1::integer * interval '1 second'",
            &[&seconds],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        tx.execute(
            "UPDATE attempts SET lease_expires_at=$2 WHERE id=$1",
            &[&lease.attempt.attempt_id, &until]
        )
        .await
        .unwrap(),
        1
    );
    for sql in [
        "UPDATE reservations SET expires_at=$2 WHERE id=$1",
        "UPDATE execution_delegations SET expires_at=$2 WHERE id=(SELECT delegation_id FROM calls WHERE id=$1)",
        "UPDATE rpc_delegations SET expires_at=$2 WHERE call_id=$1",
    ] {
        assert_eq!(tx.execute(sql, &[&lease.call, &until]).await.unwrap(), 1);
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn long_unary_response_after_the_short_lease_is_never_published() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f
        .submit_workflow(
            "runner-long-late",
            vec![f.direct_step(f.deployment, 60_000)],
        )
        .await;
    let lease = claim(&f).await;
    let gate = Arc::new(Semaphore::new(0));
    let provider = Provider {
        gate: Some(gate.clone()),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the capability must already be running before the short lease is lost");
    let wire = provider.requests.lock().await[0].clone();
    let mut c = f.app.db.connect().await.unwrap();
    let admitted = linked_lease(&c, &wire).await;
    assert!(admitted.until < admitted.hard);
    set_fixture_rolling_lease(&mut c, &admitted, -3).await;
    gate.add_permits(1);
    let failure = tokio::time::timeout(Duration::from_secs(5), in_flight)
        .await
        .expect("lost short fence must stop confirmation promptly")
        .unwrap()
        .unwrap_err();
    assert_eq!(failure.resource_id.as_deref(), Some(wire.call_id.as_str()));
    check_settlement(&c, task, &wire, f.instance, "unknown").await;
    task_engine::expire_leases(&f.app.db).await.unwrap();
    assert_eq!(task_state(&c, task).await, ("unknown".into(), Value::Null));
    assert!(linked_lease(&c, &wire).await.hard > Utc::now());
    assert_eq!(provider.requests.lock().await.len(), 1);
}

#[tokio::test]
async fn expired_rolling_lease_targets_recovery_before_the_hard_call_deadline() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let task = f
        .submit_workflow(
            "runner-long-abandoned",
            vec![f.direct_step(f.deployment, 90_000)],
        )
        .await;
    let lease = claim(&f).await;
    let provider = Provider {
        gate: Some(Arc::new(Semaphore::new(0))),
        ..Provider::default()
    };
    let _server = f.serve(f.instance, GENERATION, provider.clone()).await;
    let app = f.app.clone();
    let in_flight = tokio::spawn(async move { task_engine::dispatch(&app, &lease).await });
    tokio::time::timeout(Duration::from_secs(5), wait_invocations(&provider, 1))
        .await
        .expect("the capability must receive its only admitted invocation");
    let wire = provider.requests.lock().await[0].clone();
    // Leave the actual admitted Call in flight in the ledger, as on a lost
    // worker. A preexisting UNKNOWN would mask the expiry-selection behavior.
    in_flight.abort();
    assert!(in_flight.await.unwrap_err().is_cancelled());
    let mut c = f.app.db.connect().await.unwrap();
    let admitted = linked_lease(&c, &wire).await;
    assert_eq!(admitted.attempt, lease);
    assert!(admitted.until < admitted.hard);

    set_fixture_rolling_lease(&mut c, &admitted, 10).await;
    let shortened = linked_lease(&c, &wire).await;
    assert!(shortened.until < admitted.until);
    task_engine::renew(&f.app.db, &f.app.auth, &lease)
        .await
        .expect_err("the generic Attempt renewer must not extend only one row");
    let wrong = AttemptLease {
        fence: lease.fence + 1,
        ..lease
    };
    super::admission::renew_task_call(&f.app, &wrong, admitted.call)
        .await
        .expect_err("renewal with the wrong fence must fail");
    assert_eq!(linked_lease(&c, &wire).await.until, shortened.until);
    super::admission::renew_task_call(&f.app, &lease, admitted.call)
        .await
        .unwrap();
    let renewed = linked_lease(&c, &wire).await;
    assert_eq!(renewed.hard, admitted.hard);
    assert_eq!(renewed.attempt, lease);
    assert!(renewed.until > shortened.until);
    assert!(crate::controller::expired_call_providers(&c)
        .await
        .unwrap()
        .is_empty());

    set_fixture_rolling_lease(&mut c, &admitted, -3).await;
    let row = c
        .query_one(
            "SELECT state,deadline_at>clock_timestamp() FROM calls WHERE id=$1",
            &[&admitted.call],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, &str>(0), "running");
    assert!(row.get::<_, bool>(1));
    super::admission::renew_task_call(&f.app, &lease, admitted.call)
        .await
        .expect_err("a missed renewal cannot revive the expired physical lease");
    assert_eq!(
        crate::controller::expired_call_providers(&c).await.unwrap(),
        vec![f.instance]
    );
    task_engine::expire_leases(&f.app.db).await.unwrap();
    assert_eq!(task_state(&c, task).await, ("unknown".into(), Value::Null));
    let projection = c
        .query_one(
            "SELECT a.state,slot.status FROM attempts a
        JOIN task_attempt_calls b ON b.attempt_id=a.id JOIN reservations slot ON slot.id=b.call_id
        WHERE a.id=$1",
            &[&lease.attempt_id],
        )
        .await
        .unwrap();
    assert_eq!(projection.get::<_, &str>(0), "expired");
    assert_eq!(
        projection.get::<_, &str>(1),
        "active",
        "physical recovery must still fence the provider"
    );
    task_engine::dispatch(&f.app, &lease)
        .await
        .expect_err("expired worker cannot send again");
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
