use crate::{
    contracts::{digest, string, validate},
    db,
    error::{Error, Result},
};
use argon2::{password_hash::SaltString, Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_postgres::GenericClient;
use uuid::Uuid;

#[derive(Clone)]
pub struct Auth {
    key: Arc<Vec<u8>>,
    pub(crate) key_id: Arc<String>,
    failures: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
}
// Never deserialize a security context from an HTTP body or RPC message.
#[derive(Clone)]
pub struct Context {
    pub(crate) subject: Uuid,
    pub(crate) credential: Uuid,
    pub(crate) peer: IpAddr,
    pub(crate) request: Uuid,
    pub(crate) chain: Vec<Uuid>,
}
#[derive(Clone)]
pub struct Resource {
    pub kind: String,
    pub id: String,
    pub owner: Option<Uuid>,
}

impl Resource {
    pub fn new(kind: &str, id: impl ToString, owner: Option<Uuid>) -> Self {
        Self {
            kind: kind.into(),
            id: id.to_string(),
            owner,
        }
    }
    pub fn json(&self) -> Value {
        json!({"kind":self.kind,"match":"ids","ids":[self.id]})
    }
}

impl Auth {
    pub fn new(key: Vec<u8>) -> Result<Self> {
        if key.len() < 32 {
            return Err(Error::invalid("HMAC key is too short"));
        }
        let key_id = digest(&key);
        Ok(Self {
            key: Arc::new(key),
            key_id: Arc::new(key_id),
            failures: Default::default(),
        })
    }
    pub fn token_digest(&self, token: &str) -> Vec<u8> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key length");
        mac.update(token.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }
    pub(crate) fn verify_mac(&self, value: &str, signature: &[u8]) -> bool {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC key");
        mac.update(value.as_bytes());
        mac.verify_slice(signature).is_ok()
    }
    pub(crate) fn random_token(&self) -> (String, Vec<u8>) {
        let mut raw = [0u8; 32];
        OsRng.fill_bytes(&mut raw);
        let token = URL_SAFE_NO_PAD.encode(raw);
        let hash = self.token_digest(&token);
        (token, hash)
    }
    pub async fn authenticate(
        &self,
        c: &impl GenericClient,
        token: &str,
        peer: IpAddr,
        request: Uuid,
    ) -> Result<Context> {
        if token.len() > 512 || token.len() < 32 {
            return Err(Error::credential());
        }
        crate::db::auth_lock(c, false).await?;
        let hash = self.token_digest(token);
        let r=c.query_opt("SELECT * FROM credentials WHERE hmac_key_id=$1 AND verification_digest=$2 FOR SHARE",&[&self.key_id.as_str(),&hash]).await?.ok_or_else(Error::credential)?;
        let stored: Vec<u8> = r.get("verification_digest");
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).unwrap();
        mac.update(token.as_bytes());
        mac.verify_slice(&stored).map_err(|_| Error::credential())?;
        self.resume(c, r.get("id"), peer, request).await
    }
    pub(crate) async fn resume(
        &self,
        c: &impl GenericClient,
        credential: Uuid,
        peer: IpAddr,
        request: Uuid,
    ) -> Result<Context> {
        crate::db::auth_lock(c, false).await?;
        let r = c
            .query_opt(
                "SELECT owner_id FROM credentials WHERE id=$1 AND kind IN ('session','api_key')",
                &[&credential],
            )
            .await?
            .ok_or_else(Error::credential)?;
        let subject: Uuid = r.get(0);
        let chain = validate_sources(c, Some(credential), Some(subject), peer).await?;
        Ok(Context {
            subject,
            credential,
            peer,
            request,
            chain,
        })
    }
    pub fn check_login_budget(&self, account: Uuid, peer: IpAddr) -> Result<()> {
        let mut state = self
            .failures
            .lock()
            .map_err(|_| Error::unavailable("Login limiter unavailable"))?;
        state.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        let keys = [format!("account:{account}"), format!("peer:{peer}")];
        if state.len() > 8190
            || keys
                .iter()
                .any(|key| state.get(key).is_some_and(|(_, n)| *n >= 5))
        {
            return Err(Error::new(
                "RATE_LIMITED",
                "Login attempt limit exceeded",
                axum::http::StatusCode::TOO_MANY_REQUESTS,
            ));
        }
        for key in keys {
            state.entry(key).or_insert((Instant::now(), 0)).1 += 1;
        }
        Ok(())
    }
    fn login_succeeded(&self, account: Uuid, peer: IpAddr) -> Result<()> {
        let mut state = self
            .failures
            .lock()
            .map_err(|_| Error::unavailable("Login limiter unavailable"))?;
        for key in [format!("account:{account}"), format!("peer:{peer}")] {
            if let Some((_, n)) = state.get_mut(&key) {
                *n = n.saturating_sub(1);
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)] // The delegation binds all of these independently checked identities.
    pub(crate) async fn issue_rpc(
        &self,
        c: &impl GenericClient,
        ctx: &Context,
        call: Uuid,
        instance: Uuid,
        generation: i64,
        deadline: DateTime<Utc>,
    ) -> Result<String> {
        let execution = Uuid::new_v4();
        // The supported pure profile grants no HostService authority.
        c.execute("INSERT INTO execution_delegations(id,subject_id,issuer_credential_id,maximum_rules,run_as,expires_at,status,created_at) VALUES($1,$2,$3,'[]','user',$4,'active',now())", &[&execution,&ctx.subject,&ctx.credential,&deadline]).await?;
        c.execute(
            "UPDATE calls SET delegation_id=$2 WHERE id=$1",
            &[&call, &execution],
        )
        .await?;
        let (token, hash) = self.random_token();
        c.execute("INSERT INTO rpc_delegations(id,execution_delegation_id,subject_id,instance_id,generation,call_id,issuer_credential_id,hmac_key_id,verification_digest,expires_at,status,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'active',now())", &[&Uuid::new_v4(),&execution,&ctx.subject,&instance,&generation,&call,&ctx.credential,&self.key_id.as_str(),&hash,&deadline]).await?;
        Ok(token)
    }
    pub async fn login(
        &self,
        c: &impl GenericClient,
        account: Uuid,
        password: String,
        peer: IpAddr,
        request: Uuid,
    ) -> Result<Value> {
        self.check_login_budget(account, peer)?;
        let r = c
            .query_opt("SELECT * FROM accounts WHERE id=$1 FOR SHARE", &[&account])
            .await?;
        let real_hash = r
            .as_ref()
            .and_then(|r| r.get::<_, Option<String>>("password_hash"));
        let password_ok = tokio::task::spawn_blocking(move || {
            if let Some(hash) = real_hash {
                PasswordHash::new(&hash).ok().is_some_and(|hash| {
                    Argon2::default()
                        .verify_password(password.as_bytes(), &hash)
                        .is_ok()
                })
            } else {
                let _ = password_hash(&password);
                false
            }
        })
        .await
        .map_err(|_| Error::unavailable("Password verifier unavailable"))?;
        let active = r.as_ref().is_some_and(|r| {
            r.get::<_, String>("kind") == "user" && r.get::<_, String>("status") == "active"
        });
        if !password_ok
            || !active
            || validate_sources(c, None, Some(account), peer)
                .await
                .is_err()
        {
            let subject = r.as_ref().map(|_| account);
            c.execute("INSERT INTO audit_events(id,subject_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,'login',$4,'denied',1,'INVALID_CREDENTIAL',now(),now())",&[&Uuid::new_v4(),&subject,&request,&json!({"kind":"account","match":"ids","ids":[account]})]).await?;
            return Err(Error::credential());
        }
        self.login_succeeded(account, peer)?;
        let (token, hash) = self.random_token();
        let id = Uuid::new_v4();
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(1);
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,client_id,status,conditions,expires_at,last_used_at,created_at,updated_at) VALUES($1,$2,'session',$3,$4,'xelabctl','active','{}',$5,$6,$6,$6)",&[&id,&account,&self.key_id.as_str(),&hash,&expires,&now]).await?;
        db::audit(
            c,
            account,
            Some(id),
            request,
            "login",
            &Resource::new("account", account, Some(account)).json(),
            "succeeded",
            "SESSION_ISSUED",
        )
        .await?;
        let session = json!({"id":id,"revision":1,"created_at":now,"updated_at":now,"account_id":account,"client_id":"xelabctl","expires_at":expires,"last_seen_at":now,"status":"active"});
        let v =
            json!({"session":session,"access_token":token,"token_type":"Bearer","expires_in":3600});
        validate("SessionIssued", &v)?;
        Ok(v)
    }
}

