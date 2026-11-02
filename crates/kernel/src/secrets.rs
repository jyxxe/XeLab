//! Encrypted, write-only Secret management. All DB work runs under the API's
//! authorization fence; ciphertext authenticates its row identity and version.
use crate::{
    api::{etag, path_id, read_object},
    auth::{self, Auth, Context, Resource},
    config::{read_private, Config},
    contracts::{digest, id, string, validate},
    error::{Error, Result},
    identity,
};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::STANDARD, Engine};
use chrono::{DateTime, Utc};
use rand::RngCore;
use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};
use tokio_postgres::GenericClient;
use uuid::Uuid;
use zeroize::Zeroizing;

pub struct MasterKey {
    cipher: LessSafeKey,
    pub id: String,
}
impl MasterKey {
    pub fn load(config: &Config) -> Result<Self> {
        let bytes = Zeroizing::new(read_private(
            Path::new(config.text("security", "master_key_file")),
            32,
        )?);
        Self::new(&bytes)
    }
    fn new(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(Error::invalid("Master key must contain exactly 32 bytes"));
        }
        let key = UnboundKey::new(&aead::AES_256_GCM, bytes)
            .map_err(|_| Error::unavailable("Secret encryption is unavailable"))?;
        Ok(Self {
            cipher: LessSafeKey::new(key),
            id: digest(bytes),
        })
    }
    fn aad(&self, secret: Uuid, owner: Uuid, version: i64) -> Vec<u8> {
        format!("xelab-secret:v1:{secret}:{owner}:{version}:{}", self.id).into_bytes()
    }
    fn seal(&self, secret: Uuid, owner: Uuid, version: i64, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let mut ciphertext = plaintext.to_vec();
        self.cipher
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(secret, owner, version)),
                &mut ciphertext,
            )
            .map_err(|_| Error::unavailable("Secret encryption failed"))?;
        let mut envelope = vec![1];
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }
    pub fn open(
        &self,
        secret: Uuid,
        owner: Uuid,
        version: i64,
        key_id: &str,
        envelope: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>> {
        if key_id != self.id
            || envelope.len() < 29
            || envelope[0] != 1
            || envelope.len() > 65536 + 29
        {
            return Err(Error::unavailable(
                "Secret key or ciphertext is unavailable",
            ));
        }
        let nonce: [u8; 12] = envelope[1..13]
            .try_into()
            .map_err(|_| Error::unavailable("Invalid ciphertext"))?;
        let mut buffer = Zeroizing::new(envelope[13..].to_vec());
        let size = self
            .cipher
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(self.aad(secret, owner, version)),
                &mut buffer,
            )
            .map_err(|_| Error::unavailable("Secret authentication failed"))?
            .len();
        buffer.truncate(size);
        Ok(buffer)
    }
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "createSecret"
            | "listSecrets"
            | "getSecretMetadata"
            | "rotateSecret"
            | "revokeSecret"
            | "bindSecret"
            | "listSecretBindings"
            | "getSecretBinding"
            | "revokeSecretBinding"
    )
}
pub fn writes_value(name: &str) -> bool {
    matches!(name, "createSecret" | "rotateSecret")
}
async fn secret_access(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    secret: Uuid,
    action: &str,
) -> Result<()> {
    let row = c
        .query_opt("SELECT owner_id FROM secrets WHERE id=$1", &[&secret])
        .await?
        .ok_or_else(Error::not_found)?;
    auth::authorize(
        c,
        ctx,
        action,
        &Resource::new("secret", secret, Some(row.get(0))),
    )
    .await
}
pub async fn deployment_access(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
) -> Result<()> {
    let row = c
        .query_opt(
            "SELECT owner_id FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    auth::authorize(
        c,
        ctx,
        "plugin.permission.grant",
        &Resource::new("deployment", deployment, Some(row.get(0))),
    )
    .await
}
pub async fn authorize(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
) -> Result<()> {
    let result = async {
        match name {
            "createSecret" => {
                auth::authorize(
                    c,
                    ctx,
                    "secret.create",
                    &Resource::new("secret", "new", Some(ctx.subject)),
                )
                .await
            }
            "listSecrets" | "listSecretBindings" => Ok(()), // each object is filtered
            "bindSecret" => {
                secret_access(c, ctx, id(body, "secret_id")?, "secret.bind").await?;
                deployment_access(c, ctx, id(body, "deployment_id")?).await
            }
            "getSecretBinding" | "revokeSecretBinding" => {
                let binding = path_id(params, "binding_id")?;
                let row = c
                    .query_opt(
                        "SELECT secret_id,deployment_id FROM secret_bindings WHERE id=$1",
                        &[&binding],
                    )
                    .await?
                    .ok_or_else(Error::not_found)?;
                secret_access(c, ctx, row.get(0), "secret.bind").await?;
                deployment_access(c, ctx, row.get(1)).await
            }
            _ => {
                secret_access(
                    c,
                    ctx,
                    path_id(params, "secret_id")?,
                    match name {
                        "getSecretMetadata" => "secret.metadata.read",
                        "rotateSecret" => "secret.rotate",
                        "revokeSecret" => "secret.revoke",
                        _ => return Err(Error::not_found()),
                    },
                )
                .await
            }
        }
    }
    .await;
    result.map_err(|e| {
        if name.starts_with("get") && e.code == "PERMISSION_DENIED" {
            Error::not_found()
        } else {
            e
        }
    })
}

