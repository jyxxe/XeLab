//! Real loopback mTLS/protobuf transport with a deliberately faulty test peer.
//! These are protocol checks, not OCI, volume isolation or upgrade acceptance.
#![allow(clippy::result_large_err)]
use super::*;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, SanType};
use rpc::{
    data_migration_server::{DataMigration, DataMigrationServer},
    plugin_lifecycle_server::{PluginLifecycle, PluginLifecycleServer},
};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
};
use tonic::{Response, Status};

#[derive(Clone)]
struct Service {
    binding: Binding,
    plan: Plan,
    manifest: Vec<u8>,
    mode: Arc<AtomicU8>,
    applies: Arc<AtomicUsize>,
    format: PathBuf,
}
impl Service {
    fn context(&self, context: Option<rpc::RpcContext>, phase: &str, cap: i64) {
        let context = context.unwrap();
        assert!(!Uuid::parse_str(&context.request_id).unwrap().is_nil());
        assert_eq!(context.subject_id, self.binding.subject.to_string());
        assert!(context.actor_instance_id.is_empty());
        assert!(context.delegation_token.is_empty());
        assert_eq!(
            context.idempotency_key,
            format!("{}:{phase}", self.binding.operation)
        );
        let now = chrono::Utc::now().timestamp_millis();
        assert!(context.deadline_unix_ms > now);
        assert!(context.deadline_unix_ms <= now + cap);
        assert!(context.deadline_unix_ms <= self.binding.expires_unix_ms);
    }
    fn result(&self) -> rpc::MigrationResult {
        let mode = self.mode.load(Ordering::SeqCst);
        let actual = fs::read_to_string(&self.format).unwrap();
        rpc::MigrationResult {
            plan_id: if mode == 5 {
                Uuid::new_v4().to_string()
            } else {
                self.plan.id.to_string()
            },
            succeeded: actual == "2",
            observed_schema: if mode == 7 { "wrong".into() } else { actual },
            error: if mode == 6 {
                Some(rpc::ErrorDetail {
                    code: "DATA_CORRUPT".into(),
                    ..Default::default()
                })
            } else {
                None
            },
        }
    }
}
#[tonic::async_trait]
impl DataMigration for Service {
    async fn plan(
        &self,
        r: Request<rpc::PlanMigrationRequest>,
    ) -> std::result::Result<Response<rpc::MigrationPlan>, Status> {
        let r = r.into_inner();
        self.context(r.context, "plan", 10000);
        assert_eq!(r.deployment_id, self.binding.deployment.to_string());
        assert_eq!(r.source_schema, "1");
        assert_eq!(r.target_schema, "2");
        let mode = self.mode.load(Ordering::SeqCst);
        Ok(Response::new(rpc::MigrationPlan {
            plan_id: if mode == 4 {
                Uuid::nil().to_string()
            } else {
                self.plan.id.to_string()
            },
            plan_digest: if mode == 2 {
                "sha256:untrusted".into()
            } else {
                self.plan.digest.clone()
            },
            requires_exclusive_write: true,
            backward_read_compatible: false,
            reversible: false,
            temporary_bytes_required: if mode == 3 { u64::MAX } else { 4096 },
        }))
    }
    async fn apply(
        &self,
        r: Request<rpc::ApplyMigrationRequest>,
    ) -> std::result::Result<Response<rpc::MigrationResult>, Status> {
        let r = r.into_inner();
        self.context(r.context, "apply", 3600000);
        assert_eq!(r.plan_id, self.plan.id.to_string());
        assert_eq!(r.plan_digest, self.plan.digest);
        assert!(!Uuid::parse_str(&r.backup_id).unwrap().is_nil());
        assert_eq!(r.volume_fence, 9);
        self.applies.fetch_add(1, Ordering::SeqCst);
        fs::write(&self.format, "2").unwrap();
        if self.mode.load(Ordering::SeqCst) == 8 {
            return Err(Status::unavailable("response lost after write"));
        }
        Ok(Response::new(self.result()))
    }
    async fn verify(
        &self,
        r: Request<rpc::VerifyMigrationRequest>,
    ) -> std::result::Result<Response<rpc::MigrationResult>, Status> {
        let r = r.into_inner();
        self.context(r.context, "verify", 60000);
        assert_eq!(r.plan_id, self.plan.id.to_string());
        assert_eq!(r.volume_fence, 9);
        Ok(Response::new(self.result()))
    }
}
#[tonic::async_trait]
impl PluginLifecycle for Service {
    async fn describe(
        &self,
        r: Request<rpc::DescribeRequest>,
    ) -> std::result::Result<Response<rpc::DescribeResponse>, Status> {
        self.context(r.into_inner().context, "describe", 10000);
        let mut bytes = self.manifest.clone();
        if self.mode.load(Ordering::SeqCst) == 1 {
            bytes.push(b' ');
        }
        Ok(Response::new(rpc::DescribeResponse {
            manifest_json: bytes,
            manifest_digest: digest(&self.manifest),
        }))
    }
    async fn initialize(
        &self,
        _: Request<rpc::InitializeRequest>,
    ) -> std::result::Result<Response<rpc::InitializeResponse>, Status> {
        Err(Status::unimplemented("protocol fixture"))
    }
    async fn get_health(
        &self,
        _: Request<rpc::HealthRequest>,
    ) -> std::result::Result<Response<rpc::HealthResponse>, Status> {
        Err(Status::unimplemented("protocol fixture"))
    }
    async fn drain(
        &self,
        _: Request<rpc::DrainRequest>,
    ) -> std::result::Result<Response<rpc::DrainResponse>, Status> {
        Err(Status::unimplemented("protocol fixture"))
    }
    async fn shutdown(
        &self,
        _: Request<rpc::ShutdownRequest>,
    ) -> std::result::Result<Response<rpc::Ack>, Status> {
        Err(Status::unimplemented("protocol fixture"))
    }
}
fn leaf(
    ca: &rcgen::Certificate,
    ca_key: &KeyPair,
    name: &str,
    uri: &str,
) -> tonic::transport::Identity {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec![name.into()]).unwrap();
    params
        .subject_alt_names
        .push(SanType::URI(uri.try_into().unwrap()));
    let cert = params.signed_by(&key, ca, ca_key).unwrap();
    tonic::transport::Identity::from_pem(cert.pem(), key.serialize_pem())
}
#[tokio::test]
async fn actual_mtls_protocol_rejects_bad_plans_and_verifies_unknown_apply_without_retry() {
    crate::init_crypto();
    let root = tempfile::tempdir().unwrap();
    let format = root.path().join("schema");
    fs::write(&format, "1").unwrap();
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["migration-test-ca".into()]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = params.self_signed(&key).unwrap();
    let binding = Binding {
        operation: Uuid::new_v4(),
        deployment: Uuid::new_v4(),
        subject: Uuid::new_v4(),
        instance: Uuid::new_v4(),
        generation: 7,
        expires_unix_ms: chrono::Utc::now().timestamp_millis() + 120000,
        source_schema: "1".into(),
        target_schema: "2".into(),
    };
    binding.validate().unwrap();
    let plan = Plan {
        id: Uuid::new_v4(),
        digest: digest(b"persisted fixture plan"),
        requires_exclusive_write: true,
        backward_read_compatible: false,
        reversible: false,
        temporary_bytes_required: 4096,
    };
    let mut manifest: Value =
        serde_json::from_str(include_str!("../tests/fixtures/secret-manifest.json")).unwrap();
    manifest["data_schema_version"] = serde_json::json!("2");
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let signed_digest = digest(&bytes);
    let service = Service {
        binding: binding.clone(),
        plan: plan.clone(),
        manifest: bytes,
        mode: Arc::new(AtomicU8::new(0)),
        applies: Arc::new(AtomicUsize::new(0)),
        format: format.clone(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hostname = format!("i-{}-g-7.xelab.local", binding.instance);
    let server_identity = leaf(
        &ca,
        &key,
        &hostname,
        &format!(
            "spiffe://xelab.local/instance/{}/generation/7",
            binding.instance
        ),
    );
    let server = tonic::transport::Server::builder()
        .tls_config(
            tonic::transport::ServerTlsConfig::new()
                .identity(server_identity)
                .client_ca_root(tonic::transport::Certificate::from_pem(ca.pem())),
        )
        .unwrap()
        .add_service(PluginLifecycleServer::with_interceptor(
            service.clone(),
            tls::require_kernel,
        ))
        .add_service(DataMigrationServer::with_interceptor(
            service.clone(),
            tls::require_kernel,
        ));
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve_with_incoming_shutdown(
        tokio_stream::wrappers::TcpListenerStream::new(listener),
        async {
            let _ = stopped.await;
        },
    ));
    let endpoint = tonic::transport::Endpoint::from_shared(format!("https://{address}"))
        .unwrap()
        .timeout(Duration::from_secs(120));
    let trusted = tonic::transport::ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca.pem()))
        .domain_name(&hostname);
    let other = endpoint
        .clone()
        .tls_config(trusted.clone().identity(leaf(
            &ca,
            &key,
            "other.xelab.local",
            "spiffe://xelab.local/instance/other",
        )))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let denied = DataMigrationClient::new(other)
        .plan(rpc::PlanMigrationRequest::default())
        .await
        .unwrap_err();
    assert_eq!(denied.code(), tonic::Code::PermissionDenied);
    let wrong_server = endpoint
        .clone()
        .tls_config(
            trusted
                .clone()
                .domain_name("wrong.xelab.local")
                .identity(leaf(
                    &ca,
                    &key,
                    "kernel.xelab.local",
                    "spiffe://xelab.local/kernel",
                )),
        )
        .unwrap();
    assert!(wrong_server.connect().await.is_err());
    let channel = endpoint
        .tls_config(trusted.identity(leaf(
            &ca,
            &key,
            "kernel.xelab.local",
            "spiffe://xelab.local/kernel",
        )))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut peer = Peer {
        binding,
        channel,
        deadline: tokio::time::Instant::now() + Duration::from_secs(120),
    };
    peer.describe(&manifest, &signed_digest).await.unwrap();
    service.mode.store(1, Ordering::SeqCst);
    assert!(peer.describe(&manifest, &signed_digest).await.is_err());
    for mode in [2, 3, 4] {
        service.mode.store(mode, Ordering::SeqCst);
        assert!(peer.plan().await.is_err());
    }
    service.mode.store(0, Ordering::SeqCst);
    assert_eq!(peer.plan().await.unwrap(), plan);
    assert_eq!(fs::read_to_string(&format).unwrap(), "1");
    assert!(peer.apply(&plan, Uuid::nil(), 9).await.is_err());
    assert!(peer.apply(&plan, Uuid::new_v4(), 0).await.is_err());
    assert_eq!(service.applies.load(Ordering::SeqCst), 0);
    service.mode.store(8, Ordering::SeqCst);
    assert_eq!(
        peer.apply(&plan, Uuid::new_v4(), 9).await.unwrap_err().code,
        "OUTCOME_UNKNOWN"
    );
    assert_eq!(service.applies.load(Ordering::SeqCst), 1);
    service.mode.store(0, Ordering::SeqCst);
    peer.verify(&plan, 9).await.unwrap();
    assert_eq!(service.applies.load(Ordering::SeqCst), 1);
    for mode in [5, 6, 7] {
        service.mode.store(mode, Ordering::SeqCst);
        assert!(peer.verify(&plan, 9).await.is_err());
    }
    service.mode.store(0, Ordering::SeqCst);
    fs::write(&format, "1").unwrap();
    assert!(peer.verify(&plan, 9).await.is_err());
    assert_eq!(fs::read_to_string(&format).unwrap(), "1");
    peer.deadline = tokio::time::Instant::now() - Duration::from_secs(1);
    assert!(peer.plan().await.is_err());
    assert_eq!(service.applies.load(Ordering::SeqCst), 1);
    drop(peer);
    let _ = stop.send(());
    task.await.unwrap().unwrap();
}
