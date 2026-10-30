use clap::{Parser,Subcommand};
use serde_json::{json,Value};
use std::{io::{self,Read},path::{Path,PathBuf},time::Duration};
use xelab_kernel::{auth,config::{Config,read_private,require_host_admin},contracts::{openapi,strict_json},db::{Db,LEADER_LOCK},error::{Error,Result}};

#[derive(Parser)]struct Args{#[command(subcommand)]command:Cmd}
#[derive(Subcommand)]enum Cmd{
    ValidateConfig{#[arg(long)]config:PathBuf},
    Migrate{#[arg(long)]config:PathBuf,#[arg(long)]target:u64},
    Bootstrap{#[arg(long)]config:PathBuf,#[arg(long)]owner_name:String},
    Api{operation_id:String,#[arg(long)]endpoint:String,#[arg(long)]token_file:Option<PathBuf>,#[arg(long)]ca_file:Option<PathBuf>,#[arg(long)]body_file:Option<String>,#[arg(long="path")]path:Vec<String>,#[arg(long="query")]query:Vec<String>,#[arg(long)]if_match:Option<String>,#[arg(long)]idempotency_key:Option<String>,#[arg(long,default_value_t=60000)]timeout_ms:u64},
}
#[tokio::main]async fn main(){if let Err(e)=run(Args::parse()).await{eprintln!("{e}");std::process::exit(match e.status.as_u16(){400|413|415=>2,401=>3,403=>4,409|412|428=>5,504=>6,429|503=>7,_=>8});}}
fn read_body(path:&str,limit:usize)->Result<Vec<u8>>{let mut bytes=Vec::new();if path=="-"{io::stdin().take(limit as u64+1).read_to_end(&mut bytes)?;}else{std::fs::File::open(path)?.take(limit as u64+1).read_to_end(&mut bytes)?;}if bytes.len()>limit{return Err(Error::invalid("Input exceeds its limit"));}Ok(bytes)}
async fn run(args:Args)->Result<()> {
    match args.command{
        Cmd::ValidateConfig{config}=>{Config::load(&config)?;println!("{}",json!({"configuration":"valid","runtime_checked":false}));},
        Cmd::Migrate{config,target}=>{require_host_admin()?;if target!=1{return Err(Error::invalid("Only explicit baseline migration target 1 is supported"));}let cfg=Config::load(&config)?;let(dsn,_)=cfg.secrets()?;Db::new(dsn)?.migrate().await?;println!("{}",json!({"migration":1,"status":"applied_or_verified"}));},
        Cmd::Bootstrap{config,owner_name}=>{require_host_admin()?;let cfg=Config::load(&config)?;let(dsn,_)=cfg.secrets()?;let db=Db::new(dsn)?;db.check().await?;let password=String::from_utf8(read_body("-",1024)?).map_err(|_|Error::invalid("Password must be UTF-8"))?;let mut c=db.connect().await?;let tx=c.transaction().await?;tx.query_one("SELECT pg_advisory_xact_lock($1)",&[&LEADER_LOCK]).await?;let id=auth::bootstrap(&tx,&owner_name,password.trim_end_matches(['\r','\n']),&cfg.value["quota_defaults"]).await?;tx.commit().await?;println!("{}",json!({"owner_id":id}));},
        Cmd::Api{operation_id,endpoint,token_file,ca_file,body_file,path,query,if_match,idempotency_key,timeout_ms}=>{
            let mut selected=None;for(p,methods)in openapi()["paths"].as_object().unwrap(){for(m,op)in methods.as_object().unwrap(){if op["operationId"]==operation_id{selected=Some((p.clone(),m.clone(),op));}}}let(mut route,method,op)=selected.ok_or_else(||Error::invalid("Unknown operationId"))?;
            for item in path{let(k,v)=item.split_once('=').ok_or_else(||Error::invalid("Path arguments require NAME=VALUE"))?;if v.contains(['/', '?','#','%']){return Err(Error::invalid("Invalid path argument"));}route=route.replace(&format!("{{{k}}}"),v);}
            if route.contains('{'){return Err(Error::invalid("Missing path argument"));}
            let mut url=xelab_kernel::config::checked_origin(&endpoint)?;url.set_path(&route);for item in query{let(k,v)=item.split_once('=').ok_or_else(||Error::invalid("Query arguments require NAME=VALUE"))?;url.query_pairs_mut().append_pair(k,v);}
            let mut builder=reqwest::Client::builder().timeout(Duration::from_millis(timeout_ms)).redirect(reqwest::redirect::Policy::none());if let Some(path)=ca_file{builder=builder.add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(path)?).map_err(|_|Error::invalid("Invalid CA certificate"))?);}
            let client=builder.build().map_err(|_|Error::unavailable("Cannot initialize HTTP client"))?;let mut request=client.request(method.to_ascii_uppercase().parse().map_err(|_|Error::invalid("Invalid operation method"))?,url);
            if let Some(path)=token_file{let token=String::from_utf8(read_private(Path::new(&path),1024)?).map_err(|_|Error::invalid("Invalid token file"))?;request=request.bearer_auth(token.trim());}
            if let Some(path)=body_file{let bytes=read_body(&path,4194304)?;strict_json(&bytes,4194304)?;request=request.header("content-type","application/json").body(bytes);}
            if let Some(value)=if_match{request=request.header("if-match",value);}
            if op["parameters"].as_array().unwrap().iter().any(|p|p["name"]=="Idempotency-Key"){request=request.header("idempotency-key",idempotency_key.unwrap_or_else(||uuid::Uuid::new_v4().to_string()));}
            let response=request.send().await.map_err(|_|Error::unavailable("HTTP request failed; operation outcome may need inspection"))?;let status=response.status();if response.content_length().is_some_and(|n|n>16777216){return Err(Error::invalid("HTTP response exceeds its limit"));}let body=response.bytes().await.map_err(|_|Error::unavailable("HTTP response was interrupted"))?;if body.len()>16777216{return Err(Error::invalid("HTTP response exceeds its limit"));}
            if !body.is_empty(){let value:Value=serde_json::from_slice(&body)?;println!("{}",serde_json::to_string_pretty(&value)?);}
            if !status.is_success(){return Err(Error::new("HTTP_ERROR","Server rejected the operation",axum::http::StatusCode::from_u16(status.as_u16()).unwrap()));}
        },
    }Ok(())
}

