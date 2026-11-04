//! Single-use authorization codes and rotating refresh families. Logical OAuth
//! failures are committed outcomes; infrastructure errors alone roll back.
use super::{
    authorization::{live_paths, validate_client, AuthorizationInput},
    binding, clients, credential_error, revoke_family,
};
use crate::{
    auth::{Auth, Context},
    contracts::{string, validate},
    db::{self, Db},
    error::{Error, Result},
    policy::{self, Path},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{net::IpAddr, time::Duration};
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub enum ExchangeOutcome {
    Token(Value),
    Rejected { status: u16, body: Value },
}
fn rejected(code: &str, status: u16) -> ExchangeOutcome {
    ExchangeOutcome::Rejected {
        status,
        body: json!({"error":code}),
    }
}

/// Own the transaction so a refresh replay revocation cannot be accidentally
/// rolled back by an HTTP error adapter or ordinary idempotency savepoint.
pub async fn exchange(
    db: &Db,
    auth: &Auth,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    origin: Option<&str>,
) -> Result<ExchangeOutcome> {
    if validate("OAuthTokenRequest", input).is_err() {
        return Ok(rejected("invalid_request", 400));
    }
    let work = async {
        let mut c = db.connect_scoped(Duration::from_secs(10)).await?;
        let tx = c.transaction().await?;
        db::auth_lock(&tx, true).await?;
        let result = exchange_locked(&tx, auth, peer, request_id, input, origin).await?;
        tx.commit().await?;
        Ok(result)
    };
    tokio::time::timeout(Duration::from_secs(10), work)
        .await
        .unwrap_or_else(|_| Err(Error::unavailable("OAuth transaction deadline elapsed")))
}
async fn exchange_locked(
    tx: &Transaction<'_>,
    auth: &Auth,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    origin: Option<&str>,
) -> Result<ExchangeOutcome> {
    let public = string(input, "client_id")?;
    let client = match clients::by_public_id(tx, public).await {
        Ok(c) if c.status == "active" => c,
        Ok(_) => return Ok(rejected("invalid_client", 401)),
        Err(e) if e.code == "RESOURCE_NOT_FOUND" => return Ok(rejected("invalid_client", 401)),
        Err(e) => return Err(e),
    };
    if origin.is_some_and(|o| {
        !client.spec["allowed_origins"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(o)))
    }) {
        return Ok(rejected("invalid_client", 401));
    }
    if input["grant_type"] == "authorization_code" {
        exchange_code(tx, auth, peer, request_id, input, &client, origin).await
    } else {
        exchange_refresh(tx, auth, peer, request_id, input, &client, origin).await
    }
}
async fn trusted_session(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    session: Uuid,
    peer: IpAddr,
    request: Uuid,
) -> Result<Context> {
    let row = c
        .query_opt("SELECT kind FROM credentials WHERE id=$1", &[&session])
        .await?
        .ok_or_else(Error::credential)?;
    if row.get::<_, String>(0) != "session" {
        return Err(Error::credential());
    }
    auth.resume(c, session, peer, request).await
}
fn grant_failure(error: Error) -> Result<ExchangeOutcome> {
    if matches!(
        error.code,
        "INVALID_CREDENTIAL"
            | "PERMISSION_DENIED"
            | "RESOURCE_NOT_FOUND"
            | "STATE_CONFLICT"
            | "INVALID_ARGUMENT"
    ) {
        Ok(rejected("invalid_grant", 400))
    } else {
        Err(error)
    }
}
async fn exchange_code(
    tx: &Transaction<'_>,
    auth: &Auth,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    client: &clients::Client,
    origin: Option<&str>,
) -> Result<ExchangeOutcome> {
    let row = tx
        .query_opt(
            "SELECT * FROM authorization_requests WHERE code_digest=$1 FOR UPDATE",
            &[&auth.token_digest(string(input, "code")?)],
        )
        .await?;
    let Some(row) = row else {
        return Ok(rejected("invalid_grant", 400));
    };
    if !auth.verify_mac(
        string(input, "code")?,
        &row.get::<_, Vec<u8>>("code_digest"),
    ) || row.get::<_, String>("status") != "approved"
        || row.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
        || row.get::<_, String>("hmac_key_id") != auth.key_id.as_str()
        || row.get::<_, String>("client_id") != client.client_id
        || row.get::<_, i64>("client_revision") != client.revision
        || row.get::<_, String>("redirect_uri") != string(input, "redirect_uri")?
        || row.get::<_, String>("code_challenge")
            != URL_SAFE_NO_PAD.encode(Sha256::digest(string(input, "code_verifier")?.as_bytes()))
        || origin.is_some_and(|o| o != row.get::<_, String>("origin"))
    {
        return Ok(rejected("invalid_grant", 400));
    }
    let intent = AuthorizationInput {
        client_id: client.client_id.clone(),
        redirect_uri: row.get("redirect_uri"),
        state: row.get("state"),
        code_challenge: row.get("code_challenge"),
        scope: row.get("scope"),
    };
    if let Err(e) = validate_client(tx, &intent).await {
        return grant_failure(credential_error(e));
    }
    let session = row.get("session_id");
    let ctx = match trusted_session(tx, auth, session, peer, request_id).await {
        Ok(c) => c,
        Err(e) => return grant_failure(e),
    };
    let paths = match live_paths(
        tx,
        &ctx,
        &row.get::<_, Value>("fixed_paths"),
        &row.get::<_, Value>("fixed_rules"),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return grant_failure(e),
    };
    if !row
        .get::<_, Value>("fixed_rules")
        .as_array()
        .is_some_and(Vec::is_empty)
        && paths.is_empty()
    {
        return Ok(rejected("invalid_grant", 400));
    }
    let expiry = source_expiry(tx, &ctx).await?;
    if expiry <= Utc::now() {
        return Ok(rejected("invalid_grant", 400));
    }
    let family = Uuid::new_v4();
    tx.execute("INSERT INTO oauth_families(id,subject_id,session_id,client_id,client_revision,origin,scope,maximum_rules,fixed_paths,expires_at,status,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'active',now(),now())", &[&family,&ctx.subject,&session,&client.client_id,&client.revision,&row.get::<_,String>("origin"),&row.get::<_,String>("scope"),&policy::rules(&paths),&serde_json::to_value(&paths)?,&expiry]).await?;
    tx.execute(
        "UPDATE authorization_requests SET status='consumed' WHERE id=$1",
        &[&row.get::<_, Uuid>("id")],
    )
    .await?;
    let response = issue(
        tx,
        auth,
        &ctx,
        client,
        family,
        &row.get::<_, String>("scope"),
        expiry,
        &paths,
    )
    .await?;
    audit(tx, &ctx, client.id, "OAUTH_CODE_CONSUMED", "succeeded").await?;
    Ok(ExchangeOutcome::Token(response.0))
}