pub fn password_hash(password: &str) -> Result<String> {
    if !(12..=128).contains(&password.chars().count()) {
        return Err(Error::invalid("Passwords require 12 to 128 characters"));
    }
    Argon2::default()
        .hash_password(password.as_bytes(), &SaltString::generate(&mut OsRng))
        .map(|h| h.to_string())
        .map_err(|_| Error::unavailable("Password hashing failed"))
}

pub(crate) async fn account_chain(c: &impl GenericClient, subject: Uuid) -> Result<Vec<Uuid>> {
    let mut ids = Vec::new();
    let mut next = Some(subject);
    while let Some(id) = next {
        if ids.len() >= 8 || ids.contains(&id) {
            return Err(Error::credential());
        }
        let r = c
            .query_opt(
                "SELECT parent_id,status,expires_at FROM accounts WHERE id=$1 FOR SHARE",
                &[&id],
            )
            .await?
            .ok_or_else(Error::credential)?;
        if r.get::<_, String>("status") != "active"
            || r.get::<_, Option<DateTime<Utc>>>("expires_at")
                .is_some_and(|d| d <= Utc::now())
        {
            return Err(Error::credential());
        }
        ids.push(id);
        next = r.get("parent_id");
    }
    ids.reverse();
    Ok(ids)
}

