use crate::{admission,artifact::Artifact,auth::{self,Auth,Context,Resource},broker,config::Config,contracts::{self,id,openapi,strict_json,string,validate},db::{self,Db},error::{Error,Result},runtime::Runtime};
use axum::{body::{to_bytes,Body},extract::{ConnectInfo,State},http::{HeaderMap,Request,StatusCode},response::{IntoResponse,Response},Json,Router};
use ed25519_dalek::pkcs8::DecodePublicKey;
use serde_json::{json,Value};
use std::{collections::HashMap,net::SocketAddr,sync::{Arc,atomic::{AtomicBool,Ordering}}};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub struct App{pub config:Config,pub db:Db,pub auth:Auth,pub runtime:Runtime,pub leader:Arc<tokio_postgres::Client>,pub ready:AtomicBool,pub shutdown:AtomicBool}
pub fn router(app:Arc<App>)->Router{Router::new().fallback(handle).with_state(app)}

fn operation(method:&str,path:&str)->Option<(&'static Value,HashMap<String,String>)>{
    for(template,item)in openapi()["paths"].as_object()?{
        let Some(op)=item.get(method.to_ascii_lowercase())else{continue;};let a:Vec<_>=template.split('/').collect();let b:Vec<_>=path.split('/').collect();if a.len()!=b.len(){continue;}
        let mut params=HashMap::new();let mut matched=true;
        for(a,b)in a.iter().zip(b){if a.starts_with('{')&&a.ends_with('}'){params.insert(a[1..a.len()-1].to_owned(),b.to_owned());}else if *a!=b{matched=false;break;}}
        if matched{return Some((op,params));}
    }None
}
fn header<'a>(headers:&'a HeaderMap,name:&str)->Result<&'a str>{if headers.get_all(name).iter().count()!=1{return Err(Error::invalid("Required header must occur exactly once"));}headers.get(name).and_then(|s|s.to_str().ok()).ok_or_else(||Error::invalid("Missing or invalid required header"))}
fn bearer(headers:&HeaderMap)->Result<&str>{header(headers,"authorization").map_err(|_|Error::credential())?.strip_prefix("Bearer ").filter(|s|!s.is_empty()).ok_or_else(Error::credential)}
fn path_id(p:&HashMap<String,String>,name:&str)->Result<Uuid>{p.get(name).and_then(|s|s.parse().ok()).ok_or_else(||Error::invalid("Invalid path UUID"))}
fn etag(headers:&HeaderMap,revision:i64)->Result<()> {let value=header(headers,"if-match").map_err(|_|Error::new("PRECONDITION_REQUIRED","If-Match is required",StatusCode::PRECONDITION_REQUIRED))?;if value!=format!("\"rev-{revision}\""){return Err(Error::new("ETAG_MISMATCH","Resource revision changed",StatusCode::PRECONDITION_FAILED));}Ok(())}
fn response(value:Value,code:u16,request:Uuid)->Response{let mut r=if code==204{StatusCode::NO_CONTENT.into_response()}else{(StatusCode::from_u16(code).unwrap(),Json(value.clone())).into_response()};r.headers_mut().insert("x-request-id",request.to_string().parse().unwrap());r.headers_mut().insert("cache-control","no-store".parse().unwrap());if let Some(rev)=value["revision"].as_i64(){r.headers_mut().insert("etag",format!("\"rev-{rev}\"").parse().unwrap());}r}

async fn handle(State(app):State<Arc<App>>,ConnectInfo(peer):ConnectInfo<SocketAddr>,request:Request<Body>)->Response{
    let request_id=Uuid::new_v4();
    match dispatch(&app,peer,request_id,request).await{Ok((v,s))=>response(v,s,request_id),Err(e)=>{tracing::warn!(request_id=%request_id,reason_code=e.code,"Request rejected");let status=e.status;let mut r=response(json!({"error":e.detail(),"request_id":request_id}),status.as_u16(),request_id);if e.code=="RATE_LIMITED"{r.headers_mut().insert("retry-after","60".parse().unwrap());}r}}
}

