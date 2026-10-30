use crate::{api::App,auth::{authorize,Resource},contracts::{digest,id,string,strict_json,validate,validate_business},controller,db,error::{Error,Result},rpc::{self,capability_service_client::CapabilityServiceClient},tls};
use serde_json::{json,Value};
use std::{net::IpAddr,time::Duration};
use uuid::Uuid;

pub async fn invoke(app:&App,token:&str,peer:IpAddr,request_id:Uuid,input:&Value,key:&str)->Result<Value>{
    validate("InvokeRequest",input)?;
    let contract=&input["contract"];let name=string(contract,"name")?;let version=string(contract,"version")?;let schema_digest=string(contract,"schema_digest")?;
    let mut c=app.db.connect().await?;let tx=c.transaction().await?;let ctx=app.auth.authenticate(&tx,token,peer,request_id).await?;
    authorize(&tx,&ctx,"capability.invoke",&Resource::new("capability",format!("{name}@{version}"),None)).await?;
    let fingerprint=digest(&serde_json::to_vec(input)?);let lock=format!("call:{}:{}:{key}",ctx.subject,ctx.credential);
    tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended($1,0))",&[&lock]).await?;
    if let Some(r)=tx.query_opt("SELECT * FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path='/api/platform/v1/calls' AND key=$3 AND expires_at>now() FOR UPDATE",&[&ctx.subject,&ctx.credential,&key]).await?{
        if r.get::<_,String>("fingerprint")!=fingerprint{return Err(Error::new("IDEMPOTENCY_CONFLICT","The key was already used for different input",axum::http::StatusCode::CONFLICT));}
        let call_id=r.get::<_,Option<String>>("resource_id").and_then(|s|s.parse::<Uuid>().ok()).ok_or_else(||Error::conflict("Missing prior call"))?;
        let row=tx.query_one("SELECT state,result FROM calls WHERE id=$1",&[&call_id]).await?;
        if row.get::<_,String>("state")=="succeeded"{return row.get::<_,Option<Value>>("result").ok_or_else(||Error::conflict("Call result is unavailable"));}
        return Err(Error::new("OUTCOME_UNKNOWN","Prior call is not a confirmed success; query its call ID",axum::http::StatusCode::CONFLICT));
    }
    let cap=tx.query_opt("SELECT spec,schema_digest FROM capability_contracts WHERE name=$1 AND version=$2",&[&name,&version]).await?.ok_or_else(Error::not_found)?;
    if cap.get::<_,String>("schema_digest")!=schema_digest{return Err(Error::conflict("Capability digest differs"));}let definition:Value=cap.get("spec");
    if definition["side_effect"]!="none"{return Err(Error::invalid("Side-effecting capabilities are not enabled in M1"));}
    let bytes=serde_json::to_vec(&input["input"])?;
    if bytes.len()>definition["maximum_input_bytes"].as_u64().unwrap() as usize{return Err(Error::invalid("Capability input exceeds its byte limit"));}
    validate_business(&definition["input_schema"],&input["input"])?;
    let fixed=input.get("provider_deployment_id").map(|_|id(input,"provider_deployment_id")).transpose()?;
    let providers=tx.query("SELECT i.id,i.generation,d.id AS deployment_id,d.owner_id,n.manifest FROM provider_records p JOIN instances i ON i.id=p.instance_id JOIN deployments d ON d.id=i.deployment_id JOIN installations n ON n.id=d.installation_id WHERE p.name=$1 AND p.version=$2 AND p.availability='ready' AND p.observed_at>now()-interval '15 seconds' AND i.phase='ready' AND d.spec->>'enabled'='true' AND d.archived_at IS NULL AND ($3::uuid IS NULL OR d.id=$3) ORDER BY i.id FOR UPDATE OF i",&[&name,&version,&fixed]).await?;
    let row=providers.first().ok_or_else(||Error::unavailable("No healthy fixed provider is available"))?;let instance:Uuid=row.get("id");let generation:i64=row.get("generation");let manifest:Value=row.get("manifest");
    if manifest["business_entry"]=="skill_required"{return Err(Error::denied());}
    let active:i64=tx.query_one("SELECT count(*) FROM calls WHERE provider_instance_id=$1 AND state IN ('accepted','running')",&[&instance]).await?.get(0);
    let max=manifest["concurrency"]["max_active_requests"].as_i64().unwrap_or(1);
    if active>=max{return Err(Error::exhausted());}
    let quota=tx.query_one("SELECT * FROM quotas WHERE account_id=$1 FOR UPDATE",&[&ctx.subject]).await?;let q:Value=quota.get("spec");
    let today=chrono::Utc::now().date_naive();let daily=if quota.get::<_,chrono::NaiveDate>("daily_bucket")==today{quota.get::<_,i64>("daily_calls_used")}else{0};
    if quota.get::<_,i64>("concurrent_calls_used")>=q["maximum_concurrent_calls"].as_i64().unwrap()||daily>=q["daily_calls"].as_i64().unwrap(){return Err(Error::exhausted());}
    let call=Uuid::new_v4();let timeout=input["timeout_ms"].as_u64().unwrap_or(30000);let deadline=chrono::Utc::now()+chrono::Duration::milliseconds(timeout as i64);
    tx.execute("INSERT INTO calls(id,owner_id,credential_id,provider_instance_id,contract,input_digest,idempotency_key,state,deadline_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'running',$8,now())",&[&call,&ctx.subject,&ctx.credential,&instance,&contract,&digest(&bytes),&key,&deadline]).await?;
    tx.execute("UPDATE quotas SET concurrent_calls_used=concurrent_calls_used+1,daily_calls_used=$2+1,daily_bucket=$3 WHERE account_id=$1",&[&ctx.subject,&daily,&today]).await?;
    tx.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='POST' AND path='/api/platform/v1/calls' AND key=$3 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&key]).await?;
    tx.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,resource_id,expires_at,created_at) VALUES($1,$2,'POST','/api/platform/v1/calls',$3,$4,$5,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&key,&fingerprint,&call.to_string()]).await?;
    tx.commit().await?;
    let result=async{
        let address=app.runtime.address(instance)?;let channel=tls::channel(&app.config,instance,generation,&address).await?;let mut client=CapabilityServiceClient::new(channel).max_decoding_message_size(16778240);
        let mut context=controller::context(Some(ctx.subject));context.deadline_unix_ms=deadline.timestamp_millis();context.idempotency_key=key.to_owned();
        let mut request=tonic::Request::new(rpc::InvokeRequest{context:Some(context),call_id:call.to_string(),contract:Some(rpc::ContractRef{name:name.to_owned(),version:version.to_owned(),schema_digest:schema_digest.to_owned()}),input_digest:digest(&bytes),input_json:bytes,binding_id:String::new()});request.set_timeout(Duration::from_millis(timeout));
        let response=client.invoke(request).await?.into_inner();
        if response.call_id!=call.to_string()||digest(&response.output_json)!=response.output_digest{return Err(Error::conflict("Plugin returned a different call or output digest"));}
        let output=strict_json(&response.output_json,definition["maximum_output_bytes"].as_u64().unwrap() as usize)?;validate_business(&definition["output_schema"],&output)?;
        let out=json!({"call_id":call,"contract":contract,"provider_instance_id":instance,"output":output,"completed_at":chrono::Utc::now()});validate("CallResult",&out)?;Ok(out)
    };
    let mut result=tokio::time::timeout(Duration::from_millis(timeout),result).await.unwrap_or_else(|_|Err(Error::new("DEADLINE_EXCEEDED","Call deadline elapsed; inspect the call record",axum::http::StatusCode::GATEWAY_TIMEOUT)));
    let mut c=app.db.connect().await?;let tx=c.transaction().await?;
    let check=async{let current=app.auth.authenticate(&tx,token,peer,request_id).await?;authorize(&tx,&current,"capability.invoke",&Resource::new("capability",format!("{name}@{version}"),None)).await};
    if let Err(e)=check.await{result=Err(e);}
    let(state,value,error)=match &result{Ok(v)=>("succeeded",Some(v.clone()),None),Err(e)=>("unknown",None,Some(e.detail()))};
    tx.execute("UPDATE calls SET state=$2,result=$3,error=$4,completed_at=CASE WHEN $2='succeeded' THEN now() ELSE NULL END WHERE id=$1 AND state='running'",&[&call,&state,&value,&error]).await?;
    tx.execute("UPDATE quotas SET concurrent_calls_used=greatest(0,concurrent_calls_used-1) WHERE account_id=$1",&[&ctx.subject]).await?;
    db::audit(&tx,ctx.subject,Some(ctx.credential),request_id,"capability.invoke",&Resource::new("call",call,Some(ctx.subject)).json(),if result.is_ok(){"succeeded"}else{"failed"},if result.is_ok(){"CALL_COMPLETED"}else{"OUTCOME_UNKNOWN"}).await?;
    tx.commit().await?;result
}