pub fn conditions_match(v: &Value, peer: IpAddr, now: DateTime<Utc>) -> bool {
    if !v.is_object() {
        return false;
    }
    for (k, after) in [("not_before", true), ("expires_at", false)] {
        if let Some(raw) = v.get(k) {
            let Some(t) = raw
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            else {
                return false;
            };
            if if after { now < t } else { now >= t } {
                return false;
            }
        }
    }
    if let Some(a) = v.get("source_cidrs") {
        if !a.as_array().is_some_and(|a| {
            !a.is_empty()
                && a.iter().any(|s| {
                    s.as_str()
                        .and_then(|s| s.parse::<ipnet::IpNet>().ok())
                        .is_some_and(|n| n.contains(&peer))
                })
        }) {
            return false;
        }
    }
    if let Some(a) = v.get("audiences") {
        if !a
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v == "xelab.platform"))
        {
            return false;
        }
    }
    v.as_object().unwrap().keys().all(|k| {
        matches!(
            k.as_str(),
            "not_before" | "expires_at" | "source_cidrs" | "audiences" | "max_calls"
        )
    })
}
pub fn selector_matches(v: &Value, r: &Resource) -> bool {
    if v["kind"] != r.kind {
        return false;
    }
    match v["match"].as_str() {
        Some("all") => true,
        Some("ids") => v["ids"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == &r.id)),
        Some("owned_by") => r.owner.is_some_and(|o| v["owner_id"] == o.to_string()),
        _ => false,
    }
}
pub fn rules_allow(rules: &Value, action: &str, resource: &Resource, peer: IpAddr) -> bool {
    rules.as_array().is_some_and(|a| {
        a.iter().any(|r| {
            r["action"] == action
                && selector_matches(&r["resource"], resource)
                && r.get("conditions")
                    .is_none_or(|c| conditions_match(c, peer, Utc::now()))
        })
    })
}

pub async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    res: &Resource,
) -> Result<()> {
    if crate::policy::allows(c, ctx, action, res).await? {
        Ok(())
    } else {
        Err(Error::denied())
    }
}

