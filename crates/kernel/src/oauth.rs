//! Trusted OAuth domain state. HTTP/HTML/CORS adapters must preserve these
//! typed outcomes; a token response is never a core login Cookie response.
use crate::{
    auth::{Auth, Context},
    error::{Error, Result},
    policy::Path,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use tokio_postgres::GenericClient;
use uuid::Uuid;

pub mod authorization;
pub mod clients;
pub mod snapshot;
pub mod tokens;

/// A persisted client/family binding, not caller-supplied routing context.
#[derive(Clone)]
pub struct Binding {
    pub family_id: Uuid,
    pub subject_id: Uuid,
    pub session_id: Uuid,
    pub client_id: String,
    pub client_revision: i64,
    pub origin: String,
    pub scope: String,
    pub maximum_rules: Value,
    pub fixed_paths: Vec<Path>,
    pub client_maximum_rules: Value,
    pub expires_at: DateTime<Utc>,
}

pub(crate) async fn schema_available(c: &impl GenericClient) -> Result<bool> {
    Ok(
        c.query_one("SELECT to_regclass('oauth_families') IS NOT NULL", &[])
            .await?
            .get(0),
    )
}

/// Pure persisted binding checks. Do not recursively authenticate from here:
/// Auth::validate_sources will separately traverse the trusted session root.
pub async fn binding(c: &impl GenericClient, credential: Uuid) -> Result<Binding> {
    if !schema_available(c).await? {
        return Err(Error::credential());
    }
    let r=c.query_opt("SELECT k.owner_id,k.kind,k.client_id AS token_client,k.issuer_credential_id,k.conditions,f.*,s.owner_id AS session_owner,s.kind AS session_kind,s.status AS session_status,s.expires_at AS session_expiry FROM credentials k JOIN oauth_families f ON f.id=k.family_id JOIN credentials s ON s.id=f.session_id WHERE k.id=$1", &[&credential]).await?.ok_or_else(Error::credential)?;
    let client_id: String = r.get("client_id");
    let client = c
        .query_opt(
            "SELECT spec,status,revision FROM clients WHERE client_id=$1",
            &[&client_id],
        )
        .await?
        .ok_or_else(Error::credential)?;
    let specification: Value = client.get("spec");
    clients::validate_spec(&specification).map_err(|_| Error::credential())?;
    let subject: Uuid = r.get("subject_id");
    let session: Uuid = r.get("session_id");
    let origin: String = r.get("origin");
    if !matches!(
        r.get::<_, String>("kind").as_str(),
        "oauth_access" | "oauth_refresh"
    ) || r.get::<_, Uuid>("owner_id") != subject
        || r.get::<_, Uuid>("session_owner") != subject
        || r.get::<_, Option<Uuid>>("issuer_credential_id") != Some(session)
        || r.get::<_, Option<String>>("token_client") != Some(client_id.clone())
        || r.get::<_, String>("session_kind") != "session"
        || r.get::<_, String>("session_status") != "active"
        || r.get::<_, DateTime<Utc>>("session_expiry") <= Utc::now()
        || r.get::<_, String>("status") != "active"
        || r.get::<_, DateTime<Utc>>("expires_at") <= Utc::now()
        || client.get::<_, String>("status") != "active"
        || client.get::<_, i64>("revision") != r.get::<_, i64>("client_revision")
        || !specification["allowed_origins"]
            .as_array()
            .is_some_and(|a| a.contains(&json!(origin)))
        || r.get::<_, Value>("conditions")["audiences"] != json!(["xelab.platform"])
    {
        return Err(Error::credential());
    }
    let saved: Value = r.get("fixed_paths");
    let paths: Vec<Path> = serde_json::from_value(saved).map_err(|_| Error::credential())?;
    Ok(Binding {
        family_id: r.get("id"),
        subject_id: subject,
        session_id: session,
        client_id,
        client_revision: client.get("revision"),
        origin,
        scope: r.get("scope"),
        maximum_rules: r.get("maximum_rules"),
        fixed_paths: paths,
        client_maximum_rules: specification["maximum_rules"].clone(),
        expires_at: r.get("expires_at"),
    })
}

pub async fn validate_origin(
    c: &(impl GenericClient + Sync),
    credential: Uuid,
    origin: Option<&str>,
) -> Result<()> {
    let b = binding(c, credential).await?;
    if origin.is_some_and(|origin| origin != b.origin) {
        return Err(Error::denied());
    }
    Ok(())
}

pub async fn session_context(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
) -> Result<Context> {
    let r = c
        .query_opt(
            "SELECT kind FROM credentials WHERE id=$1",
            &[&ctx.credential],
        )
        .await?
        .ok_or_else(Error::credential)?;
    if r.get::<_, String>(0) != "session" {
        return Err(Error::credential());
    }
    let live = auth
        .resume(c, ctx.credential, ctx.peer, ctx.request)
        .await?;
    if live.subject != ctx.subject {
        return Err(Error::credential());
    }
    Ok(live)
}

pub async fn revoke_family(c: &(impl GenericClient + Sync), family: Uuid) -> Result<()> {
    c.execute(
        "UPDATE oauth_families SET status='revoked',updated_at=now() WHERE id=$1",
        &[&family],
    )
    .await?;
    c.execute("UPDATE credentials SET status='revoked',revision=revision+1,updated_at=now() WHERE family_id=$1 AND kind IN ('oauth_access','oauth_refresh') AND status<>'revoked'", &[&family]).await?;
    Ok(())
}
pub async fn revoke_session(c: &(impl GenericClient + Sync), session: Uuid) -> Result<()> {
    if !schema_available(c).await? {
        return Ok(());
    }
    c.execute(
        "UPDATE oauth_families SET status='revoked',updated_at=now() WHERE session_id=$1",
        &[&session],
    )
    .await?;
    c.execute("UPDATE credentials SET status='revoked',revision=revision+1,updated_at=now() WHERE family_id IN(SELECT id FROM oauth_families WHERE session_id=$1) AND status<>'revoked'", &[&session]).await?;
    c.execute("UPDATE authorization_requests SET status='denied' WHERE session_id=$1 AND status IN ('pending','approved')", &[&session]).await?;
    Ok(())
}
pub async fn revoke_account(c: &(impl GenericClient + Sync), account: Uuid) -> Result<()> {
    if !schema_available(c).await? {
        return Ok(());
    }
    let rows=c.query("SELECT id FROM oauth_families WHERE subject_id IN(SELECT descendant_id FROM account_closure WHERE ancestor_id=$1)", &[&account]).await?;
    for row in rows {
        revoke_family(c, row.get(0)).await?;
    }
    c.execute("UPDATE authorization_requests SET status='denied' WHERE session_id IN(SELECT id FROM credentials WHERE owner_id IN(SELECT descendant_id FROM account_closure WHERE ancestor_id=$1)) AND status IN ('pending','approved')", &[&account]).await?;
    Ok(())
}

pub(crate) async fn reset_authority(c: &(impl GenericClient + Sync)) -> Result<()> {
    if schema_available(c).await? {
        c.execute(
            "UPDATE oauth_families SET status='revoked',updated_at=now()",
            &[],
        )
        .await?;
        c.execute("DELETE FROM oauth_login_intents", &[]).await?;
    }
    Ok(())
}

pub(crate) fn credential_error(error: Error) -> Error {
    if matches!(
        error.code,
        "PERMISSION_DENIED"
            | "RESOURCE_NOT_FOUND"
            | "NOT_READY"
            | "STATE_CONFLICT"
            | "INVALID_ARGUMENT"
    ) {
        Error::credential()
    } else {
        error
    }
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
