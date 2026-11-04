//! Host maintenance for password-cleared historical users. A genuine current
//! recovery Owner authorizes a new one-time activation, never old authority.
use crate::{
    auth::{self, Auth, Context},
    config::{require_host_admin, Config},
    db::{self, Db},
    error::{Error, Result},
    identity,
    policy::{self, Policy},
    runtime::Runtime,
};
use serde_json::{json, Value};
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub struct Request {
    pub account: Uuid,
    pub confirm_account: Uuid,
    pub operation: Uuid,
    pub revision: i64,
}

/// Takes both real host/database maintenance leases; the kernel must be stopped.
/// The token is a current recovery-Owner credential, not a claimed account UUID.
pub async fn issue(config: Config, token: &str, request: Request) -> Result<Value> {
    require_host_admin()?;
    let runtime = Runtime::new(config.clone());
    let _host = runtime.host_lease()?;
    let (dsn, key) = config.secrets()?;
    let db = Db::new(dsn)?;
    db.check().await?;
    let auth = Auth::new(key)?;
    let mut c = db.connect().await?;
    let tx = c.transaction().await?;
    let held: bool = tx
        .query_one("SELECT pg_try_advisory_xact_lock($1)", &[&db::LEADER_LOCK])
        .await?
        .get(0);
    if !held {
        return Err(Error::conflict(
            "Stop the kernel before recovered-account maintenance",
        ));
    }
    db::auth_lock(&tx, true).await?;
    let ctx = auth
        .authenticate(&tx, token, "127.0.0.1".parse().unwrap(), request.operation)
        .await?;
    let result = issue_rows(&tx, &auth, &ctx, request).await?;
    tx.commit().await?;
    Ok(result)
}

async fn gate(
    c: &(impl GenericClient + Sync),
    subject: Uuid,
) -> Result<(Uuid, chrono::DateTime<chrono::Utc>)> {
    let row = c.query_opt("SELECT recovery_id,created_at FROM platform_recovery_state WHERE phase='ready' AND recovery_owner_id=$1 FOR SHARE", &[&subject]).await?.ok_or_else(||Error::conflict("Account reactivation requires the current completed recovery Owner"))?;
    Ok((row.get(0), row.get(1)))
}