pub async fn metadata(c: &(impl GenericClient + Sync), secret: Uuid) -> Result<Value> {
    let mut value = read_object(c, "secrets", secret, "Secret").await?;
    if value["status"] == "active"
        && value["expires_at"]
            .as_str()
            .is_some_and(|v| DateTime::parse_from_rfc3339(v).is_ok_and(|t| t <= Utc::now()))
    {
        value["status"] = json!("expired");
    }
    Ok(value)
}
pub async fn binding_value(c: &(impl GenericClient + Sync), binding: Uuid) -> Result<Value> {
    let row = c
        .query_opt("SELECT * FROM secret_bindings WHERE id=$1", &[&binding])
        .await?
        .ok_or_else(Error::not_found)?;
    let mut value = crate::db::stamp(&row);
    value["spec"] = json!({"secret_id":row.get::<_,Uuid>("secret_id"),"deployment_id":row.get::<_,Uuid>("deployment_id"),"maximum_lease_seconds":row.get::<_,i32>("maximum_lease_seconds")});
    value["status"] = json!(row.get::<_, String>("status"));
    validate("SecretBinding", &value)?;
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
pub async fn execute(
    c: &(impl GenericClient + Sync),
    config: &Config,
    auth: &Auth,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    match name {
        "createSecret" | "rotateSecret" => {
            let value = Zeroizing::new(
                STANDARD
                    .decode(string(body, "value_base64")?)
                    .map_err(|_| Error::invalid("Invalid Secret base64"))?,
            );
            if value.is_empty() || value.len() > 65536 {
                return Err(Error::invalid(
                    "Secret content must contain 1 through 65536 bytes",
                ));
            }
            let expires: Option<DateTime<Utc>> = body
                .get("expires_at")
                .map(crate::policy::timestamp)
                .transpose()?;
            if expires.is_some_and(|t| t <= Utc::now()) {
                return Err(Error::invalid("Secret expiry must be in the future"));
            }
            let key = MasterKey::load(config)?;
            let secret = if name == "createSecret" {
                Uuid::new_v4()
            } else {
                path_id(params, "secret_id")?
            };
            let (owner, version) = if name == "createSecret" {
                (ctx.subject, 1)
            } else {
                let row = c.query_one("SELECT owner_id,version,revision,status FROM secrets WHERE id=$1 FOR UPDATE", &[&secret]).await?;
                etag(headers, row.get("revision"))?;
                if row.get::<_, String>("status") != "active" {
                    return Err(Error::conflict("A revoked Secret cannot be rotated"));
                }
                (
                    row.get("owner_id"),
                    row.get::<_, i64>("version")
                        .checked_add(1)
                        .filter(|v| *v <= i32::MAX as i64)
                        .ok_or_else(Error::exhausted)?,
                )
            };
            let ciphertext = key.seal(secret, owner, version, &value)?;
            let name_value = string(body, "name")?;
            if name == "createSecret" {
                c.execute("INSERT INTO secrets(id,owner_id,name,version,encrypted_value,encryption_key_id,expires_at,status,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'active',now(),now())", &[&secret,&owner,&name_value,&version,&ciphertext,&key.id,&expires]).await?;
            } else {
                c.execute("UPDATE secrets SET name=$2,version=$3,encrypted_value=$4,encryption_key_id=$5,expires_at=$6,revision=revision+1,updated_at=now() WHERE id=$1", &[&secret,&name_value,&version,&ciphertext,&key.id,&expires]).await?;
            }
            Ok((
                metadata(c, secret).await?,
                if name == "createSecret" { 201 } else { 200 },
            ))
        }
        "getSecretMetadata" => Ok((metadata(c, path_id(params, "secret_id")?).await?, 200)),
        "revokeSecret" => {
            let secret = path_id(params, "secret_id")?;
            let row = c
                .query_one(
                    "SELECT revision FROM secrets WHERE id=$1 FOR UPDATE",
                    &[&secret],
                )
                .await?;
            etag(headers, row.get(0))?;
            c.execute("UPDATE secrets SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1", &[&secret]).await?;
            Ok((Value::Null, 204))
        }
        "bindSecret" => {
            let secret = id(body, "secret_id")?;
            let deployment = id(body, "deployment_id")?;
            // Serialize binding creation with revoke/rotation and deployment state.
            let manifest=c.query_opt("SELECT a.manifest FROM deployments d JOIN installations a ON a.id=d.installation_id WHERE d.id=$1 AND d.archived_at IS NULL FOR UPDATE OF d", &[&deployment]).await?.ok_or_else(||Error::conflict("Archived deployment cannot receive a new binding"))?.get::<_,Value>(0);
            let secret_owner = c
                .query_one("SELECT owner_id FROM secrets WHERE id=$1", &[&secret])
                .await?
                .get(0);
            if !requested_use(&manifest, secret, secret_owner, ctx.peer) {
                return Err(Error::denied());
            }
            if c.query_opt("SELECT id FROM secrets WHERE id=$1 AND status='active' AND (expires_at IS NULL OR expires_at>now()) FOR UPDATE", &[&secret]).await?.is_none() { return Err(Error::conflict("Secret is not active")); }
            if c.query_opt("SELECT id FROM secret_bindings WHERE deployment_id=$1 AND secret_id=$2 AND status='active'", &[&deployment,&secret]).await?.is_some() { return Err(Error::conflict("An active binding already exists")); }
            let binding = Uuid::new_v4();
            let seconds = body["maximum_lease_seconds"].as_i64().unwrap() as i32;
            c.execute("INSERT INTO secret_bindings(id,deployment_id,secret_id,maximum_lease_seconds,status,created_at,updated_at) VALUES($1,$2,$3,$4,'active',now(),now())", &[&binding,&deployment,&secret,&seconds]).await?;
            let peer = ctx.peer.to_string();
            c.execute("INSERT INTO secret_binding_authority(binding_id,credential_id,peer) VALUES($1,$2,$3::text::inet)", &[&binding,&ctx.credential,&peer]).await?;
            Ok((binding_value(c, binding).await?, 201))
        }
        "getSecretBinding" => Ok((binding_value(c, path_id(params, "binding_id")?).await?, 200)),
        "revokeSecretBinding" => {
            let binding = path_id(params, "binding_id")?;
            let row = c
                .query_one(
                    "SELECT revision FROM secret_bindings WHERE id=$1 FOR UPDATE",
                    &[&binding],
                )
                .await?;
            etag(headers, row.get(0))?;
            c.execute("UPDATE secret_bindings SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1", &[&binding]).await?;
            Ok((Value::Null, 204))
        }
        "listSecrets" | "listSecretBindings" => Ok((
            identity::list(c, auth, ctx, name, params, query).await?,
            200,
        )),
        _ => Err(Error::not_found()),
    }
}

fn requested_use(manifest: &Value, secret: Uuid, owner: Uuid, peer: std::net::IpAddr) -> bool {
    auth::rules_allow(
        manifest.get("requested_rules").unwrap_or(&Value::Null),
        "secret.use",
        &Resource::new("secret", secret, Some(owner)),
        peer,
    )
}

/// The caller derives identity from a CA-verified client leaf, never RpcContext.
/// The shared authorization fence orders leases against revocation.
pub async fn lease(
    app: &crate::api::App,
    instance: Uuid,
    generation: i64,
    request: &crate::rpc::LeaseSecretRequest,
) -> Result<crate::rpc::SecretLease> {
    if !app.ready.load(std::sync::atomic::Ordering::SeqCst) || app.leader.is_closed() {
        return Err(Error::unavailable("Kernel is not ready"));
    }
    let context = request
        .context
        .as_ref()
        .ok_or_else(|| Error::invalid("RPC context is required"))?;
    if context.deadline_unix_ms <= Utc::now().timestamp_millis()
        || (!context.actor_instance_id.is_empty()
            && context.actor_instance_id != instance.to_string())
    {
        return Err(Error::denied());
    }
    let binding: Uuid = request
        .binding_id
        .parse()
        .map_err(|_| Error::invalid("Invalid binding ID"))?;
    if !(1..=900).contains(&request.requested_seconds) {
        return Err(Error::invalid(
            "Secret lease duration must be 1 through 900 seconds",
        ));
    }
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    crate::db::auth_lock(&tx, false).await?;
    let row=tx.query_opt("SELECT b.secret_id,b.deployment_id,b.maximum_lease_seconds,s.owner_id,s.version,s.encrypted_value,s.encryption_key_id,s.expires_at,a.credential_id,host(a.peer) AS approved_peer,t.manifest FROM secret_bindings b JOIN secret_binding_authority a ON a.binding_id=b.id JOIN secrets s ON s.id=b.secret_id JOIN deployments d ON d.id=b.deployment_id JOIN instances i ON i.deployment_id=d.id JOIN installations t ON t.id=d.installation_id JOIN publishers p ON p.id=t.publisher_id WHERE b.id=$1 AND b.status='active' AND s.status='active' AND (s.expires_at IS NULL OR s.expires_at>now()) AND i.id=$2 AND i.generation=$3 AND i.phase IN ('ready','degraded') AND i.runtime_id=i.id::text AND d.spec->'enabled'='true'::jsonb AND d.archived_at IS NULL AND t.state='installed' AND p.status='trusted' FOR SHARE OF b,s,d,i,t,p", &[&binding,&instance,&generation]).await?.ok_or_else(Error::denied)?;
    if request.requested_seconds > row.get::<_, i32>("maximum_lease_seconds") as u32 {
        return Err(Error::invalid("Lease exceeds binding duration"));
    }
    let secret: Uuid = row.get("secret_id");
    let owner: Uuid = row.get("owner_id");
    let peer = row
        .get::<_, String>("approved_peer")
        .parse()
        .map_err(|_| Error::denied())?;
    let authority = app
        .auth
        .resume(&tx, row.get("credential_id"), peer, Uuid::new_v4())
        .await?;
    secret_access(&tx, &authority, secret, "secret.bind").await?;
    deployment_access(&tx, &authority, row.get("deployment_id")).await?;
    if !requested_use(&row.get("manifest"), secret, owner, peer) {
        return Err(Error::denied());
    }
    let expires: Option<DateTime<Utc>> = row.get("expires_at");
    let until = (Utc::now() + chrono::Duration::seconds(request.requested_seconds as i64))
        .min(expires.unwrap_or(DateTime::<Utc>::MAX_UTC));
    let key = MasterKey::load(&app.config)?;
    let version: i64 = row.get("version");
    let plaintext = key.open(
        secret,
        owner,
        version,
        &row.get::<_, String>("encryption_key_id"),
        &row.get::<_, Vec<u8>>("encrypted_value"),
    )?;
    let lease = Uuid::new_v4();
    tx.execute("INSERT INTO secret_leases(id,binding_id,instance_id,generation,secret_version,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,now())", &[&lease,&binding,&instance,&generation,&version,&until]).await?;
    tx.execute("INSERT INTO audit_events(id,subject_id,actor_instance_id,credential_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'secret.use',$6,'succeeded',(SELECT authz_revision FROM accounts WHERE id=$2),'LEASED',now(),now())", &[&Uuid::new_v4(),&authority.subject,&instance,&authority.credential,&authority.request,&Resource::new("secret",secret,Some(owner)).json()]).await?;
    if until <= Utc::now()
        || app.leader.is_closed()
        || context.deadline_unix_ms <= Utc::now().timestamp_millis()
    {
        return Err(Error::unavailable("Lease authority expired"));
    }
    tx.commit().await?;
    Ok(crate::rpc::SecretLease {
        lease_id: lease.to_string(),
        value: plaintext.to_vec(),
        secret_version: version as u64,
        expires_unix_ms: until.timestamp_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ciphertext_authenticates_identity_version_key_and_every_byte() {
        let key = MasterKey::new(&[42; 32]).unwrap();
        let id = Uuid::new_v4();
        let owner = Uuid::new_v4();
        let encrypted = key.seal(id, owner, 1, b"actual secret").unwrap();
        assert_eq!(
            &**key.open(id, owner, 1, &key.id, &encrypted).unwrap(),
            b"actual secret"
        );
        assert_ne!(encrypted, key.seal(id, owner, 1, b"actual secret").unwrap());
        for (i, o, v) in [
            (Uuid::new_v4(), owner, 1),
            (id, Uuid::new_v4(), 1),
            (id, owner, 2),
        ] {
            assert!(key.open(i, o, v, &key.id, &encrypted).is_err());
        }
        for n in 0..encrypted.len() {
            let mut bad = encrypted.clone();
            bad[n] ^= 1;
            assert!(key.open(id, owner, 1, &key.id, &bad).is_err());
        }
        assert!(MasterKey::new(&[41; 32])
            .unwrap()
            .open(id, owner, 1, &key.id, &encrypted)
            .is_err());
        assert!(MasterKey::new(&[42; 31]).is_err());
    }
}
