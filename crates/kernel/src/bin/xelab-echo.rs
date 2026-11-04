#![allow(clippy::result_large_err)] // Tonic service trait signatures.
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{watch, RwLock};
use tonic::{Request, Response, Status};
use xelab_kernel::{
    contracts::{digest, strict_json, validate_business},
    rpc::{
        self,
        capability_service_server::{CapabilityService, CapabilityServiceServer},
        plugin_lifecycle_server::{PluginLifecycle, PluginLifecycleServer},
    },
    tls::require_kernel,
};

#[derive(Clone)]
struct Echo {
    manifest: Arc<Vec<u8>>,
    generation: u64,
    initialized: Arc<AtomicBool>,
    draining: Arc<AtomicBool>,
    results: Arc<RwLock<std::collections::HashMap<String, rpc::InvokeResponse>>>,
    active_streams: Arc<RwLock<std::collections::HashMap<String, watch::Sender<bool>>>>,
}
#[tonic::async_trait]
impl PluginLifecycle for Echo {
    async fn describe(
        &self,
        _: Request<rpc::DescribeRequest>,
    ) -> Result<Response<rpc::DescribeResponse>, Status> {
        Ok(Response::new(rpc::DescribeResponse {
            manifest_json: self.manifest.as_ref().clone(),
            manifest_digest: digest(&self.manifest),
        }))
    }
    async fn initialize(
        &self,
        r: Request<rpc::InitializeRequest>,
    ) -> Result<Response<rpc::InitializeResponse>, Status> {
        let r = r.into_inner();
        if r.generation != self.generation
            || r.instance_id != std::env::var("XELAB_INSTANCE_ID").unwrap_or_default()
            || r.deployment_id != std::env::var("XELAB_DEPLOYMENT_ID").unwrap_or_default()
            || digest(&r.config_json) != r.config_digest
        {
            return Err(Status::failed_precondition("Identity or config differs"));
        }
        self.initialized.store(true, Ordering::SeqCst);
        self.draining.store(false, Ordering::SeqCst);
        Ok(Response::new(rpc::InitializeResponse {
            observed_generation: self.generation,
            initialized: true,
        }))
    }
    async fn get_health(
        &self,
        _: Request<rpc::HealthRequest>,
    ) -> Result<Response<rpc::HealthResponse>, Status> {
        let m = strict_json(&self.manifest, 1048576)
            .map_err(|_| Status::internal("Invalid manifest"))?;
        Ok(Response::new(rpc::HealthResponse {
            alive: true,
            observed_generation: self.generation,
            capabilities: m["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|cap| rpc::CapabilityHealth {
                    contract: Some(rpc::ContractRef {
                        name: cap["contract"]["name"].as_str().unwrap().into(),
                        version: cap["contract"]["version"].as_str().unwrap().into(),
                        schema_digest: cap["contract"]["schema_digest"].as_str().unwrap().into(),
                    }),
                    ready: self.initialized.load(Ordering::SeqCst)
                        && !self.draining.load(Ordering::SeqCst),
                    reason_code: String::new(),
                })
                .collect(),
        }))
    }
    async fn drain(
        &self,
        _: Request<rpc::DrainRequest>,
    ) -> Result<Response<rpc::DrainResponse>, Status> {
        self.draining.store(true, Ordering::SeqCst);
        let active = self.active_streams.read().await;
        for cancellation in active.values() {
            let _ = cancellation.send(true);
        }
        Ok(Response::new(rpc::DrainResponse {
            accepting_requests: false,
            active_calls: active.len() as u32,
            drained: active.is_empty(),
        }))
    }
    async fn shutdown(
        &self,
        _: Request<rpc::ShutdownRequest>,
    ) -> Result<Response<rpc::Ack>, Status> {
        self.draining.store(true, Ordering::SeqCst);
        Ok(Response::new(rpc::Ack {
            request_id: uuid::Uuid::new_v4().to_string(),
            accepted: true,
        }))
    }
}
#[tonic::async_trait]
impl CapabilityService for Echo {
    async fn invoke(
        &self,
        r: Request<rpc::InvokeRequest>,
    ) -> Result<Response<rpc::InvokeResponse>, Status> {
        let r = r.into_inner();
        if !self.initialized.load(Ordering::SeqCst) || self.draining.load(Ordering::SeqCst) {
            return Err(Status::unavailable("Not initialized"));
        }
        if digest(&r.input_json) != r.input_digest {
            return Err(Status::invalid_argument("Digest differs"));
        }
        let input = strict_json(&r.input_json, 65536)
            .map_err(|_| Status::invalid_argument("Invalid input"))?;
        let value = if r
            .contract
            .as_ref()
            .is_some_and(|contract| contract.name == "org.xelab.echo.stream")
        {
            serde_json::json!({"message":input["message"]})
        } else {
            input
        };
        let output =
            serde_json::to_vec(&value).map_err(|_| Status::internal("Serialization failed"))?;
        let response = rpc::InvokeResponse {
            call_id: r.call_id.clone(),
            output_digest: digest(&output),
            output_json: output,
        };
        let mut results = self.results.write().await;
        if results.len() >= 1024 {
            results.clear();
        }
        results.insert(r.call_id, response.clone());
        Ok(Response::new(response))
    }
    type InvokeStreamStream =
        tokio_stream::wrappers::ReceiverStream<Result<rpc::InvokeChunk, Status>>;
    async fn invoke_stream(
        &self,
        request: Request<rpc::InvokeRequest>,
    ) -> Result<Response<Self::InvokeStreamStream>, Status> {
        if !self.initialized.load(Ordering::SeqCst) || self.draining.load(Ordering::SeqCst) {
            return Err(Status::unavailable("Not initialized"));
        }
        let request = request.into_inner();
        let context = request
            .context
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Context required"))?;
        if context.deadline_unix_ms <= chrono::Utc::now().timestamp_millis()
            || context.delegation_token.is_empty()
            || digest(&request.input_json) != request.input_digest
        {
            return Err(Status::invalid_argument(
                "Invalid request context or digest",
            ));
        }
        let manifest = strict_json(&self.manifest, 1048576)
            .map_err(|_| Status::internal("Invalid manifest"))?;
        let contract = request
            .contract
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Contract required"))?;
        let cap = manifest["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|cap| {
                cap["contract"]["name"] == contract.name
                    && cap["contract"]["version"] == contract.version
                    && cap["contract"]["schema_digest"] == contract.schema_digest
                    && !cap["stream_item_schema"].is_null()
            })
            .ok_or_else(|| Status::invalid_argument("No matching signed streaming contract"))?;
        let input = strict_json(&request.input_json, 65536)
            .map_err(|_| Status::invalid_argument("Invalid input"))?;
        validate_business(&cap["input_schema"], &input)
            .map_err(|_| Status::invalid_argument("Input schema differs"))?;
        let count = input["count"].as_u64().unwrap_or(3);
        let delay = std::time::Duration::from_millis(input["delay_ms"].as_u64().unwrap_or(0));
        let fault = input["fault"].as_str().unwrap_or("none").to_owned();
        let message = input["message"].as_str().unwrap().to_owned();
        let call = request.call_id;
        let deadline = context.deadline_unix_ms;
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let (cancellation, mut canceled) = watch::channel(false);
        let mut active = self.active_streams.write().await;
        if active.len() >= 4 || active.contains_key(&call) {
            return Err(Status::resource_exhausted("Stream capacity unavailable"));
        }
        active.insert(call.clone(), cancellation);
        drop(active);
        let this = self.clone();
        tokio::spawn(async move {
            let produce = async {
                for index in 0..count {
                    tokio::select! {
                        _ = sender.closed() => return,
                        _ = canceled.changed() => return,
                        _ = tokio::time::sleep(delay) => (),
                    }
                    if this.draining.load(Ordering::SeqCst)
                        || chrono::Utc::now().timestamp_millis() >= deadline
                    {
                        return;
                    }
                    let item = if fault == "item" && index == 0 {
                        serde_json::json!({"index":"invalid","message":message})
                    } else {
                        serde_json::json!({"index":index,"message":message})
                    };
                    let sequence = index + 1 + u64::from(fault == "sequence" && index > 0);
                    let chunk = rpc::InvokeChunk {
                        call_id: if fault == "call-id" && index == 0 {
                            uuid::Uuid::nil().to_string()
                        } else {
                            call.clone()
                        },
                        sequence,
                        body: Some(rpc::invoke_chunk::Body::ItemJson(
                            serde_json::to_vec(&item).unwrap(),
                        )),
                    };
                    tokio::select! {
                        _ = canceled.changed() => return,
                        sent = sender.send(Ok(chunk)) => if sent.is_err() { return; },
                    }
                }
                if fault == "missing-terminal" {
                    return;
                }
                let value = if fault == "output" {
                    serde_json::json!({"message":false})
                } else {
                    serde_json::json!({"message":message})
                };
                let output = serde_json::to_vec(&value).unwrap();
                let response = rpc::InvokeResponse {
                    call_id: call.clone(),
                    output_digest: if fault == "digest" {
                        digest(b"different")
                    } else {
                        digest(&output)
                    },
                    output_json: output,
                };
                let completed = rpc::InvokeChunk {
                    call_id: call.clone(),
                    sequence: count + 1,
                    body: Some(rpc::invoke_chunk::Body::Completed(response.clone())),
                };
                if sender.send(Ok(completed)).await.is_err() {
                    return;
                }
                if fault == "after-terminal" {
                    let _ = sender
                        .send(Ok(rpc::InvokeChunk {
                            call_id: call.clone(),
                            sequence: count + 2,
                            body: Some(rpc::invoke_chunk::Body::ItemJson(
                                serde_json::to_vec(
                                    &serde_json::json!({"index":0,"message":message}),
                                )
                                .unwrap(),
                            )),
                        }))
                        .await;
                }
                if fault == "none" {
                    let mut results = this.results.write().await;
                    if results.len() >= 1024 {
                        results.clear();
                    }
                    results.insert(call.clone(), response);
                }
            };
            let remaining = std::time::Duration::from_millis(
                (deadline - chrono::Utc::now().timestamp_millis()).max(0) as u64,
            );
            let _ = tokio::time::timeout(remaining, produce).await;
            this.active_streams.write().await.remove(&call);
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            receiver,
        )))
    }
    async fn cancel(
        &self,
        r: Request<rpc::CancelCallRequest>,
    ) -> Result<Response<rpc::Ack>, Status> {
        let request = r.into_inner();
        if let Some(cancellation) = self.active_streams.read().await.get(&request.call_id) {
            let _ = cancellation.send(true);
        }
        Ok(Response::new(rpc::Ack {
            request_id: request.context.map(|c| c.request_id).unwrap_or_default(),
            accepted: true,
        }))
    }
    async fn query_result(
        &self,
        r: Request<rpc::QueryResultRequest>,
    ) -> Result<Response<rpc::QueryResultResponse>, Status> {
        let map = self.results.read().await;
        let result = map.get(&r.into_inner().call_id).cloned();
        Ok(Response::new(rpc::QueryResultResponse {
            state: if result.is_some() {
                rpc::ExecutionState::Succeeded as i32
            } else {
                rpc::ExecutionState::Unknown as i32
            },
            outcome: result.map(rpc::query_result_response::Outcome::Completed),
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    xelab_kernel::init_crypto();
    let echo = Echo {
        manifest: Arc::new(std::fs::read("/xelab/plugin.json")?),
        generation: std::env::var("XELAB_GENERATION")?.parse()?,
        initialized: Arc::new(AtomicBool::new(false)),
        draining: Arc::new(AtomicBool::new(false)),
        results: Default::default(),
        active_streams: Default::default(),
    };
    loop {
        let cert = std::fs::read(PathBuf::from(std::env::var("XELAB_TLS_CERT_PATH")?))?;
        let key = std::fs::read(std::env::var("XELAB_TLS_KEY_PATH")?)?;
        let ca = std::fs::read(std::env::var("XELAB_TLS_CA_PATH")?)?;
        let tls = tonic::transport::ServerTlsConfig::new()
            .identity(tonic::transport::Identity::from_pem(cert, key))
            .client_ca_root(tonic::transport::Certificate::from_pem(ca));
        tonic::transport::Server::builder()
            .tls_config(tls)?
            .add_service(PluginLifecycleServer::with_interceptor(
                echo.clone(),
                require_kernel,
            ))
            .add_service(CapabilityServiceServer::with_interceptor(
                echo.clone(),
                require_kernel,
            ))
            .serve_with_shutdown("0.0.0.0:7000".parse()?, async {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            })
            .await?;
    }
}