async fn dispatch(app:&App,peer:SocketAddr,request_id:Uuid,request:Request<Body>)->Result<(Value,u16)>{
    let(method,uri,headers)=(request.method().to_string(),request.uri().clone(),request.headers().clone());let path=uri.path();
    if path.contains('%')||path.contains("//"){return Err(Error::invalid("Noncanonical API path"));}
    let(op,params)=operation(&method,path).ok_or_else(Error::not_found)?;let name=op["operationId"].as_str().unwrap();
    if name=="getLiveness"{return Ok((json!({"status":"ok","spec_version":crate::SPEC_VERSION}),200));}
    if name=="getReadiness"{if !app.ready.load(Ordering::SeqCst)||app.leader.is_closed()||app.db.check().await.is_err(){return Err(Error::unavailable("Kernel reconciliation is incomplete"));}return Ok((json!({"status":"ok","spec_version":crate::SPEC_VERSION}),200));}
    if let Some(origin)=headers.get("origin"){if origin.to_str().ok()!=Some(app.config.text("server","public_origin")){return Err(Error::denied());}}
    let bytes=to_bytes(request.into_body(),4194304).await.map_err(|_|Error::new("PAYLOAD_TOO_LARGE","Request body exceeds 4 MiB",StatusCode::PAYLOAD_TOO_LARGE))?;
    let body=if op.get("requestBody").is_some(){if header(&headers,"content-type")?.split(';').next()!=Some("application/json"){return Err(Error::new("UNSUPPORTED_MEDIA_TYPE","Expected application/json",StatusCode::UNSUPPORTED_MEDIA_TYPE));}let value=strict_json(&bytes,4194304)?;let model=op["requestBody"]["content"]["application/json"]["schema"]["$ref"].as_str().and_then(|s|s.rsplit('/').next()).ok_or_else(||Error::invalid("This operation is not supported"))?;validate(model,&value)?;value}else{if !bytes.is_empty(){return Err(Error::invalid("Unexpected request body"));}Value::Null};
    for parameter in op["parameters"].as_array().unwrap(){let source=parameter["in"].as_str().unwrap_or("");if source=="header"&&parameter["required"]==true{let value=header(&headers,parameter["name"].as_str().unwrap())?;contracts::validate_business(&parameter["schema"],&json!(value))?;}}
    if name=="login"{
        let mut c=app.db.connect().await?;let tx=c.transaction().await?;let result=app.auth.login(&tx,id(&body,"account_id")?,string(&body,"password")?.to_owned(),peer.ip(),request_id).await;tx.commit().await?;return result.map(|v|(v,200));
    }
    if app.leader.is_closed(){return Err(Error::unavailable("Controller leadership is lost"));}
    let token=bearer(&headers)?;
    if name=="invokeCapability"{if !app.ready.load(Ordering::SeqCst){return Err(Error::unavailable("Kernel is not ready"));}return broker::invoke(app,token,peer.ip(),request_id,&body,header(&headers,"idempotency-key")?).await.map(|v|(v,200));}
    let mut c=app.db.connect().await?;let tx=c.transaction().await?;let ctx=app.auth.authenticate(&tx,token,peer.ip(),request_id).await?;
    let mutating=matches!(method.as_str(),"POST"|"PUT"|"PATCH"|"DELETE");
    let key=if mutating{Some(header(&headers,"idempotency-key")?)}else{None};let fingerprint=contracts::digest(&serde_json::to_vec(&json!({"body":body,"if_match":headers.get("if-match").and_then(|v|v.to_str().ok())}))?);
    if let Some(key)=key{
        let lock=format!("{}:{}:{method}:{path}:{key}",ctx.subject,ctx.credential);tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended($1,0))",&[&lock]).await?;
        if let Some(row)=tx.query_opt("SELECT fingerprint,safe_response,response_status FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method=$3 AND path=$4 AND key=$5 AND expires_at>now()",&[&ctx.subject,&ctx.credential,&method,&path,&key]).await?{
            if row.get::<_,String>("fingerprint")!=fingerprint{return Err(Error::new("IDEMPOTENCY_CONFLICT","Idempotency key fingerprint differs",StatusCode::CONFLICT));}
            // Authentication has been rechecked; resource authorization is checked by the same dispatch before any replay below.
            let response:Option<Value>=row.get("safe_response");if let Some(value)=response{authorize_operation(&tx,&ctx,name,&params,&body).await?;return Ok((value,row.get::<_,i32>("response_status") as u16));}return Err(Error::conflict("Operation has no replayable response"));
        }
    }
    if let Err(error)=authorize_operation(&tx,&ctx,name,&params,&body).await{
        db::audit(&tx,ctx.subject,Some(ctx.credential),request_id,name,&json!({"kind":"platform","match":"all"}),"denied",error.code).await?;tx.commit().await?;return Err(error);
    }
    let result=match name{
        "getNode"=>(admission::status(&tx).await?,200),
        "getOperation"=>(db::operation_value(&tx,path_id(&params,"operation_id")?).await?,200),
        "trustPublisher"=>{
            ed25519_dalek::VerifyingKey::from_public_key_pem(string(&body,"public_key_pem")?).map_err(|_|Error::invalid("Expected an Ed25519 public key"))?;
            let id=Uuid::new_v4();tx.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,$2,$3,$4,'trusted',now(),now())",&[&id,&string(&body,"name")?,&string(&body,"key_id")?,&string(&body,"public_key_pem")?]).await?;(read_object(&tx,"publishers",id,"Publisher").await?,201)
        },
        "installPlugin"=>{let publisher=id(&body,"publisher_id")?;if tx.query_opt("SELECT id FROM publishers WHERE id=$1 AND status='trusted' FOR SHARE",&[&publisher]).await?.is_none(){return Err(Error::denied());}let id=db::operation(&tx,ctx.subject,name,None).await?;db::outbox(&tx,"kernel.install",id,&json!({"request":body,"credential_id":ctx.credential,"subject_id":ctx.subject})).await?;(db::operation_value(&tx,id).await?,202)},
        "createDeployment"=>{
            if body["enabled"]==true{return Err(Error::invalid("Create a disabled deployment, approve it, then enable it"));}
            let installation=id(&body,"installation_id")?;let r=tx.query_opt("SELECT * FROM installations WHERE id=$1 AND state='installed' FOR SHARE",&[&installation]).await?.ok_or_else(Error::not_found)?;
            let artifact=Artifact{image:String::new(),manifest:r.get("manifest"),raw:Vec::new(),image_digest:r.get("image_digest"),manifest_digest:r.get("manifest_digest")};artifact.validate_deployment(&body)?;
            let mut spec=body.clone();spec["enabled"]=json!(false);if spec.get("replicas").is_none(){spec["replicas"]=json!(1);}
            let id=Uuid::new_v4();tx.execute("INSERT INTO deployments(id,owner_id,installation_id,spec,phase,created_at,updated_at) VALUES($1,$2,$3,$4,'disabled',now(),now())",&[&id,&ctx.subject,&installation,&spec]).await?;let op=db::operation(&tx,ctx.subject,name,Some(id.to_string())).await?;db::complete(&tx,op,Ok(json!({"deployment_id":id}))).await?;(db::operation_value(&tx,op).await?,202)
        },
        "approvePlugin"=>{
            if body!=json!({"rules":[],"egress":[]}){return Err(Error::invalid("M1 only approves empty proxy and egress permissions"));}
            let id=path_id(&params,"deployment_id")?;let r=tx.query_one("SELECT spec,revision FROM deployments WHERE id=$1 FOR UPDATE",&[&id]).await?;etag(&headers,r.get("revision"))?;if r.get::<_,Value>("spec")["enabled"]==true{return Err(Error::conflict("Approval changes require a disabled deployment"));}
            tx.execute("UPDATE deployments SET approval=$2,revision=revision+1,updated_at=now() WHERE id=$1",&[&id,&body]).await?;(body.clone(),200)
        },
        "setDeploymentEnabled"=>{
            let id=path_id(&params,"deployment_id")?;let row=tx.query_one("SELECT * FROM deployments WHERE id=$1 FOR UPDATE",&[&id]).await?;etag(&headers,row.get("revision"))?;let enabled=body["enabled"].as_bool().unwrap();let mut spec:Value=row.get("spec");
            if enabled&&row.get::<_,Value>("approval")!=json!({"rules":[],"egress":[]}){return Err(Error::denied());}
            if enabled&&spec["enabled"]!=true{
                if tx.query_one("SELECT count(*) FROM instances WHERE deployment_id=$1 AND phase<>'stopped'",&[&id]).await?.get::<_,i64>(0)>0{return Err(Error::conflict("Previous instances have not drained"));}
                let instance=Uuid::new_v4();let generation=row.get::<_,i64>("revision")+1;
                tx.execute("INSERT INTO instances(id,deployment_id,generation,ordinal,phase,health,limits,created_at,updated_at) VALUES($1,$2,$3,0,'pending','unknown',$4,now(),now())",&[&instance,&id,&generation,&spec["limits"]]).await?;
            }
            spec["enabled"]=json!(enabled);let phase=if enabled{"reconciling"}else{"draining"};
            tx.execute("UPDATE deployments SET spec=$2,phase=$3,revision=revision+1,updated_at=now() WHERE id=$1",&[&id,&spec,&phase]).await?;
            if !enabled{tx.execute("UPDATE provider_records SET availability='unavailable' WHERE instance_id IN(SELECT id FROM instances WHERE deployment_id=$1)",&[&id]).await?;}
            let op=db::operation(&tx,ctx.subject,name,Some(id.to_string())).await?;db::outbox(&tx,"deployment.changed",id,&json!({})).await?;(db::operation_value(&tx,op).await?,202)
        },
        "getInstallation"=>(read_object(&tx,"installations",path_id(&params,"installation_id")?,"Installation").await?,200),
        "getDeployment"=>(read_object(&tx,"deployments",path_id(&params,"deployment_id")?,"Deployment").await?,200),
        "getInstance"=>(read_object(&tx,"instances",path_id(&params,"instance_id")?,"Instance").await?,200),
        "getPublisher"=>(read_object(&tx,"publishers",path_id(&params,"publisher_id")?,"Publisher").await?,200),
        "getPluginApproval"=>{let row=tx.query_one("SELECT approval FROM deployments WHERE id=$1",&[&path_id(&params,"deployment_id")?]).await?;(row.get(0),200)},
        "revokePublisher"=>{let id=path_id(&params,"publisher_id")?;let row=tx.query_one("SELECT revision FROM publishers WHERE id=$1 FOR UPDATE",&[&id]).await?;etag(&headers,row.get(0))?;tx.execute("UPDATE publishers SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1",&[&id]).await?;(Value::Null,204)},
        "revokeSession"=>{let id=path_id(&params,"session_id")?;let row=tx.query_one("SELECT revision FROM credentials WHERE id=$1 AND kind='session' FOR UPDATE",&[&id]).await?;etag(&headers,row.get(0))?;tx.execute("UPDATE credentials SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1",&[&id]).await?;(Value::Null,204)},
        _=>return Err(Error::unavailable("Operation is not implemented in the M1 profile; see crates/kernel/README.md")),
    };
    if mutating{
        db::audit(&tx,ctx.subject,Some(ctx.credential),request_id,name,&json!({"kind":"platform","match":"all"}),"succeeded","COMMITTED").await?;
        let key=key.unwrap();tx.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method=$3 AND path=$4 AND key=$5 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&method,&path,&key]).await?;
        tx.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,response_status,safe_response,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&method,&path,&key,&fingerprint,&(result.1 as i32),&result.0]).await?;
    }
    if let Some(model)=op["responses"][result.1.to_string()]["content"]["application/json"]["schema"]["$ref"].as_str().and_then(|r|r.rsplit('/').next()){validate(model,&result.0)?;}
    tx.commit().await?;Ok(result)
}

