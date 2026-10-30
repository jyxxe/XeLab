#![allow(clippy::result_large_err)] // Tonic service trait signatures.
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}},path::PathBuf};
use tokio::sync::RwLock;
use tonic::{Request,Response,Status};
use xelab_kernel::{contracts::{digest,strict_json},rpc::{self,plugin_lifecycle_server::{PluginLifecycle,PluginLifecycleServer},capability_service_server::{CapabilityService,CapabilityServiceServer}},tls::require_kernel};

#[derive(Clone)]struct Echo{manifest:Arc<Vec<u8>>,generation:u64,initialized:Arc<AtomicBool>,draining:Arc<AtomicBool>,results:Arc<RwLock<std::collections::HashMap<String,rpc::InvokeResponse>>>}
#[tonic::async_trait]impl PluginLifecycle for Echo{
    async fn describe(&self,_:Request<rpc::DescribeRequest>)->Result<Response<rpc::DescribeResponse>,Status>{Ok(Response::new(rpc::DescribeResponse{manifest_json:self.manifest.as_ref().clone(),manifest_digest:digest(&self.manifest)}))}
    async fn initialize(&self,r:Request<rpc::InitializeRequest>)->Result<Response<rpc::InitializeResponse>,Status>{let r=r.into_inner();if r.generation!=self.generation||r.instance_id!=std::env::var("XELAB_INSTANCE_ID").unwrap_or_default()||r.deployment_id!=std::env::var("XELAB_DEPLOYMENT_ID").unwrap_or_default()||digest(&r.config_json)!=r.config_digest{return Err(Status::failed_precondition("Identity or config differs"));}self.initialized.store(true,Ordering::SeqCst);self.draining.store(false,Ordering::SeqCst);Ok(Response::new(rpc::InitializeResponse{observed_generation:self.generation,initialized:true}))}
    async fn get_health(&self,_:Request<rpc::HealthRequest>)->Result<Response<rpc::HealthResponse>,Status>{let m=strict_json(&self.manifest,1048576).map_err(|_|Status::internal("Invalid manifest"))?;let cap=&m["capabilities"][0]["contract"];Ok(Response::new(rpc::HealthResponse{alive:true,observed_generation:self.generation,capabilities:vec![rpc::CapabilityHealth{contract:Some(rpc::ContractRef{name:cap["name"].as_str().unwrap().into(),version:cap["version"].as_str().unwrap().into(),schema_digest:cap["schema_digest"].as_str().unwrap().into()}),ready:self.initialized.load(Ordering::SeqCst)&&!self.draining.load(Ordering::SeqCst),reason_code:String::new()}]}))}
    async fn drain(&self,_:Request<rpc::DrainRequest>)->Result<Response<rpc::DrainResponse>,Status>{self.draining.store(true,Ordering::SeqCst);Ok(Response::new(rpc::DrainResponse{accepting_requests:false,active_calls:0,drained:true}))}
    async fn shutdown(&self,_:Request<rpc::ShutdownRequest>)->Result<Response<rpc::Ack>,Status>{self.draining.store(true,Ordering::SeqCst);Ok(Response::new(rpc::Ack{request_id:uuid::Uuid::new_v4().to_string(),accepted:true}))}
}
#[tonic::async_trait]impl CapabilityService for Echo{
    async fn invoke(&self,r:Request<rpc::InvokeRequest>)->Result<Response<rpc::InvokeResponse>,Status>{let r=r.into_inner();if !self.initialized.load(Ordering::SeqCst)||self.draining.load(Ordering::SeqCst){return Err(Status::unavailable("Not initialized"));}if digest(&r.input_json)!=r.input_digest{return Err(Status::invalid_argument("Digest differs"));}let input=strict_json(&r.input_json,65536).map_err(|_|Status::invalid_argument("Invalid input"))?;let output=serde_json::to_vec(&input).map_err(|_|Status::internal("Serialization failed"))?;let response=rpc::InvokeResponse{call_id:r.call_id.clone(),output_digest:digest(&output),output_json:output};let mut results=self.results.write().await;if results.len()>=1024{results.clear();}results.insert(r.call_id,response.clone());Ok(Response::new(response))}
    type InvokeStreamStream=tokio_stream::wrappers::ReceiverStream<Result<rpc::InvokeChunk,Status>>;
    async fn invoke_stream(&self,_:Request<rpc::InvokeRequest>)->Result<Response<Self::InvokeStreamStream>,Status>{Err(Status::unimplemented("No streaming contract"))}
    async fn cancel(&self,r:Request<rpc::CancelCallRequest>)->Result<Response<rpc::Ack>,Status>{Ok(Response::new(rpc::Ack{request_id:r.into_inner().context.map(|c|c.request_id).unwrap_or_default(),accepted:true}))}
    async fn query_result(&self,r:Request<rpc::QueryResultRequest>)->Result<Response<rpc::QueryResultResponse>,Status>{let map=self.results.read().await;let result=map.get(&r.into_inner().call_id).cloned();Ok(Response::new(rpc::QueryResultResponse{state:if result.is_some(){rpc::ExecutionState::Succeeded as i32}else{rpc::ExecutionState::Unknown as i32},outcome:result.map(rpc::query_result_response::Outcome::Completed)}))}
}

#[tokio::main]async fn main()->Result<(),Box<dyn std::error::Error>>{
    xelab_kernel::init_crypto();
    let echo=Echo{manifest:Arc::new(std::fs::read("/xelab/plugin.json")?),generation:std::env::var("XELAB_GENERATION")?.parse()?,initialized:Arc::new(AtomicBool::new(false)),draining:Arc::new(AtomicBool::new(false)),results:Default::default()};
    loop{
        let cert=std::fs::read(PathBuf::from(std::env::var("XELAB_TLS_CERT_PATH")?))?;let key=std::fs::read(std::env::var("XELAB_TLS_KEY_PATH")?)?;let ca=std::fs::read(std::env::var("XELAB_TLS_CA_PATH")?)?;
        let tls=tonic::transport::ServerTlsConfig::new().identity(tonic::transport::Identity::from_pem(cert,key)).client_ca_root(tonic::transport::Certificate::from_pem(ca));
        tonic::transport::Server::builder().tls_config(tls)?.add_service(PluginLifecycleServer::with_interceptor(echo.clone(),require_kernel)).add_service(CapabilityServiceServer::with_interceptor(echo.clone(),require_kernel)).serve_with_shutdown("0.0.0.0:7000".parse()?,async{tokio::time::sleep(std::time::Duration::from_secs(300)).await;}).await?;
    }
}