async fn exchange_refresh(
    tx: &Transaction<'_>,
    auth: &Auth,
    peer: IpAddr,
    request_id: Uuid,
    input: &Value,
    client: &clients::Client,
    origin: Option<&str>,
) -> Result<ExchangeOutcome> {
    // Lookup reveals only the stable family ID. No used-state decision or
    // credential row lock may precede the family lock.
    let row=tx.query_opt("SELECT k.id,k.family_id FROM credentials k JOIN oauth_refresh_tokens r ON r.credential_id=k.id AND r.family_id=k.family_id WHERE k.kind='oauth_refresh' AND k.hmac_key_id=$1 AND k.verification_digest=$2", &[&auth.key_id.as_str(),&auth.token_digest(string(input,"refresh_token")?)]).await?;
    let Some(row) = row else {
        return Ok(rejected("invalid_grant", 400));
    };
    let old: Uuid = row.get("id");
    let family: Uuid = row.get("family_id");
    let f = tx
        .query_opt(
            "SELECT * FROM oauth_families WHERE id=$1 FOR UPDATE",
            &[&family],
        )
        .await?
        .ok_or_else(Error::credential)?;
    if f.get::<_, String>("client_id") != client.client_id
        || f.get::<_, i64>("client_revision") != client.revision
        || f.get::<_, String>("status") != "active"
        || f.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
        || origin.is_some_and(|o| o != f.get::<_, String>("origin"))
    {
        return Ok(rejected("invalid_grant", 400));
    }
    let oldrow=tx.query_one("SELECT k.*,r.used_at FROM credentials k JOIN oauth_refresh_tokens r ON r.credential_id=k.id WHERE k.id=$1 FOR UPDATE OF k,r", &[&old]).await?;
    if !auth.verify_mac(
        string(input, "refresh_token")?,
        &oldrow.get::<_, Vec<u8>>("verification_digest"),
    ) {
        return Ok(rejected("invalid_grant", 400));
    }
    if oldrow.get::<_, Uuid>("owner_id") != f.get::<_, Uuid>("subject_id")
        || oldrow.get::<_, Option<Uuid>>("issuer_credential_id") != Some(f.get("session_id"))
        || oldrow.get::<_, Option<String>>("client_id") != Some(client.client_id.clone())
    {
        return Ok(rejected("invalid_grant", 400));
    }
    if oldrow.get::<_, Option<DateTime<Utc>>>("used_at").is_some() {
        revoke_family(tx, family).await?;
        // Reuse also fences a family whose original grant has since been
        // withdrawn. Recording the persisted subject/session is an audit of
        // the rejected secret, not a fresh authentication or authorization.
        db::audit(
            tx,
            f.get("subject_id"),
            Some(f.get("session_id")),
            request_id,
            "oauth.token",
            &json!({"kind":"client","match":"ids","ids":[client.id]}),
            "failed",
            "OAUTH_REFRESH_REUSE",
        )
        .await?;
        // This is Ok(Rejected), so exchange() COMMITs the family revocation.
        return Ok(rejected("invalid_grant", 400));
    }
    let ctx = match trusted_session(tx, auth, f.get("session_id"), peer, request_id).await {
        Ok(c) => c,
        Err(e) => return grant_failure(e),
    };
    if oldrow.get::<_, String>("status") != "active"
        || oldrow.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
    {
        return Ok(rejected("invalid_grant", 400));
    }
    if let Err(e) = binding(tx, old).await {
        return grant_failure(e);
    }
    let paths = match live_paths(
        tx,
        &ctx,
        &f.get::<_, Value>("fixed_paths"),
        &f.get::<_, Value>("maximum_rules"),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return grant_failure(e),
    };
    if !f
        .get::<_, Value>("maximum_rules")
        .as_array()
        .is_some_and(Vec::is_empty)
        && paths.is_empty()
    {
        return Ok(rejected("invalid_grant", 400));
    }
    let expiry = f
        .get::<_, DateTime<Utc>>("expires_at")
        .min(source_expiry(tx, &ctx).await?);
    let (response, replacement) = issue(
        tx,
        auth,
        &ctx,
        client,
        family,
        &f.get::<_, String>("scope"),
        expiry,
        &paths,
    )
    .await?;
    tx.execute("UPDATE oauth_refresh_tokens SET used_at=now(),replacement_id=$2 WHERE credential_id=$1 AND used_at IS NULL", &[&old,&replacement]).await?;
    tx.execute(
        "UPDATE credentials SET status='revoked',revision=revision+1,updated_at=now() WHERE id=$1",
        &[&old],
    )
    .await?;
    audit(tx, &ctx, client.id, "OAUTH_REFRESH_ROTATED", "succeeded").await?;
    Ok(ExchangeOutcome::Token(response))
}