async fn issue_rows(
    c: &Transaction<'_>,
    auth: &Auth,
    ctx: &Context,
    request: Request,
) -> Result<Value> {
    if request.account.is_nil()
        || request.operation.is_nil()
        || request.confirm_account != request.account
        || request.revision < 1
    {
        return Err(Error::invalid(
            "Confirm the exact account, operation and current revision",
        ));
    }
    let (recovery, imported) = gate(c, ctx.subject).await?;
    let paths = Policy::for_context(c, ctx)
        .credential(ctx.credential, 0)
        .await?;
    let rules = policy::rules(&paths);
    identity::authorize_management_snapshot(c, ctx, request.account, &rules).await?;
    if let Some(old) = c
        .query_opt(
            "SELECT owner_id,kind,resource_id FROM operations WHERE id=$1",
            &[&request.operation],
        )
        .await?
    {
        if old.get::<_, Uuid>("owner_id") != ctx.subject
            || old.get::<_, String>("kind") != "recoverAccount"
            || old.get::<_, Option<String>>("resource_id") != Some(request.account.to_string())
        {
            return Err(Error::conflict("Operation identity is already used"));
        }
        return Err(Error::new("ONE_TIME_RESPONSE_UNAVAILABLE","Activation material is not replayable; inspect the account before an explicit new request",axum::http::StatusCode::CONFLICT).with_resource(request.account));
    }
    let row = c
        .query_opt(
            "SELECT * FROM accounts WHERE id=$1 FOR UPDATE",
            &[&request.account],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    if row.get::<_, i64>("revision") != request.revision {
        return Err(Error::conflict("Account revision changed"));
    }
    if row.get::<_, String>("kind") != "user"
        || !matches!(
            row.get::<_, String>("status").as_str(),
            "disabled" | "pending"
        )
        || row.get::<_, Option<String>>("password_hash").is_some()
        || row.get::<_, chrono::DateTime<chrono::Utc>>("created_at") > imported
        || row
            .get::<_, Option<chrono::DateTime<chrono::Utc>>>("expires_at")
            .is_some_and(|at| at <= chrono::Utc::now())
        || request.account == ctx.subject
    {
        return Err(Error::conflict(
            "Only a nonarchived password-cleared historical user can be reactivated",
        ));
    }
    if c.query_opt(
        "SELECT id FROM credentials WHERE owner_id=$1 AND status='active' LIMIT 1",
        &[&request.account],
    )
    .await?
    .is_some()
    {
        return Err(Error::conflict(
            "Revoke active target credentials before reactivation",
        ));
    }
    if let Some(parent) = row.get::<_, Option<Uuid>>("parent_id") {
        auth::validate_sources(c, None, Some(parent), ctx.peer).await?;
    }
    let authority = serde_json::to_value(paths)?;
    if serde_json::to_vec(&authority)?.len() > 1048576 {
        return Err(Error::exhausted());
    }
    let (token, digest) = auth.random_token();
    let mut expires = chrono::Utc::now() + chrono::Duration::hours(1);
    let credential_bound: Option<chrono::DateTime<chrono::Utc>> = c
        .query_one(
            "SELECT min(expires_at) FROM credentials WHERE id=ANY($1)",
            &[&ctx.chain],
        )
        .await?
        .get(0);
    if let Some(bound) = credential_bound {
        expires = expires.min(bound);
    }
    if let Some(bound) = row.get::<_, Option<chrono::DateTime<chrono::Utc>>>("expires_at") {
        expires = expires.min(bound);
    }
    if expires <= chrono::Utc::now() {
        return Err(Error::credential());
    }
    c.execute("INSERT INTO operations(id,owner_id,kind,state,resource_id,created_at,updated_at) VALUES($1,$2,'recoverAccount','pending',$3,now(),now())", &[&request.operation,&ctx.subject,&request.account.to_string()]).await?;
    c.execute("INSERT INTO recovered_account_activations(account_id,operation_id,recovery_id,credential_id,subject_id,peer,authority,token_digest,expires_at,state) VALUES($1,$2,$3,$4,$5,$6::text::inet,$7,$8,$9,'pending') ON CONFLICT(account_id) DO UPDATE SET operation_id=excluded.operation_id,recovery_id=excluded.recovery_id,credential_id=excluded.credential_id,subject_id=excluded.subject_id,peer=excluded.peer,authority=excluded.authority,token_digest=excluded.token_digest,expires_at=excluded.expires_at,state='pending',updated_at=now()", &[&request.account,&request.operation,&recovery,&ctx.credential,&ctx.subject,&ctx.peer.to_string(),&authority,&digest,&expires]).await?;
    c.execute("UPDATE accounts SET status='pending',activation_digest=$2,activation_expires_at=$3,revision=revision+1,updated_at=now() WHERE id=$1", &[&request.account,&digest,&expires]).await?;
    identity::changed(c, request.account).await?;
    db::complete(
        c,
        request.operation,
        Ok(json!({"account_id":request.account,"activation_expires_at":expires})),
    )
    .await?;
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        request.operation,
        "account.manage",
        &auth::Resource::new("account", request.account, Some(request.account)).json(),
        "succeeded",
        "RECOVERED_ACCOUNT_ACTIVATION_ISSUED",
    )
    .await?;
    Ok(
        json!({"operation_id":request.operation,"account_id":request.account,"activation_token":token,"activation_expires_at":expires}),
    )
}

pub(crate) async fn authorize_activation(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    account: Uuid,
    digest: &[u8],
) -> Result<bool> {
    let Some(row) = c.query_opt("SELECT r.*,host(r.peer) AS approved_peer FROM recovered_account_activations r WHERE account_id=$1", &[&account]).await? else { return Ok(false); };
    if row.get::<_, String>("state") != "pending"
        || row.get::<_, Vec<u8>>("token_digest") != digest
        || row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at") <= chrono::Utc::now()
    {
        return Err(Error::credential());
    }
    let peer = row
        .get::<_, String>("approved_peer")
        .parse()
        .map_err(|_| Error::credential())?;
    let ctx = auth
        .resume(c, row.get("credential_id"), peer, row.get("operation_id"))
        .await?;
    if ctx.subject != row.get::<_, Uuid>("subject_id")
        || gate(c, ctx.subject).await?.0 != row.get::<_, Uuid>("recovery_id")
    {
        return Err(Error::credential());
    }
    let paths: Vec<policy::Path> =
        serde_json::from_value(row.get("authority")).map_err(|_| Error::credential())?;
    let available = policy::rules(
        &Policy::new(c, peer)
            .revalidate(&paths, ctx.credential)
            .await?,
    );
    identity::authorize_management_snapshot(c, &ctx, account, &available).await?;
    if c.query_opt(
        "SELECT id FROM credentials WHERE owner_id=$1 AND status='active' LIMIT 1",
        &[&account],
    )
    .await?
    .is_some()
    {
        return Err(Error::credential());
    }
    Ok(true)
}

pub(crate) async fn consume(
    c: &(impl GenericClient + Sync),
    account: Uuid,
    digest: &[u8],
) -> Result<()> {
    c.execute("UPDATE recovered_account_activations SET state='used',updated_at=now() WHERE account_id=$1 AND token_digest=$2 AND state='pending'", &[&account,&digest]).await?;
    Ok(())
}

#[cfg(test)]
#[path = "account_recovery_tests.rs"]
mod tests;
