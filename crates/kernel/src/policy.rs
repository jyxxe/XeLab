//! Bounded, uncached policy evaluation. Sources in one path are conjunctive;
//! separate paths are alternatives. Snapshots freeze rules AND source paths.
use crate::{auth::{conditions_match, rules_allow, Context, Resource}, contracts::{id, string, validate}, error::{Error, Result}};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::{BTreeSet, HashSet}, net::IpAddr};
use tokio_postgres::GenericClient;
use uuid::Uuid;

const MAX_PATHS: usize = 4096;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Path {
    pub rule: Value,
    pub sources: BTreeSet<Uuid>,
}
pub struct Policy<'a, C> {
    c: &'a C,
    peer: IpAddr,
    evaluating_grants: HashSet<Uuid>,
}
impl<'a, C: GenericClient + Sync> Policy<'a, C> {
    pub fn new(c: &'a C, peer: IpAddr) -> Self { Self { c, peer, evaluating_grants: HashSet::new() } }

    pub async fn account(&mut self, account: Uuid, depth: usize) -> Result<Vec<Path>> {
        if depth > 64 { return Err(Error::denied()); }
        let chain = crate::auth::account_chain(self.c, account).await?;
        let mut paths = Vec::new();
        for (level, subject) in chain.iter().enumerate() {
            if level == 0 {
                for row in self.c.query("SELECT id FROM grants WHERE subject_id=$1 AND status='active' ORDER BY id", &[subject]).await? {
                    paths.extend(Box::pin(self.grant(row.get(0), depth + 1)).await?);
                    bounded(&paths)?;
                }
            } else {
                let row = self.c.query_opt("SELECT * FROM inheritance_grants WHERE account_id=$1", &[subject]).await?.ok_or_else(Error::credential)?;
                if row.get::<_, Uuid>("parent_id") != chain[level-1] { return Err(Error::credential()); }
                if let Some(issuer) = row.get::<_, Option<Uuid>>("issuer_credential_id") {
                    let limits = Box::pin(self.credential(issuer, depth+1)).await?;
                    paths = self.intersection(&paths, &limits).await?;
                }
                if row.get::<_, String>("mode") != "follow_parent" {
                    paths = self.freeze(paths, row.get::<_, Option<Value>>("maximum_rules").as_ref().ok_or_else(Error::denied)?, Some(*subject), None).await?;
                }
            }
        }
        // Identity self-service is a fixed system source, scoped to this user.
        // It is added after business inheritance, but never after credential limits.
        let row = self.c.query_one("SELECT kind FROM accounts WHERE id=$1", &[&account]).await?;
        if row.get::<_, String>(0) == "user" { paths.extend(self.self_service(account).await?); }
        bounded(&paths)?;
        Ok(paths)
    }

    async fn self_service(&self, account: Uuid) -> Result<Vec<Path>> {
        let descendants: Vec<Value> = self.c.query("SELECT descendant_id FROM account_closure WHERE ancestor_id=$1 ORDER BY depth,descendant_id", &[&account]).await?.iter().map(|r| json!(r.get::<_,Uuid>(0))).collect();
        let mut paths=Vec::new();
        for r in self.c.query("SELECT action,resource_kinds FROM permission_definitions WHERE self_service ORDER BY action", &[]).await? {
            let action:String=r.get(0);
            let kinds:Value=r.get(1);
            for kind in kinds.as_array().ok_or_else(Error::denied)? {
                if kind == "account" {
                    let ids=if matches!(action.as_str(),"account.child.create"|"account.child.manage"|"account.permissions.read"|"credential.api-key.create"|"credential.api-key.read") { descendants.clone() } else { vec![json!(account)] };
                    for chunk in ids.chunks(256) { paths.push(Path{rule:json!({"action":action,"resource":{"kind":kind,"match":"ids","ids":chunk}}),sources:BTreeSet::new()}); }
                } else {
                    paths.push(Path{rule:json!({"action":action,"resource":{"kind":kind,"match":"owned_by","owner_id":account}}),sources:BTreeSet::new()});
                }
            }
        }
        Ok(paths)
    }

    pub async fn credential(&mut self, credential: Uuid, depth: usize) -> Result<Vec<Path>> {
        if depth>64 { return Err(Error::denied()); }
        let row=self.c.query_opt("SELECT * FROM credentials WHERE id=$1", &[&credential]).await?.ok_or_else(Error::credential)?;
        if row.get::<_,String>("status")!="active" || row.get::<_,DateTime<Utc>>("expires_at")<=Utc::now() || !conditions_match(&row.get::<_,Value>("conditions"),self.peer,Utc::now()) {return Err(Error::credential());}
        let mut paths=Box::pin(self.account(row.get("owner_id"), depth+1)).await?;
        if let Some(issuer)=row.get::<_,Option<Uuid>>("issuer_credential_id") {
            let limits=Box::pin(self.credential(issuer,depth+1)).await?;
            paths=self.intersection(&paths,&limits).await?;
        }
        match row.get::<_,String>("kind").as_str() {
            "session"=>{},
            "api_key"=>match row.get::<_,Option<String>>("inheritance_mode").as_deref() {
                Some("follow_parent")=>{},
                Some("selected"|"all_snapshot")=> paths=self.freeze(paths,row.get::<_,Option<Value>>("maximum_rules").as_ref().ok_or_else(Error::denied)?,None,Some(credential)).await?,
                _=>return Err(Error::credential()),
            },
            _=>return Err(Error::credential()),
        }
        // Carry non-action restrictions into newly issued snapshots/templates too.
        let mut conditions:Value=row.get("conditions");
        conditions.as_object_mut().ok_or_else(Error::credential)?.remove("max_calls");
        for path in &mut paths {
            let Some(condition)=intersect_conditions(path.rule.get("conditions").unwrap_or(&json!({})),&conditions) else {return Ok(Vec::new());};
            path.rule["conditions"]=condition;
        }
        Ok(paths)
    }

    async fn grant(&mut self, grant: Uuid, depth: usize) -> Result<Vec<Path>> {
        // A grant cannot prove its own issuing authority. Other independent paths
        // remain available while evaluating a delegated grant on the same subject.
        if !self.evaluating_grants.insert(grant) { return Ok(Vec::new()); }
        let result=self.grant_inner(grant,depth).await;
        self.evaluating_grants.remove(&grant);
        result
    }
    async fn grant_inner(&mut self, grant: Uuid, depth: usize) -> Result<Vec<Path>> {
        let Some(row)=self.c.query_opt("SELECT * FROM grants WHERE id=$1 AND status='active' AND (expires_at IS NULL OR expires_at>now())", &[&grant]).await? else {return Ok(Vec::new());};
        let scope:Value=row.get("scope");
        let mut rules=Vec::new();
        if let Some(role)=row.get::<_,Option<String>>("role_id") {
            let def:Value=self.c.query_one("SELECT definition FROM role_definitions WHERE id=$1", &[&role]).await?.get(0);
            let kind=string(&scope,"kind")?;
            let supported=self.c.query("SELECT action FROM permission_definitions WHERE resource_kinds ? $1", &[&kind]).await?;
            for row in supported {
                let action:String=row.get(0);
                if def["actions"].as_array().ok_or_else(Error::denied)?.iter().any(|a|a==&action) {rules.push(json!({"action":action,"resource":scope}));}
            }
        } else {
            let group:Uuid=row.get("group_id");let version:i64=row.get("group_version");
            let Some(g)=self.c.query_opt("SELECT v.rules FROM group_versions v JOIN permission_groups g ON g.id=v.group_id WHERE v.group_id=$1 AND v.version=$2 AND g.status='active'", &[&group,&version]).await? else {return Ok(Vec::new());};
            for r in g.get::<_,Value>(0).as_array().ok_or_else(Error::denied)? {
                if let Some(resource)=self.selector_intersection(&r["resource"],&scope).await? {let mut rule=r.clone();rule["resource"]=resource;rules.push(rule);}
            }
        }
        let expiry:Option<DateTime<Utc>>=row.get("expires_at");
        let mut paths=Vec::new();
        for mut rule in rules {
            if let Some(expiry)=expiry {
                let Some(conditions)=intersect_conditions(rule.get("conditions").unwrap_or(&json!({})),&json!({"expires_at":expiry})) else {continue;};
                rule["conditions"]=conditions;
            }
            paths.push(Path{rule,sources:BTreeSet::from([grant])});
        }
        if let Some(issuer)=row.get::<_,Option<Uuid>>("issuer_credential_id") {
            // Invalid delegated source withdraws this alternative, not all grants.
            match Box::pin(self.credential(issuer,depth+1)).await {
                Ok(limits)=>paths=self.intersection(&paths,&limits).await?,
                Err(e) if matches!(e.code,"INVALID_CREDENTIAL"|"PERMISSION_DENIED")=>return Ok(Vec::new()),
                Err(e)=>return Err(e),
            }
        } else {
            let issuer:Uuid=row.get("issuer_id");
            let subject:Uuid=row.get("subject_id");
            let bootstrap=self.c.query_one("SELECT EXISTS(SELECT 1 FROM accounts a JOIN grants g ON g.subject_id=a.id WHERE g.id=$1 AND a.id=$2 AND a.independent_owner AND a.parent_id IS NULL AND g.role_id='Owner' AND g.created_at=a.created_at AND g.issuer_id=a.id AND g.issuer_credential_id IS NULL)", &[&grant,&subject]).await?.get::<_,bool>(0);
            if !bootstrap {
                match Box::pin(self.account(issuer,depth+1)).await {
                    Ok(limits)=>paths=self.intersection(&paths,&limits).await?,
                    Err(e) if matches!(e.code,"INVALID_CREDENTIAL"|"PERMISSION_DENIED")=>return Ok(Vec::new()),
                    Err(e)=>return Err(e),
                }
            }
        }
        Ok(paths)
    }

    async fn freeze(&mut self, paths:Vec<Path>, ceiling:&Value, account:Option<Uuid>, credential:Option<Uuid>) -> Result<Vec<Path>> {
        let caps:Vec<Path>=ceiling.as_array().ok_or_else(Error::denied)?.iter().map(|r|Path{rule:r.clone(),sources:BTreeSet::new()}).collect();
        let paths=self.intersection(&paths,&caps).await?;
        let mut saved=Vec::new();
        let mut seen=HashSet::new();
        for row in self.c.query("SELECT source_rules FROM grant_provenance WHERE derived_account_id=$1 OR derived_credential_id=$2", &[&account,&credential]).await? {
            let v:Value=row.get(0);
            if v["version"]!=1 {return Err(Error::denied());}
            let p:Path=serde_json::from_value(v["path"].clone()).map_err(|_|Error::denied())?;
            if seen.insert(serde_json::to_string(&p)?) {
                let mut valid=true;
                for source in &p.sources {
                    let rule_paths=Box::pin(self.grant(*source,1)).await?;
                    valid &= !self.intersection(std::slice::from_ref(&p),&rule_paths).await?.is_empty();
                }
                if valid {saved.push(p);}
            }
        }
        // Source-less paths can only originate from fixed user self-service;
        // they are still intersected with the exact frozen selector and conditions.
        saved.extend(paths.iter().filter(|p|p.sources.is_empty()).cloned());
        self.intersection(&paths,&saved).await
    }

    pub async fn intersection(&self, left:&[Path],right:&[Path])->Result<Vec<Path>> {
        let mut out=Vec::new();let mut seen=HashSet::new();
        for a in left {for b in right {
            if a.rule["action"]!=b.rule["action"] {continue;}
            let Some(resource)=self.selector_intersection(&a.rule["resource"],&b.rule["resource"]).await? else {continue;};
            let Some(conditions)=intersect_conditions(a.rule.get("conditions").unwrap_or(&json!({})),b.rule.get("conditions").unwrap_or(&json!({}))) else {continue;};
            let rule=json!({"action":a.rule["action"],"resource":resource,"conditions":conditions});
            let sources=a.sources.union(&b.sources).copied().collect();
            let path=Path{rule,sources};
            if seen.insert(serde_json::to_string(&path)?) {out.push(path);bounded(&out)?;}
        }}
        Ok(out)
    }

    async fn selector_intersection(&self,a:&Value,b:&Value)->Result<Option<Value>> {
        if a["kind"]!=b["kind"] {return Ok(None);}
        if a["match"]=="all" {return Ok(Some(b.clone()));}
        if b["match"]=="all" || a==b {return Ok(Some(a.clone()));}
        if a["match"]=="owned_by" && b["match"]=="owned_by" {return Ok(None);}
        let (ids,other)=if a["match"]=="ids" {(a,b)} else {(b,a)};
        let mut kept=Vec::new();
        for value in ids["ids"].as_array().ok_or_else(Error::denied)? {
            let matches=if other["match"]=="ids" {other["ids"].as_array().is_some_and(|a|a.contains(value))} else {
                let owner=resource_owner(self.c,string(ids,"kind")?,value.as_str().ok_or_else(Error::denied)?).await?;
                owner.is_some_and(|owner|other["owner_id"]==owner.to_string())
            };
            if matches {kept.push(value.clone());}
        }
        Ok((!kept.is_empty()).then(||json!({"kind":a["kind"],"match":"ids","ids":kept})))
    }

    pub async fn select(&mut self, ctx:&Context, spec:&Value)->Result<Vec<Path>> {
        validate("InheritanceSpec",spec)?;
        let effective=self.credential(ctx.credential,0).await?;
        if spec["mode"]!="selected" {return Ok(effective);}
        let mut rules=spec.get("rules").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(groups)=spec.get("group_versions").and_then(Value::as_array) {
            for group in groups {
                let row=self.c.query_opt("SELECT v.rules,g.owner_id,g.visibility FROM group_versions v JOIN permission_groups g ON g.id=v.group_id WHERE v.group_id=$1 AND v.version=$2 AND g.status='active'", &[&id(group,"group_id")?,&group["version"].as_i64().ok_or_else(Error::denied)?]).await?.ok_or_else(Error::not_found)?;
                if row.get::<_,Uuid>("owner_id")!=ctx.subject && row.get::<_,String>("visibility")!="public" {return Err(Error::not_found());}
                rules.extend(row.get::<_,Value>("rules").as_array().ok_or_else(Error::denied)?.iter().cloned());
            }
        }
        self.validate_rules(&rules).await?;
        let requested:Vec<Path>=rules.into_iter().map(|rule|Path{rule,sources:BTreeSet::new()}).collect();
        // Selected is a ceiling, not an authority source. Intersect at issuance;
        // the exact resulting rules are persisted, never the group template alone.
        self.intersection(&effective,&requested).await
    }

    pub async fn validate_rules(&self,rules:&[Value])->Result<()> {
        for rule in rules {
            validate("PermissionRule",rule)?;
            if !registered(self.c,string(rule,"action")?,string(&rule["resource"],"kind")?).await? {return Err(Error::invalid("Unknown action or incompatible resource kind"));}
            if let Some(conditions)=rule.get("conditions") {validate_conditions(conditions)?;}
        }
        Ok(())
    }
    pub async fn require_subset(&mut self,ctx:&Context,rules:&[Value])->Result<()> {
        self.validate_rules(rules).await?;
        let available=self.credential(ctx.credential,0).await?;
        for rule in rules {
            // Require a covering alternative, including ALL non-action conditions.
            // Disjoint ID alternatives are checked independently.
            let parts=split_ids(rule);
            for part in parts {
                let wanted=Path{rule:part.clone(),sources:BTreeSet::new()};
                let intersections=self.intersection(std::slice::from_ref(&wanted),&available).await?;
                if !intersections.iter().any(|p|same_rule(&p.rule,&part)) {return Err(Error::denied());}
            }
        }
        Ok(())
    }
}

fn split_ids(rule:&Value)->Vec<Value>{
    if rule["resource"]["match"]!="ids" {return vec![rule.clone()];}
    rule["resource"]["ids"].as_array().unwrap().iter().map(|id|{let mut r=rule.clone();r["resource"]["ids"]=json!([id]);r}).collect()
}
fn same_rule(a:&Value,b:&Value)->bool {a["action"]==b["action"]&&a["resource"]==b["resource"]&&a.get("conditions").unwrap_or(&json!({}))==b.get("conditions").unwrap_or(&json!({}))}
fn bounded(paths:&[Path])->Result<()> {if paths.len()>MAX_PATHS {Err(Error::invalid("Policy expansion exceeds 4096 paths"))}else{Ok(())}}
pub fn rules(paths:&[Path])->Value {let mut out=Vec::new();for p in paths{if !out.contains(&p.rule){out.push(p.rule.clone());}}json!(out)}

pub async fn save_sources(c:&(impl GenericClient+Sync),paths:&[Path],account:Option<Uuid>,credential:Option<Uuid>)->Result<()> {
    for path in paths {for source in &path.sources {
        c.execute("INSERT INTO grant_provenance(id,derived_account_id,derived_credential_id,source_grant_id,source_rules) VALUES($1,$2,$3,$4,$5)", &[&Uuid::new_v4(),&account,&credential,&source,&json!({"version":1,"path":path})]).await?;
    }}
    Ok(())
}
pub async fn registered(c:&impl GenericClient,action:&str,kind:&str)->Result<bool>{
    Ok(c.query_opt("SELECT resource_kinds FROM permission_definitions WHERE action=$1", &[&action]).await?.is_some_and(|r|r.get::<_,Value>(0).as_array().is_some_and(|k|k.iter().any(|v|v==kind))))
}
pub async fn allows(c:&(impl GenericClient+Sync),ctx:&Context,action:&str,resource:&Resource)->Result<bool>{
    if !registered(c,action,&resource.kind).await? {return Ok(false);}
    let paths=Policy::new(c,ctx.peer).credential(ctx.credential,0).await?;
    Ok(rules_allow(&rules(&paths),action,resource,ctx.peer))
}

pub async fn resource_owner(c:&impl GenericClient,kind:&str,id:&str)->Result<Option<Uuid>>{
    let sql=match kind {
        "account"=>"SELECT id FROM accounts WHERE id::text=$1",
        "group"=>"SELECT owner_id FROM permission_groups WHERE id::text=$1",
        "credential"=>"SELECT owner_id FROM credentials WHERE id::text=$1",
        "deployment"=>"SELECT owner_id FROM deployments WHERE id::text=$1",
        "instance"=>"SELECT d.owner_id FROM instances i JOIN deployments d ON d.id=i.deployment_id WHERE i.id::text=$1",
        "operation"=>"SELECT owner_id FROM operations WHERE id::text=$1",
        "call"=>"SELECT owner_id FROM calls WHERE id::text=$1",
        "task"=>"SELECT owner_id FROM tasks WHERE id::text=$1",
        "workflow"=>"SELECT owner_id FROM workflows WHERE id::text=$1",
        "schedule"=>"SELECT owner_id FROM schedules WHERE id::text=$1",
        "volume"=>"SELECT owner_id FROM volumes WHERE id::text=$1",
        "blob"=>"SELECT owner_id FROM blobs WHERE id::text=$1",
        "secret"=>"SELECT owner_id FROM secrets WHERE id::text=$1",
        _=>return Ok(None),
    };
    Ok(c.query_opt(sql,&[&id]).await?.map(|r|r.get(0)))
}

pub fn validate_conditions(v:&Value)->Result<()> {
    if let Some(cidrs)=v.get("source_cidrs").and_then(Value::as_array) {
        for cidr in cidrs {cidr.as_str().ok_or_else(Error::denied)?.parse::<ipnet::IpNet>().map_err(|_|Error::invalid("Invalid source CIDR"))?;}
    }
    if let (Some(start),Some(end))=(v.get("not_before"),v.get("expires_at")) {
        if timestamp(start)? >= timestamp(end)? {return Err(Error::invalid("Empty validity interval"));}
    }
    Ok(())
}
pub fn timestamp(v:&Value)->Result<DateTime<Utc>> {DateTime::parse_from_rfc3339(v.as_str().ok_or_else(||Error::invalid("Expected timestamp"))?).map(|t|t.with_timezone(&Utc)).map_err(|_|Error::invalid("Invalid timestamp"))}
pub fn intersect_conditions(a:&Value,b:&Value)->Option<Value> {
    let mut out=json!({});
    for (key,latest) in [("not_before",true),("expires_at",false)] {
        let t=match (a.get(key),b.get(key)) {
            (Some(x),Some(y))=>{let x=timestamp(x).ok()?;let y=timestamp(y).ok()?;Some(if latest{x.max(y)}else{x.min(y)})},
            (Some(v),None)|(None,Some(v))=>Some(timestamp(v).ok()?),
            _=>None,
        };if let Some(t)=t {out[key]=json!(t);}
    }
    for key in ["audiences","source_cidrs"] {
        match (a.get(key),b.get(key)) {
            (Some(a),Some(b))=>{
                let mut values=Vec::new();
                for x in a.as_array()? {for y in b.as_array()? {
                    let v=if key=="source_cidrs" {
                        let x=x.as_str()?.parse::<ipnet::IpNet>().ok()?;let y=y.as_str()?.parse::<ipnet::IpNet>().ok()?;
                        if x.contains(&y) {Some(json!(y.to_string()))}else if y.contains(&x){Some(json!(x.to_string()))}else{None}
                    }else if x==y {Some(x.clone())}else{None};
                    if let Some(v)=v {if !values.contains(&v){values.push(v);}}
                }}
                if values.is_empty(){return None;}out[key]=json!(values);
            },
            (Some(v),None)|(None,Some(v))=>out[key]=v.clone(),
            _=>{},
        }
    }
    if let (Some(a),Some(b))=(out.get("not_before"),out.get("expires_at")){if timestamp(a).ok()?>=timestamp(b).ok()?{return None;}}
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn condition_intersection_never_widens() {
        let a=json!({"source_cidrs":["10.0.0.0/8"],"audiences":["xelab.platform","other"],"expires_at":"2030-01-01T00:00:00Z"});
        let b=json!({"source_cidrs":["10.1.0.0/16"],"audiences":["xelab.platform"],"expires_at":"2029-01-01T00:00:00Z"});
        assert_eq!(intersect_conditions(&a,&b),Some(b));
        assert!(intersect_conditions(&a,&json!({"source_cidrs":["192.0.2.0/24"]})).is_none());
        assert!(intersect_conditions(&a,&json!({"not_before":"2031-01-01T00:00:00Z"})).is_none());
    }
}