async fn source_expiry(c: &(impl GenericClient + Sync), ctx: &Context) -> Result<DateTime<Utc>> {
    let mut expiry = Utc::now() + chrono::Duration::hours(24);
    for row in c
        .query(
            "SELECT expires_at FROM credentials WHERE id=ANY($1)",
            &[&ctx.chain],
        )
        .await?
    {
        expiry = expiry.min(row.get(0));
    }
    for row in c.query("SELECT a.expires_at FROM accounts a JOIN account_closure t ON t.ancestor_id=a.id WHERE t.descendant_id=$1 AND a.expires_at IS NOT NULL", &[&ctx.subject]).await? {expiry=expiry.min(row.get(0));}
    Ok(expiry)
}
#[allow(clippy::too_many_arguments)] // All independently checked subject, client, family and scope bindings are explicit.
async fn issue(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    client: &clients::Client,
    family: Uuid,
    scope: &str,
    refresh_expiry: DateTime<Utc>,
    paths: &[Path],
) -> Result<(Value, Uuid)> {
    let mut access = String::new();
    let mut refresh = String::new();
    let mut refresh_id = Uuid::nil();
    for kind in ["oauth_access", "oauth_refresh"] {
        let id = Uuid::new_v4();
        let (token, digest) = auth.random_token();
        let expiry = if kind == "oauth_access" {
            Utc::now() + chrono::Duration::seconds(900)
        } else {
            refresh_expiry
        };
        let maximum = policy::rules(paths);
        c.execute("INSERT INTO credentials(id,owner_id,kind,hmac_key_id,verification_digest,issuer_credential_id,client_id,family_id,status,inheritance_mode,maximum_rules,conditions,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'active','selected',$9,$10,$11,now(),now())", &[&id,&ctx.subject,&kind,&auth.key_id.as_str(),&digest,&ctx.credential,&client.client_id,&family,&maximum,&json!({"audiences":["xelab.platform"]}),&expiry]).await?;
        policy::save_sources(c, paths, None, Some(id)).await?;
        if kind == "oauth_access" {
            access = token;
        } else {
            refresh = token;
            refresh_id = id;
            c.execute("INSERT INTO oauth_refresh_tokens(credential_id,family_id,created_at) VALUES($1,$2,now())", &[&id,&family]).await?;
        }
    }
    let value = json!({"access_token":access,"token_type":"Bearer","expires_in":900,"refresh_token":refresh,"scope":scope});
    validate("OAuthToken", &value)?;
    Ok((value, refresh_id))
}
async fn audit(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    client: Uuid,
    code: &str,
    outcome: &str,
) -> Result<()> {
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        ctx.request,
        "oauth.token",
        &json!({"kind":"client","match":"ids","ids":[client]}),
        outcome,
        code,
    )
    .await
}