/// Validate the complete account/credential source graph, including sources on
/// ancestor accounts. Non-action restrictions also gate fixed self-service.
pub(crate) async fn validate_sources(
    c: &impl GenericClient,
    credential: Option<Uuid>,
    account: Option<Uuid>,
    peer: IpAddr,
) -> Result<Vec<Uuid>> {
    let mut pending = Vec::new();
    if let Some(id) = credential {
        pending.push((true, id));
    }
    if let Some(id) = account {
        pending.push((false, id));
    }
    let mut seen = HashSet::new();
    let mut credentials = Vec::new();
    while let Some((is_credential, id)) = pending.pop() {
        if !seen.insert((is_credential, id)) {
            continue;
        }
        if seen.len() > 512 {
            return Err(Error::credential());
        }
        if is_credential {
            let r = c
                .query_opt("SELECT * FROM credentials WHERE id=$1 FOR SHARE", &[&id])
                .await?
                .ok_or_else(Error::credential)?;
            if r.get::<_, String>("status") != "active"
                || r.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
                || !conditions_match(&r.get::<_, Value>("conditions"), peer, Utc::now())
            {
                return Err(Error::credential());
            }
            let kind: String = r.get("kind");
            if !matches!(kind.as_str(), "session" | "api_key") {
                return Err(Error::credential());
            }
            pending.push((false, r.get("owner_id")));
            if let Some(issuer) = r.get::<_, Option<Uuid>>("issuer_credential_id") {
                pending.push((true, issuer));
            }
            credentials.push(id);
        } else {
            for ancestor in account_chain(c, id).await? {
                if let Some(r) = c
                    .query_opt(
                        "SELECT issuer_credential_id FROM inheritance_grants WHERE account_id=$1",
                        &[&ancestor],
                    )
                    .await?
                {
                    if let Some(issuer) = r.get::<_, Option<Uuid>>(0) {
                        pending.push((true, issuer));
                    }
                }
            }
        }
    }
    // Parent links are immutable and issuance always points to an existing key.
    // Also reject corrupted credential cycles explicitly, independent of dedup.
    for start in &credentials {
        let mut cursor = Some(*start);
        let mut chain = HashSet::new();
        while let Some(id) = cursor {
            if !chain.insert(id) || chain.len() > 64 {
                return Err(Error::credential());
            }
            cursor = c
                .query_one(
                    "SELECT issuer_credential_id FROM credentials WHERE id=$1",
                    &[&id],
                )
                .await?
                .get(0);
        }
    }
    credentials.sort();
    Ok(credentials)
}