async fn authorize_operation(c:&impl GenericClient,ctx:&Context,name:&str,params:&HashMap<String,String>,body:&Value)->Result<()> {
    let(action,resource)=match name{
        "getNode"=>("platform.read",Resource::new("platform","node",None)),
        "trustPublisher"=>("publisher.trust",Resource::new("publisher","new",Some(ctx.subject))),
        "installPlugin"=>("plugin.install",Resource::new("plugin",body["image"].as_str().unwrap_or(""),Some(ctx.subject))),
        "createDeployment"=>("plugin.deploy",Resource::new("deployment","new",Some(ctx.subject))),
        "getPublisher"|"revokePublisher"=>{let id=path_id(params,"publisher_id")?;if c.query_opt("SELECT id FROM publishers WHERE id=$1",&[&id]).await?.is_none(){return Err(Error::not_found());}(if name=="getPublisher"{"publisher.read"}else{"publisher.revoke"},Resource::new("publisher",id,None))},
        "getInstallation"=>{let id=path_id(params,"installation_id")?;let r=c.query_opt("SELECT plugin_id FROM installations WHERE id=$1",&[&id]).await?.ok_or_else(Error::not_found)?;("plugin.read",Resource::new("plugin",r.get::<_,String>(0),None))},
        "getDeployment"|"setDeploymentEnabled"|"approvePlugin"|"getPluginApproval"=>{let id=path_id(params,"deployment_id")?;let r=c.query_opt("SELECT owner_id FROM deployments WHERE id=$1",&[&id]).await?.ok_or_else(Error::not_found)?;(match name{"setDeploymentEnabled"=>"plugin.enable","approvePlugin"=>"plugin.permission.grant",_=>"plugin.read"},Resource::new("deployment",id,Some(r.get(0))))},
        "getInstance"=>{let id=path_id(params,"instance_id")?;let r=c.query_opt("SELECT d.owner_id FROM instances i JOIN deployments d ON d.id=i.deployment_id WHERE i.id=$1",&[&id]).await?.ok_or_else(Error::not_found)?;("plugin.read",Resource::new("instance",id,Some(r.get(0))))},
        "getOperation"=>{let id=path_id(params,"operation_id")?;let r=c.query_opt("SELECT owner_id FROM operations WHERE id=$1",&[&id]).await?.ok_or_else(Error::not_found)?;("operation.read",Resource::new("operation",id,Some(r.get(0))))},
        "revokeSession"=>{let id=path_id(params,"session_id")?;let r=c.query_opt("SELECT owner_id FROM credentials WHERE id=$1 AND kind='session'",&[&id]).await?.ok_or_else(Error::not_found)?;let owner:Uuid=r.get(0);if owner!=ctx.subject{return Err(Error::not_found());}("session.revoke",Resource::new("credential",id,Some(owner)))},
        _=>return Err(Error::unavailable("Operation is not implemented in the M1 profile; see crates/kernel/README.md")),
    };
    auth::authorize(c,ctx,action,&resource).await.map_err(|e|if name.starts_with("get")&&e.code=="PERMISSION_DENIED"{Error::not_found()}else{e})
}

async fn read_object(c:&impl GenericClient,table:&str,id:Uuid,model:&str)->Result<Value>{
    // table is selected exclusively from the static dispatch above.
    let row=c.query_opt(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"),&[&id]).await?.ok_or_else(Error::not_found)?;let raw:Value=row.get(0);let mut result=json!({});
    for key in openapi()["components"]["schemas"][model]["required"].as_array().unwrap(){let key=key.as_str().unwrap();result[key]=raw[key].clone();}
    validate(model,&result)?;Ok(result)
}

#[cfg(test)]mod tests{use super::*;#[test]fn paths_match_only_existing_operations(){assert_eq!(operation("GET","/health/live").unwrap().0["operationId"],"getLiveness");assert!(operation("POST","/health/live").is_none());assert!(operation("GET","/made-up").is_none());assert_eq!(operation("PUT","/api/platform/v1/deployments/x/enabled").unwrap().1["deployment_id"],"x");}}
