//! Authenticated plugin-to-kernel service. Unsupported profile methods fail with
//! UNIMPLEMENTED; LeaseSecret never accepts a body-supplied workload identity.
use crate::{
    api::App,
    error::{Error, Result},
    rpc::{
        self,
        host_service_server::{HostService, HostServiceServer},
    },
};
use std::{
    pin::Pin,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

type Stream<T> = Pin<Box<dyn tokio_stream::Stream<Item = std::result::Result<T, Status>> + Send>>;
#[derive(Clone, Copy)]
struct Workload {
    instance: Uuid,
    generation: i64,
}
#[allow(clippy::result_large_err)] // Tonic authentication returns the transport status type.
fn workload<T>(request: &Request<T>) -> std::result::Result<Workload, Status> {
    let certs = request
        .peer_certs()
        .ok_or_else(|| Status::unauthenticated("mTLS required"))?;
    let leaf = certs
        .first()
        .ok_or_else(|| Status::unauthenticated("mTLS required"))?;
    let (_, parsed) = x509_parser::parse_x509_certificate(leaf.as_ref())
        .map_err(|_| Status::unauthenticated("Invalid certificate"))?;
    if !parsed.validity().is_valid() {
        return Err(Status::unauthenticated("Workload certificate expired"));
    }
    let san = parsed
        .subject_alternative_name()
        .map_err(|_| Status::unauthenticated("Invalid SAN"))?
        .ok_or_else(|| Status::permission_denied("Workload identity required"))?;
    let mut found = None;
    for name in &san.value.general_names {
        if let x509_parser::extensions::GeneralName::URI(uri) = name {
            let identity = parse_identity(uri)?;
            if found.replace(identity).is_some() {
                return Err(Status::permission_denied("Ambiguous workload identity"));
            }
        }
    }
    found.ok_or_else(|| Status::permission_denied("Workload identity required"))
}
#[allow(clippy::result_large_err)] // Tonic authentication returns the transport status type.
fn parse_identity(uri: &str) -> std::result::Result<Workload, Status> {
    let denied = || Status::permission_denied("Workload identity required");
    let tail = uri
        .strip_prefix("spiffe://xelab.local/instance/")
        .ok_or_else(denied)?;
    let (instance, generation) = tail.split_once("/generation/").ok_or_else(denied)?;
    let parsed: Uuid = instance.parse().map_err(|_| denied())?;
    let version: i64 = generation.parse().map_err(|_| denied())?;
    if parsed.to_string() != instance || version <= 0 || version.to_string() != generation {
        return Err(denied());
    }
    Ok(Workload {
        instance: parsed,
        generation: version,
    })
}
#[derive(Clone)]
pub struct Host {
    pub app: Arc<App>,
    leases: Arc<tokio::sync::Semaphore>,
    blobs: Arc<tokio::sync::Semaphore>,
    uploads: Arc<tokio::sync::Semaphore>,
    proxy_slots: Arc<tokio::sync::Semaphore>,
}
#[tonic::async_trait]
impl HostService for Host {
    type InvokeSkillStreamStream = Stream<rpc::InvokeChunk>;
    type InvokeStreamStream = Stream<rpc::InvokeChunk>;
    type SubscribeStream = Stream<rpc::EventDelivery>;
    type ReadBlobStream = Stream<rpc::BlobChunk>;
    async fn lease_secret(
        &self,
        r: Request<rpc::LeaseSecretRequest>,
    ) -> std::result::Result<Response<rpc::SecretLease>, Status> {
        let _permit = self
            .leases
            .try_acquire()
            .map_err(|_| Status::resource_exhausted("Secret lease capacity is unavailable"))?;
        let identity = workload(&r)?;
        let request = r.into_inner();
        let duration = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 2000) as u64;
        let result = tokio::time::timeout(
            Duration::from_millis(duration),
            crate::secrets::lease(&self.app, identity.instance, identity.generation, &request),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Lease deadline exceeded"))?;
        result
            .map(Response::new)
            .map_err(|e| match e.status.as_u16() {
                400 => Status::invalid_argument(e.message),
                401 => Status::unauthenticated(e.message),
                403 | 404 => Status::permission_denied("Secret lease is not authorized"),
                409 => Status::failed_precondition(e.message),
                _ => Status::unavailable("Secret lease is unavailable"),
            })
    }
    async fn attach_micro_worker(
        &self,
        _: Request<rpc::AttachMicroWorkerRequest>,
    ) -> std::result::Result<Response<rpc::Ack>, Status> {
        Err(Status::unimplemented(
            "Method is unavailable in this kernel profile",
        ))
    }
    async fn proxy_http(
        &self,
        _: Request<rpc::MicroProxyHttpRequest>,
    ) -> std::result::Result<Response<rpc::MicroProxyHttpResponse>, Status> {
        Err(Status::unimplemented(
            "Method is unavailable in this kernel profile",
        ))
    }
    async fn list_skill_candidates(
        &self,
        request: Request<rpc::ListSkillCandidatesRequest>,
    ) -> std::result::Result<Response<rpc::ListSkillCandidatesResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 2000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::list_skill_candidates(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn resolve_skill(
        &self,
        request: Request<rpc::ResolveSkillRequest>,
    ) -> std::result::Result<Response<rpc::ResolveSkillResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 2000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::resolve_skill(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn check_skill_access(
        &self,
        request: Request<rpc::CheckSkillAccessRequest>,
    ) -> std::result::Result<Response<rpc::CheckSkillAccessResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 2000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::check_skill_access(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn invoke_skill(
        &self,
        request: Request<rpc::SkillInvokeRequest>,
    ) -> std::result::Result<Response<rpc::InvokeResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 300000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::invoke_skill(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn invoke_skill_stream(
        &self,
        request: Request<rpc::SkillInvokeRequest>,
    ) -> std::result::Result<Response<Self::InvokeSkillStreamStream>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 300000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::invoke_skill_stream(
                self.app.clone(),
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        use futures_util::StreamExt;
        let result = result.map(move |frame| {
            let _held = &permit;
            frame
        });
        Ok(Response::new(Box::pin(result)))
    }
    async fn resolve(
        &self,
        request: Request<rpc::ResolveRequest>,
    ) -> std::result::Result<Response<rpc::ResolveResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 2000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::resolve(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn invoke(
        &self,
        request: Request<rpc::InvokeRequest>,
    ) -> std::result::Result<Response<rpc::InvokeResponse>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 300000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::invoke(
                &self.app,
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        drop(permit);
        Ok(Response::new(result))
    }
    async fn invoke_stream(
        &self,
        request: Request<rpc::InvokeRequest>,
    ) -> std::result::Result<Response<Self::InvokeStreamStream>, Status> {
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let permit =
            self.proxy_slots.clone().try_acquire_owned().map_err(|_| {
                Status::resource_exhausted("HostService proxy capacity is unavailable")
            })?;
        let request = request.into_inner();
        let milliseconds = request
            .context
            .as_ref()
            .map(|c| c.deadline_unix_ms - chrono::Utc::now().timestamp_millis())
            .unwrap_or(0)
            .clamp(0, 300000) as u64;
        let bound = deadline.min(tokio::time::Instant::now() + Duration::from_millis(milliseconds));
        let result = tokio::time::timeout_at(
            bound,
            crate::host_calls::invoke_stream(
                self.app.clone(),
                crate::host_calls::Workload {
                    instance: identity.instance,
                    generation: identity.generation,
                },
                request,
            ),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("HostService deadline elapsed"))?
        .map_err(crate::host_calls::status)
        .map_err(crate::host_blobs::rpc_error)?;
        use futures_util::StreamExt;
        let result = result.map(move |frame| {
            let _held = &permit;
            frame
        });
        Ok(Response::new(Box::pin(result)))
    }
    async fn publish_event(
        &self,
        _: Request<rpc::PublishEventRequest>,
    ) -> std::result::Result<Response<rpc::PublishEventResponse>, Status> {
        Err(Status::unimplemented(
            "Method is unavailable in this kernel profile",
        ))
    }
    async fn subscribe(
        &self,
        _: Request<rpc::SubscribeRequest>,
    ) -> std::result::Result<Response<Self::SubscribeStream>, Status> {
        Err(Status::unimplemented(
            "Method is unavailable in this kernel profile",
        ))
    }
    async fn ack_event(
        &self,
        _: Request<rpc::AckEventRequest>,
    ) -> std::result::Result<Response<rpc::Ack>, Status> {
        Err(Status::unimplemented(
            "Method is unavailable in this kernel profile",
        ))
    }
    async fn read_blob(
        &self,
        request: Request<rpc::ReadBlobRequest>,
    ) -> std::result::Result<Response<Self::ReadBlobStream>, Status> {
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let permit = self.blobs.clone().try_acquire_owned().map_err(|_| {
            crate::host_blobs::rpc_error(Status::resource_exhausted(
                "Blob read capacity is unavailable",
            ))
        })?;
        let workload = crate::host_blobs::Workload {
            instance: identity.instance,
            generation: identity.generation,
        };
        crate::host_blobs::read(
            self.app.clone(),
            workload,
            request.into_inner(),
            permit,
            deadline,
        )
        .await
        .map(Response::new)
        .map_err(crate::host_blobs::rpc_error)
    }
    async fn write_blob(
        &self,
        request: Request<tonic::Streaming<rpc::WriteBlobRequest>>,
    ) -> std::result::Result<Response<rpc::BlobReference>, Status> {
        let deadline = crate::host_blobs::transport_deadline(&request)
            .map_err(crate::host_blobs::rpc_error)?;
        let identity = workload(&request).map_err(crate::host_blobs::rpc_error)?;
        let workload = crate::host_blobs::Workload {
            instance: identity.instance,
            generation: identity.generation,
        };
        let permit = self.uploads.clone().try_acquire_owned().map_err(|_| {
            crate::host_blobs::rpc_error(Status::resource_exhausted(
                "Blob worker capacity is unavailable",
            ))
        })?;
        crate::host_blobs::write(&self.app, workload, request.into_inner(), permit, deadline)
            .await
            .map(Response::new)
            .map_err(crate::host_blobs::rpc_error)
    }
}

// Resolve the atomically rotated certificate on each new TLS handshake. Existing
// authenticated channels retain their peer identity; each lease rechecks DB state.
#[derive(Debug)]
struct RotatingIdentity {
    directory: std::path::PathBuf,
}
impl rustls::server::ResolvesServerCert for RotatingIdentity {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let certs = std::fs::read(self.directory.join("tls.crt")).ok()?;
        let key = crate::config::read_private(&self.directory.join("tls.key"), 16384).ok()?;
        let certs = rustls_pemfile::certs(&mut &certs[..])
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        let key = rustls_pemfile::private_key(&mut &key[..]).ok()??;
        let key = rustls::crypto::ring::sign::any_supported_type(&key).ok()?;
        Some(Arc::new(rustls::sign::CertifiedKey::new(certs, key)))
    }
}

pub async fn serve(app: Arc<App>) -> Result<()> {
    use futures_util::StreamExt;
    let mut roots = rustls::RootCertStore::empty();
    let ca = std::fs::read(app.config.text("security", "workload_ca_cert_file"))?;
    for cert in rustls_pemfile::certs(&mut &ca[..]) {
        roots
            .add(cert?)
            .map_err(|_| Error::invalid("Invalid workload CA"))?;
    }
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|_| Error::invalid("Invalid workload verifier"))?;
    let mut tls = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(Arc::new(RotatingIdentity {
            directory: app.config.root().join("kernel-identity"),
        }));
    tls.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener =
        tokio::net::TcpListener::bind(app.config.text("server", "host_rpc_listen")).await?;
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener)
        .map(move |socket| {
            let acceptor = acceptor.clone();
            async move {
                let socket = socket?;
                tokio::time::timeout(Duration::from_secs(2), acceptor.accept(socket))
                    .await
                    .map_err(|_| {
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "TLS handshake timeout")
                    })?
            }
        })
        .buffer_unordered(32);
    let service = HostServiceServer::new(Host {
        app: app.clone(),
        leases: Arc::new(tokio::sync::Semaphore::new(32)),
        blobs: Arc::new(tokio::sync::Semaphore::new(16)),
        uploads: Arc::new(tokio::sync::Semaphore::new(4)),
        proxy_slots: Arc::new(tokio::sync::Semaphore::new(64)),
    })
    .max_decoding_message_size(4194304 + 65536)
    .max_encoding_message_size(16777216 + 65536);
    let stop = app.clone();
    tonic::transport::Server::builder()
        // One shared limit across all mTLS connections, before protobuf body
        // decoding. Per-method/stream pools further narrow retained responses.
        .layer(tower::limit::GlobalConcurrencyLimitLayer::new(64))
        .concurrency_limit_per_connection(16)
        .timeout(Duration::from_secs(300))
        .add_service(service)
        .serve_with_incoming_shutdown(incoming, async move {
            while !stop.shutdown.load(Ordering::SeqCst) && !stop.leader.is_closed() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| Error::unavailable("HostService stopped unexpectedly"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workload_spiffe_has_one_canonical_instance_and_positive_generation() {
        let id = Uuid::new_v4();
        let parsed =
            parse_identity(&format!("spiffe://xelab.local/instance/{id}/generation/12")).unwrap();
        assert_eq!(parsed.instance, id);
        assert_eq!(parsed.generation, 12);
        for uri in [
            "spiffe://xelab.local/kernel".to_string(),
            format!("spiffe://other/instance/{id}/generation/1"),
            format!("spiffe://xelab.local/instance/{id}/generation/0"),
            format!("spiffe://xelab.local/instance/{id}/generation/01"),
            format!("spiffe://xelab.local/instance/{id}/generation/1/extra"),
        ] {
            assert!(parse_identity(&uri).is_err());
        }
        assert!(workload(&Request::new(())).is_err());
    }
}