pub async fn seed_roles(c: &impl GenericClient) -> Result<()> {
    let defs: Value =
        serde_json::from_str(include_str!("../../../docs/contracts/permissions.json"))?;
    let actions: Vec<&str> = defs["definitions"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let templates = [
        (
            "Owner",
            actions.clone(),
            "所有当前注册动作，仍受强制边界限制",
        ),
        (
            "Operator",
            vec![
                "plugin.read",
                "plugin.enable",
                "plugin.upgrade",
                "operation.read",
                "operation.cancel",
                "platform.read",
                "plugin.logs.read",
            ],
            "插件和平台运维",
        ),
        (
            "Auditor",
            vec!["audit.read", "platform.read", "plugin.read"],
            "脱敏审计",
        ),
        (
            "Guest",
            vec!["capability.discover", "capability.invoke", "plugin.ui.open"],
            "仅明确共享的能力和页面",
        ),
        (
            "Member",
            actions
                .iter()
                .copied()
                .filter(|a| {
                    a.starts_with("task.")
                        || a.starts_with("workflow.")
                        || a.starts_with("schedule.")
                        || a.starts_with("blob.")
                        || a.starts_with("batch.")
                        || a.starts_with("micro.")
                        || matches!(*a, "capability.discover" | "capability.invoke")
                })
                .collect(),
            "本人业务资源；必须绑定明确 scope",
        ),
    ];
    for (name, actions, description) in templates {
        c.execute("INSERT INTO role_definitions(id,definition,version) VALUES($1,$2,1) ON CONFLICT(id) DO UPDATE SET definition=EXCLUDED.definition", &[&name,&json!({"actions":actions,"description":description})]).await?;
    }
    Ok(())
}

pub async fn bootstrap(
    c: &impl GenericClient,
    name: &str,
    password: &str,
    quota: &Value,
) -> Result<Uuid> {
    validate("QuotaSpec", quota)?;
    if name.is_empty() || name.chars().count() > 80 {
        return Err(Error::invalid("Invalid owner name"));
    }
    let hash = password_hash(password)?;
    let r = c.query_one("SELECT count(*) FROM accounts", &[]).await?;
    if r.get::<_, i64>(0) != 0 {
        return Err(Error::conflict(
            "Bootstrap is only available for an empty database",
        ));
    }
    let defs: Value =
        serde_json::from_str(include_str!("../../../docs/contracts/permissions.json"))?;
    let mut kinds = HashSet::new();
    for (action, spec) in defs["definitions"].as_object().unwrap() {
        c.execute("INSERT INTO permission_definitions(action,resource_kinds,description,self_service) VALUES($1,$2,$3,$4)",&[&action,&spec["resource_kinds"],&string(spec,"description")?,&spec["self_service"].as_bool().unwrap_or(false)]).await?;
        for kind in spec["resource_kinds"].as_array().unwrap() {
            kinds.insert(kind.as_str().unwrap());
        }
    }
    seed_roles(c).await?;
    let id = Uuid::new_v4();
    c.execute("INSERT INTO accounts(id,kind,status,display_name,independent_owner,password_hash,created_at,updated_at) VALUES($1,'user','active',$2,true,$3,now(),now())",&[&id,&name,&hash]).await?;
    c.execute(
        "INSERT INTO account_closure(ancestor_id,descendant_id,depth) VALUES($1,$1,0)",
        &[&id],
    )
    .await?;
    c.execute("INSERT INTO quotas(account_id,spec,daily_bucket,created_at,updated_at) VALUES($1,$2,current_date,now(),now())",&[&id,&quota]).await?;
    for kind in kinds {
        c.execute("INSERT INTO grants(id,subject_id,issuer_id,role_id,scope,status,created_at,updated_at) VALUES($1,$2,$2,'Owner',$3,'active',now(),now())",&[&Uuid::new_v4(),&id,&json!({"kind":kind,"match":"all"})]).await?;
    }
    db::audit(
        c,
        id,
        None,
        Uuid::new_v4(),
        "bootstrap",
        &Resource::new("account", id, Some(id)).json(),
        "succeeded",
        "OWNER_CREATED",
    )
    .await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permissions_need_action_object_and_conditions() {
        let id = Uuid::new_v4();
        let r = Resource::new("deployment", "d1", Some(id));
        let p = "127.0.0.1".parse().unwrap();
        let rules = json!([{"action":"plugin.enable","resource":{"kind":"deployment","match":"owned_by","owner_id":id},"conditions":{"source_cidrs":["127.0.0.0/8"]}}]);
        assert!(rules_allow(&rules, "plugin.enable", &r, p));
        assert!(!rules_allow(&rules, "plugin.install", &r, p));
        assert!(!rules_allow(
            &rules,
            "plugin.enable",
            &Resource::new("deployment", "d1", Some(Uuid::new_v4())),
            p
        ));
        assert!(!rules_allow(
            &rules,
            "plugin.enable",
            &r,
            "192.0.2.1".parse().unwrap()
        ));
    }
    #[test]
    fn credentials_are_random_and_hash_only() {
        let a = Auth::new(vec![7; 32]).unwrap();
        let (x, h) = a.random_token();
        let (y, _) = a.random_token();
        assert_ne!(x, y);
        assert_eq!(h, a.token_digest(&x));
        assert_ne!(h, x.as_bytes());
        assert_eq!(h.len(), 32);
    }
    #[test]
    fn login_limits_both_account_and_peer() {
        let a = Auth::new(vec![9; 32]).unwrap();
        let id = Uuid::new_v4();
        let p = "127.0.0.1".parse().unwrap();
        for _ in 0..5 {
            a.check_login_budget(id, p).unwrap();
        }
        assert!(a.check_login_budget(Uuid::new_v4(), p).is_err());
        assert!(a
            .check_login_budget(id, "192.0.2.1".parse().unwrap())
            .is_err());
    }
}

