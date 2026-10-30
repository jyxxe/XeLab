use crate::{contracts::{digest,string,validate},db,error::{Error,Result}};
use argon2::{Argon2,PasswordHash,PasswordHasher,PasswordVerifier,password_hash::SaltString};
use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime,Utc};
use hmac::{Hmac,Mac};
use rand::{RngCore,rngs::OsRng};
use serde_json::{json,Value};
use sha2::Sha256;
use std::{collections::{HashMap,HashSet},net::IpAddr,sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio_postgres::GenericClient;
use uuid::Uuid;

#[derive(Clone)]pub struct Auth{key:Arc<Vec<u8>>,key_id:Arc<String>,failures:Arc<Mutex<HashMap<String,(Instant,u32)>>>}
// Never deserialize a security context from an HTTP body or RPC message.
#[derive(Clone)]pub struct Context{pub(crate)subject:Uuid,pub(crate)credential:Uuid,pub(crate)peer:IpAddr,pub(crate)request:Uuid,pub(crate)chain:Vec<Uuid>}
#[derive(Clone)]pub struct Resource{pub kind:String,pub id:String,pub owner:Option<Uuid>}

impl Resource{pub fn new(kind:&str,id:impl ToString,owner:Option<Uuid>)->Self{Self{kind:kind.into(),id:id.to_string(),owner}}pub fn json(&self)->Value{json!({"kind":self.kind,"match":"ids","ids":[self.id]})}}

impl Auth{
    pub fn new(key:Vec<u8>)->Result<Self>{if key.len()<32{return Err(Error::invalid("HMAC key is too short"));}let key_id=digest(&key);Ok(Self{key:Arc::new(key),key_id:Arc::new(key_id),failures:Default::default()})}
    pub fn token_digest(&self,token:&str)->Vec<u8>{let mut mac=Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key length");mac.update(token.as_bytes());mac.finalize().into_bytes().to_vec()}
    fn random_token(&self)->(String,Vec<u8>){let mut raw=[0u8;32];OsRng.fill_bytes(&mut raw);let token=URL_SAFE_NO_PAD.encode(raw);let hash=self.token_digest(&token);(token,hash)}
    pub async fn authenticate(&self,c:&impl GenericClient,token:&str,peer:IpAddr,request:Uuid)->Result<Context>{
        if token.len()>512||token.len()<32{return Err(Error::credential());}
        let hash=self.token_digest(token);
        let r=c.query_opt("SELECT * FROM credentials WHERE hmac_key_id=$1 AND verification_digest=$2 FOR SHARE",&[&self.key_id.as_str(),&hash]).await?.ok_or_else(Error::credential)?;
        let stored:Vec<u8>=r.get("verification_digest");let mut mac=Hmac::<Sha256>::new_from_slice(&self.key).unwrap();mac.update(token.as_bytes());mac.verify_slice(&stored).map_err(|_|Error::credential())?;
        let subject:Uuid=r.get("owner_id");let credential:Uuid=r.get("id");let mut chain=Vec::new();let mut cursor=Some(credential);let mut seen=HashSet::new();
        while let Some(id)=cursor{
            if chain.len()>=64||!seen.insert(id){return Err(Error::credential());}
            let r=c.query_opt("SELECT * FROM credentials WHERE id=$1 FOR SHARE",&[&id]).await?.ok_or_else(Error::credential)?;
            if r.get::<_,String>("status")!="active"||r.get::<_,DateTime<Utc>>("expires_at")<=Utc::now(){return Err(Error::credential());}
            let conditions:Value=r.get("conditions");if !conditions_match(&conditions,peer,Utc::now()){return Err(Error::credential());}
            account_chain(c,r.get("owner_id")).await?;
            cursor=r.get("issuer_credential_id");chain.push(id);
        }
        Ok(Context{subject,credential,peer,request,chain})
    }
    pub fn check_login_budget(&self,account:Uuid,peer:IpAddr)->Result<()> {
        let mut state=self.failures.lock().map_err(|_|Error::unavailable("Login limiter unavailable"))?;
        state.retain(|_,(at,_)|at.elapsed()<Duration::from_secs(60));
        let keys=[format!("account:{account}"),format!("peer:{peer}")];
        if state.len()>8190||keys.iter().any(|key|state.get(key).is_some_and(|(_,n)|*n>=5)){return Err(Error::new("RATE_LIMITED","Login attempt limit exceeded",axum::http::StatusCode::TOO_MANY_REQUESTS));}
        for key in keys{state.entry(key).or_insert((Instant::now(),0)).1+=1;}Ok(())
    }
    pub async fn login(&self,c:&impl GenericClient,account:Uuid,password:String,peer:IpAddr,request:Uuid)->Result<Value>{
        self.check_login_budget(account,peer)?;
        let r=c.query_opt("SELECT * FROM accounts WHERE id=$1 FOR SHARE",&[&account]).await?;
        let real_hash=r.as_ref().and_then(|r|r.get::<_,Option<String>>("password_hash"));
        let password_ok=tokio::task::spawn_blocking(move||{
            if let Some(hash)=real_hash {PasswordHash::new(&hash).ok().is_some_and(|hash|Argon2::default().verify_password(password.as_bytes(),&hash).is_ok())}
            else{let _=password_hash(&password);false}
        }).await.map_err(|_|Error::unavailable("Password verifier unavailable"))?;
        let active=r.as_ref().is_some_and(|r|r.get::<_,String>("kind")=="user"&&r.get::<_,String>("status")=="active");
        if !password_ok||!active||account_chain(c,account).await.is_err(){
            let subject=r.as_ref().map(|_|account);
            c.execute("INSERT INTO audit_events(id,subject_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,'login',$4,'denied',1,'INVALID_CREDENTIAL',now(),now())",&[&Uuid::new_v4(),&subject,&request,&json!({"kind":"account","match":"ids","ids":[account]})]).await?;
            return Err(Error::credential());
        }
        let (token,hash)=self.random_token();let id=Uuid::new_v4();let now=Utc::now();let expires=now+chrono::Duration::hours(1);
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,client_id,status,conditions,expires_at,last_used_at,created_at,updated_at) VALUES($1,$2,'session',$3,$4,'xelabctl','active','{}',$5,$6,$6,$6)",&[&id,&account,&self.key_id.as_str(),&hash,&expires,&now]).await?;
        db::audit(c,account,Some(id),request,"login",&Resource::new("account",account,Some(account)).json(),"succeeded","SESSION_ISSUED").await?;
        let session=json!({"id":id,"revision":1,"created_at":now,"updated_at":now,"account_id":account,"client_id":"xelabctl","expires_at":expires,"last_seen_at":now,"status":"active"});
        let v=json!({"session":session,"access_token":token,"token_type":"Bearer","expires_in":3600});validate("SessionIssued",&v)?;Ok(v)
    }
}

pub fn password_hash(password:&str)->Result<String>{
    if !(12..=128).contains(&password.chars().count()){return Err(Error::invalid("Passwords require 12 to 128 characters"));}
    Argon2::default().hash_password(password.as_bytes(),&SaltString::generate(&mut OsRng)).map(|h|h.to_string()).map_err(|_|Error::unavailable("Password hashing failed"))
}

async fn account_chain(c:&impl GenericClient,subject:Uuid)->Result<Vec<Uuid>>{
    let mut ids=Vec::new();let mut next=Some(subject);
    while let Some(id)=next{
        if ids.len()>=8||ids.contains(&id){return Err(Error::credential());}
        let r=c.query_opt("SELECT parent_id,status,expires_at FROM accounts WHERE id=$1 FOR SHARE",&[&id]).await?.ok_or_else(Error::credential)?;
        if r.get::<_,String>("status")!="active"||r.get::<_,Option<DateTime<Utc>>>("expires_at").is_some_and(|d|d<=Utc::now()){return Err(Error::credential());}
        ids.push(id);next=r.get("parent_id");
    }ids.reverse();Ok(ids)
}

pub fn conditions_match(v:&Value,peer:IpAddr,now:DateTime<Utc>)->bool{
    if !v.is_object(){return false;}
    for (k,after) in [("not_before",true),("expires_at",false)]{if let Some(raw)=v.get(k){let Some(t)=raw.as_str().and_then(|s|DateTime::parse_from_rfc3339(s).ok()) else{return false;};if if after{now<t}else{now>=t}{return false;}}}
    if let Some(a)=v.get("source_cidrs"){if !a.as_array().is_some_and(|a|!a.is_empty()&&a.iter().any(|s|s.as_str().and_then(|s|s.parse::<ipnet::IpNet>().ok()).is_some_and(|n|n.contains(&peer)))){return false;}}
    if let Some(a)=v.get("audiences"){if !a.as_array().is_some_and(|a|a.iter().any(|v|v=="xelab.platform")){return false;}}
    v.as_object().unwrap().keys().all(|k|matches!(k.as_str(),"not_before"|"expires_at"|"source_cidrs"|"audiences"))
}
pub fn selector_matches(v:&Value,r:&Resource)->bool{
    if v["kind"]!=r.kind{return false;}
    match v["match"].as_str(){Some("all")=>true,Some("ids")=>v["ids"].as_array().is_some_and(|a|a.iter().any(|x|x==&r.id)),Some("owned_by")=>r.owner.is_some_and(|o|v["owner_id"]==o.to_string()),_=>false}
}
pub fn rules_allow(rules:&Value,action:&str,resource:&Resource,peer:IpAddr)->bool{
    rules.as_array().is_some_and(|a|a.iter().any(|r|r["action"]==action&&selector_matches(&r["resource"],resource)&&r.get("conditions").is_none_or(|c|conditions_match(c,peer,Utc::now()))))
}

async fn account_allows(c:&impl GenericClient,subject:Uuid,action:&str,res:&Resource,peer:IpAddr)->Result<bool>{
    let chain=account_chain(c,subject).await?;let mut effective=false;
    for (level,id) in chain.iter().enumerate(){
        let grants=c.query("SELECT g.scope,g.role_id,g.group_id,g.group_version,g.issuer_credential_id FROM grants g WHERE g.subject_id=$1 AND g.status='active' AND (g.expires_at IS NULL OR g.expires_at>now()) FOR SHARE",&[id]).await?;
        let mut local=false;
        for g in grants{
            if g.get::<_,Option<Uuid>>("issuer_credential_id").is_some(){continue;} // delegated grant issuance is not exposed by this M1 profile
            let scope:Value=g.get("scope");if !selector_matches(&scope,res){continue;}
            if let Some(role)=g.get::<_,Option<String>>("role_id"){
                let r=c.query_one("SELECT definition FROM role_definitions WHERE id=$1 FOR SHARE",&[&role]).await?;let d:Value=r.get(0);
                local|=d["actions"].as_array().is_some_and(|a|a.iter().any(|v|v==action));
            }else if let Some(group)=g.get::<_,Option<Uuid>>("group_id"){
                let version:i64=g.get("group_version");
                if let Some(r)=c.query_opt("SELECT v.rules FROM group_versions v JOIN permission_groups g ON g.id=v.group_id WHERE v.group_id=$1 AND v.version=$2 AND g.status='active' FOR SHARE OF g,v",&[&group,&version]).await?{local|=rules_allow(&r.get::<_,Value>(0),action,res,peer);}
            }
        }
        if level==0{effective=local;}else{
            let r=c.query_opt("SELECT * FROM inheritance_grants WHERE account_id=$1 FOR SHARE",&[id]).await?.ok_or_else(Error::credential)?;
            if r.get::<_,Uuid>("parent_id")!=chain[level-1]||r.get::<_,Option<Uuid>>("issuer_credential_id").is_some(){return Ok(false);}
            let mode:String=r.get("mode");
            let ceiling=mode=="follow_parent"||r.get::<_,Option<Value>>("maximum_rules").is_some_and(|rules|rules_allow(&rules,action,res,peer));
            effective=effective&&ceiling;
        }
    }Ok(effective)
}

pub async fn authorize(c:&impl GenericClient,ctx:&Context,action:&str,res:&Resource)->Result<()> {
    let registered=c.query_opt("SELECT resource_kinds FROM permission_definitions WHERE action=$1",&[&action]).await?;
    let kind_ok=registered.is_some_and(|r|r.get::<_,Value>(0).as_array().is_some_and(|a|a.iter().any(|v|v==&res.kind)));
    let mut allowed=kind_ok&&account_allows(c,ctx.subject,action,res,ctx.peer).await?;
    for id in &ctx.chain{
        let row=c.query_one("SELECT * FROM credentials WHERE id=$1 FOR SHARE",&[id]).await?;
        allowed &= row.get::<_,String>("status")=="active"&&row.get::<_,DateTime<Utc>>("expires_at")>Utc::now();
        allowed &= account_allows(c,row.get("owner_id"),action,res,ctx.peer).await?;
        if row.get::<_,String>("kind")!="session"{
            match row.get::<_,Option<String>>("inheritance_mode").as_deref(){
                Some("follow_parent")=>(),
                Some("selected"|"all_snapshot")=>allowed &= row.get::<_,Option<Value>>("maximum_rules").is_some_and(|r|rules_allow(&r,action,res,ctx.peer)),
                _=>allowed=false,
            }
        }
    }
    if allowed{Ok(())}else{Err(Error::denied())}
}

pub async fn bootstrap(c:&impl GenericClient,name:&str,password:&str,quota:&Value)->Result<Uuid>{
    validate("QuotaSpec",quota)?;
    if name.is_empty()||name.chars().count()>80{return Err(Error::invalid("Invalid owner name"));}
    let hash=password_hash(password)?;
    let r=c.query_one("SELECT count(*) FROM accounts",&[]).await?;if r.get::<_,i64>(0)!=0{return Err(Error::conflict("Bootstrap is only available for an empty database"));}
    let defs:Value=serde_json::from_str(include_str!("../../../docs/contracts/permissions.json"))?;
    let mut actions=Vec::new();let mut kinds=HashSet::new();
    for (action,spec) in defs["definitions"].as_object().unwrap(){
        c.execute("INSERT INTO permission_definitions(action,resource_kinds,description,self_service) VALUES($1,$2,$3,$4)",&[&action,&spec["resource_kinds"],&string(spec,"description")?,&spec["self_service"].as_bool().unwrap_or(false)]).await?;
        actions.push(action);for kind in spec["resource_kinds"].as_array().unwrap(){kinds.insert(kind.as_str().unwrap());}
    }
    c.execute("INSERT INTO role_definitions(id,definition,version) VALUES('Owner',$1,1)",&[&json!({"actions":actions})]).await?;
    let id=Uuid::new_v4();c.execute("INSERT INTO accounts(id,kind,status,display_name,independent_owner,password_hash,created_at,updated_at) VALUES($1,'user','active',$2,true,$3,now(),now())",&[&id,&name,&hash]).await?;
    c.execute("INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",&[&id]).await?;
    c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) VALUES($1,$2,current_date,now(),now())",&[&id,&quota]).await?;
    for kind in kinds{c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$2,'Owner',$3,'active',now(),now())",&[&Uuid::new_v4(),&id,&json!({"kind":kind,"match":"all"})]).await?;}
    db::audit(c,id,None,Uuid::new_v4(),"bootstrap",&Resource::new("account",id,Some(id)).json(),"succeeded","OWNER_CREATED").await?;Ok(id)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn permissions_need_action_object_and_conditions(){let id=Uuid::new_v4();let r=Resource::new("deployment","d1",Some(id));let p="127.0.0.1".parse().unwrap();let rules=json!([{"action":"plugin.enable","resource":{"kind":"deployment","match":"owned_by","owner_id":id},"conditions":{"source_cidrs":["127.0.0.0/8"]}}]);assert!(rules_allow(&rules,"plugin.enable",&r,p));assert!(!rules_allow(&rules,"plugin.install",&r,p));assert!(!rules_allow(&rules,"plugin.enable",&Resource::new("deployment","d1",Some(Uuid::new_v4())),p));assert!(!rules_allow(&rules,"plugin.enable",&r,"192.0.2.1".parse().unwrap()));}
    #[test]fn credentials_are_random_and_hash_only(){let a=Auth::new(vec![7;32]).unwrap();let(x,h)=a.random_token();let(y,_)=a.random_token();assert_ne!(x,y);assert_eq!(h,a.token_digest(&x));assert_ne!(h,x.as_bytes());assert_eq!(h.len(),32);}
    #[test]fn login_limits_both_account_and_peer(){let a=Auth::new(vec![9;32]).unwrap();let id=Uuid::new_v4();let p="127.0.0.1".parse().unwrap();for _ in 0..5{a.check_login_budget(id,p).unwrap();}assert!(a.check_login_budget(Uuid::new_v4(),p).is_err());assert!(a.check_login_budget(id,"192.0.2.1".parse().unwrap()).is_err());}
}
