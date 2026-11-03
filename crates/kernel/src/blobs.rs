//! Bounded HTTP Blob transfers and crash-recoverable physical deletion.
use crate::{
    api::{self, App},
    auth::{self, Context, Resource},
    contracts::{self, validate},
    db,
    error::{Error, Result},
    storage::blob::Store,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_postgres::GenericClient;
use uuid::Uuid;
const MAX: u64 = 64 * 1024 * 1024;
const LIVE:&str="(expires_at>now() OR pin_count>0 OR EXISTS(SELECT 1 FROM attempts WHERE checkpoint_blob_id=blobs.id))";
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "uploadBlob" | "getBlob" | "downloadBlob" | "deleteBlob"
    )
}
fn metadata(row: &tokio_postgres::Row) -> Result<Value> {
    let mut value = db::stamp(row);
    value["owner_id"] = json!(row.get::<_, Uuid>("owner_id"));
    value["content_type"] = json!(row.get::<_, String>("content_type"));
    value["size_bytes"] = json!(row.get::<_, i64>("size_bytes"));
    value["digest"] = json!(row.get::<_, String>("digest"));
    value["expires_at"] = json!(row.get::<_, chrono::DateTime<chrono::Utc>>("expires_at"));
    validate("Blob", &value)?;
    Ok(value)
}
async fn access(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    action: &str,
    id: Uuid,
    owner: Uuid,
) -> Result<()> {
    auth::authorize(c, ctx, action, &Resource::new("blob", id, Some(owner)))
        .await
        .map_err(|e| {
            if action == "blob.read" && e.code == "PERMISSION_DENIED" {
                Error::not_found()
            } else {
                e
            }
        })
}
async fn context(
    app: &App,
    c: &(impl GenericClient + Sync),
    token: &str,
    peer: SocketAddr,
    request: Uuid,
) -> Result<Context> {
    if app.leader.is_closed() || app.shutdown.load(Ordering::SeqCst) {
        return Err(Error::unavailable("Controller leadership is unavailable"));
    }
    db::auth_lock(c, false).await?;
    app.auth.authenticate(c, token, peer.ip(), request).await
}
fn parameters(query: &str) -> Result<std::collections::HashMap<String, String>> {
    let mut params = std::collections::HashMap::new();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        if params.insert(k.into_owned(), v.into_owned()).is_some() {
            return Err(Error::invalid("Duplicate Blob query parameter"));
        }
    }
    Ok(params)
}
fn backend(row: &tokio_postgres::Row, id: Uuid) -> Result<()> {
    if row.get::<_, String>("backend_reference") != id.to_string() {
        return Err(Error::unavailable("Blob backend identity differs"));
    }
    Ok(())
}
async fn lock_key(
    c: &impl GenericClient,
    ctx: &Context,
    method: &str,
    path: &str,
    key: &str,
) -> Result<()> {
    let lock = format!("{}:{}:{method}:{path}:{key}", ctx.subject, ctx.credential);
    c.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        &[&lock],
    )
    .await?;
    Ok(())
}
#[allow(clippy::too_many_arguments)] // Fields mirror the durable idempotency envelope.
async fn record(
    c: &impl GenericClient,
    ctx: &Context,
    method: &str,
    path: &str,
    key: &str,
    fingerprint: &str,
    value: &Value,
    status: i32,
) -> Result<()> {
    c.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method=$3 AND path=$4 AND key=$5 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&method,&path,&key]).await?;
    c.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,response_status,safe_response,resource_id,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&method,&path,&key,&fingerprint,&status,&value,&value["id"].as_str()]).await?;
    Ok(())
}
async fn replay(
    c: &impl GenericClient,
    ctx: &Context,
    method: &str,
    path: &str,
    key: &str,
    fingerprint: &str,
) -> Result<Option<Value>> {
    let row=c.query_opt("SELECT fingerprint,safe_response FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method=$3 AND path=$4 AND key=$5 AND expires_at>now()",&[&ctx.subject,&ctx.credential,&method,&path,&key]).await?;
    if let Some(row) = row {
        if row.get::<_, String>(0) != fingerprint {
            return Err(Error::new(
                "IDEMPOTENCY_CONFLICT",
                "Idempotency key fingerprint differs",
                StatusCode::CONFLICT,
            ));
        }
        return Ok(Some(row.get::<_, Option<Value>>(1).ok_or_else(|| {
            Error::conflict("Blob replay metadata is unavailable")
        })?));
    }
    Ok(None)
}

async fn bound_store(app: &App) -> Result<Store> {
    const LOCK: i64 = 0x58454c424c4f4252;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&LOCK])
        .await?;
    tx.execute("INSERT INTO blob_storage_root(id,state) VALUES($1,'preparing') ON CONFLICT(singleton) DO NOTHING",&[&Uuid::new_v4()]).await?;
    tx.commit().await?; // Registration intent survives process loss before the marker.
    let tx = c.transaction().await?;
    tx.query_one("SELECT pg_advisory_xact_lock($1)", &[&LOCK])
        .await?;
    let row = tx
        .query_one("SELECT id,state FROM blob_storage_root FOR UPDATE", &[])
        .await?;
    let expected: Uuid = row.get(0);
    let creating = row.get::<_, String>(1) == "preparing";
    let root = app.config.root();
    let store = tokio::task::spawn_blocking(move || {
        let store = Store::open(&root)?;
        store.identity(expected, creating)?;
        Ok::<_, Error>(store)
    })
    .await
    .map_err(|_| Error::unavailable("Blob registration worker stopped"))??;
    tx.execute(
        "UPDATE blob_storage_root SET state='ready' WHERE id=$1",
        &[&expected],
    )
    .await?;
    tx.commit().await?;
    Ok(store)
}

pub async fn handle(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
) -> Result<Response> {
    let method = request.method().clone();
    let resource = api::operation(method.as_str(), request.uri().path())
        .and_then(|(_, p)| p.get("blob_id").cloned())
        .unwrap_or_else(|| "new".into());
    let mut actor = None;
    let result = execute(app.clone(), peer, request_id, request, &mut actor).await;
    let failure = result.as_ref().err().cloned();
    if let (Some(error), Some(ctx)) = (failure, actor) {
        let action = if method == "POST" {
            "blob.create"
        } else if method == "DELETE" {
            "blob.delete"
        } else {
            "blob.read"
        };
        if let Ok(c) = app.db.connect().await {
            if db::audit(
                &c,
                ctx.subject,
                Some(ctx.credential),
                request_id,
                action,
                &Resource::new("blob", resource, None).json(),
                if matches!(error.status.as_u16(), 401 | 403 | 404) {
                    "denied"
                } else {
                    "failed"
                },
                error.code,
            )
            .await
            .is_err()
            {
                tracing::error!(reason_code = "BLOB_AUDIT_FAILED");
            }
        }
    }
    result
}

async fn execute(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
    actor: &mut Option<Context>,
) -> Result<Response> {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path();
    if path.contains('%') || path.contains("//") {
        return Err(Error::invalid("Noncanonical API path"));
    }
    let (op, params) = api::operation(parts.method.as_str(), path).ok_or_else(Error::not_found)?;
    let name = op["operationId"].as_str().unwrap();
    let headers = &parts.headers;
    if let Some(origin) = headers.get("origin") {
        if origin.to_str().ok() != Some(app.config.text("server", "public_origin")) {
            return Err(Error::denied());
        }
    }
    let mutating = parts.method == "POST" || parts.method == "DELETE";
    if mutating
        && !headers.contains_key("authorization")
        && headers.get("origin").and_then(|v| v.to_str().ok())
            != Some(app.config.text("server", "public_origin"))
    {
        return Err(Error::denied());
    }
    let token = api::bearer(headers)?.to_string();
    let args = parameters(parts.uri.query().unwrap_or(""))?;
    for parameter in op["parameters"].as_array().unwrap() {
        if parameter["in"] == "header" && parameter["required"] == true {
            let field = parameter["name"].as_str().unwrap();
            if field.eq_ignore_ascii_case("if-match") && !headers.contains_key("if-match") {
                return Err(Error::new(
                    "PRECONDITION_REQUIRED",
                    "If-Match is required",
                    StatusCode::PRECONDITION_REQUIRED,
                ));
            }
            let raw = api::header(headers, field)?;
            let value = if parameter["schema"]["type"] == "integer" {
                json!(raw
                    .parse::<u64>()
                    .map_err(|_| Error::invalid("Invalid Blob length"))?)
            } else {
                json!(raw)
            };
            contracts::validate_business(&parameter["schema"], &value)?;
        }
    }
    let key = if mutating {
        api::header(headers, "idempotency-key")?
    } else {
        ""
    };
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    let ctx = context(&app, &tx, &token, peer, request_id).await?;
    *actor = Some(ctx.clone());
    if name == "uploadBlob" {
        if args.len() != 1 || !args.contains_key("content_type") {
            return Err(Error::invalid(
                "Blob content_type is required; other query parameters are not supported",
            ));
        }
        let mime = &args["content_type"];
        if mime.is_empty()
            || mime.len() > 128
            || !mime.is_ascii()
            || mime.bytes().any(|b| b.is_ascii_control())
        {
            return Err(Error::invalid("Invalid Blob content type"));
        }
        if api::header(headers, "content-type")? != "application/octet-stream"
            || headers.contains_key("content-encoding")
            || headers.contains_key("transfer-encoding")
        {
            return Err(Error::new(
                "UNSUPPORTED_MEDIA_TYPE",
                "Expected a binary Blob with an exact Content-Length",
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ));
        }
        auth::authorize(
            &tx,
            &ctx,
            "blob.create",
            &Resource::new("blob", "new", Some(ctx.subject)),
        )
        .await?;
        tx.commit().await?;
        let size = api::header(headers, "content-length")?
            .parse::<u64>()
            .map_err(|_| Error::invalid("Invalid Blob length"))?;
        if size > MAX {
            return Err(Error::invalid("Blob exceeds 64 MiB"));
        }
        let expected = api::header(headers, "x-content-sha256")?;
        let _permit = app
            .blob_uploads
            .try_acquire()
            .map_err(|_| Error::exhausted())?;
        let store = bound_store(&app).await?;
        let mut staged = tokio::task::spawn_blocking(move || store.stage(size))
            .await
            .map_err(|_| Error::unavailable("Blob upload worker stopped"))??;
        let mut stream = body.into_data_stream();
        let mut length = 0u64;
        let mut hash = Sha256::new();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.map_err(|_| Error::invalid("Blob upload was interrupted"))?;
                length = length
                    .checked_add(bytes.len() as u64)
                    .ok_or_else(Error::exhausted)?;
                if length > size {
                    return Err(Error::invalid("Blob length differs"));
                }
                hash.update(&bytes);
                staged.file.as_mut().unwrap().write_all(&bytes).await?;
            }
            if length != size {
                return Err(Error::invalid("Blob length differs"));
            }
            Ok(())
        })
        .await
        .map_err(|_| {
            Error::new(
                "DEADLINE_EXCEEDED",
                "Blob upload deadline exceeded",
                StatusCode::GATEWAY_TIMEOUT,
            )
        })??;
        if format!("sha256:{:x}", hash.finalize()) != expected {
            return Err(Error::invalid("Blob digest differs"));
        }
        let fingerprint = contracts::digest(&serde_json::to_vec(
            &json!({"size":size,"digest":expected,"content_type":mime}),
        )?);
        let tx = c.transaction().await?;
        let ctx = context(&app, &tx, &token, peer, request_id).await?;
        auth::authorize(
            &tx,
            &ctx,
            "blob.create",
            &Resource::new("blob", "new", Some(ctx.subject)),
        )
        .await?;
        lock_key(&tx, &ctx, "POST", path, key).await?;
        if let Some(value) = replay(&tx, &ctx, "POST", path, key, &fingerprint).await? {
            let id = contracts::id(&value, "id")?;
            if tx
                .query_opt(
                    &format!("SELECT id FROM blobs WHERE id=$1 AND {LIVE}"),
                    &[&id],
                )
                .await?
                .is_none()
            {
                return Err(Error::conflict(
                    "The original Blob has been deleted or expired",
                ));
            }
            return Ok(api::response(value, 201, request_id));
        }
        let id = Uuid::new_v4();
        tx.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended($1, $2))",
            &[&id.to_string(), &0x58454c424c4f4246_i64],
        )
        .await?;
        staged.publish(id).await?;
        tx.execute("INSERT INTO blobs(id,owner_id,content_type,size_bytes,digest,backend_reference,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,now()+interval '24 hours',now(),now())",&[&id,&ctx.subject,&mime,&(size as i64),&expected,&id.to_string()]).await?;
        let value = metadata(
            &tx.query_one("SELECT * FROM blobs WHERE id=$1", &[&id])
                .await?,
        )?;
        record(&tx, &ctx, "POST", path, key, &fingerprint, &value, 201).await?;
        db::audit(
            &tx,
            ctx.subject,
            Some(ctx.credential),
            request_id,
            "blob.create",
            &Resource::new("blob", id, Some(ctx.subject)).json(),
            "succeeded",
            "COMMITTED",
        )
        .await?;
        if app.leader.is_closed() {
            return Err(Error::unavailable("Controller leadership is lost"));
        }
        tx.commit().await?;
        return Ok(api::response(value, 201, request_id));
    }
    if !args.is_empty() {
        return Err(Error::invalid("Unsupported Blob query parameter"));
    }
    let bytes = tokio::time::timeout(Duration::from_secs(1), axum::body::to_bytes(body, 1))
        .await
        .map_err(|_| Error::invalid("Unexpected Blob request body"))?
        .map_err(|_| Error::invalid("Unexpected Blob request body"))?;
    if !bytes.is_empty() {
        return Err(Error::invalid("Unexpected Blob request body"));
    }
    let id = api::path_id(&params, "blob_id")?;
    if name == "deleteBlob" {
        let fingerprint = contracts::digest(api::header(headers, "if-match")?.as_bytes());
        lock_key(&tx, &ctx, "DELETE", path, key).await?;
        if let Some(value) = replay(&tx, &ctx, "DELETE", path, key, &fingerprint).await? {
            access(
                &tx,
                &ctx,
                "blob.delete",
                id,
                contracts::id(&value, "owner_id")?,
            )
            .await?;
            return Ok(api::response(Value::Null, 204, request_id));
        }
        let row = tx
            .query_opt("SELECT * FROM blobs WHERE id=$1 FOR UPDATE", &[&id])
            .await?
            .ok_or_else(Error::not_found)?;
        access(&tx, &ctx, "blob.delete", id, row.get("owner_id")).await?;
        api::etag(headers, row.get("revision"))?;
        backend(&row, id)?;
        if row.get::<_, i64>("pin_count") > 0
            || tx
                .query_opt(
                    "SELECT id FROM attempts WHERE checkpoint_blob_id=$1 LIMIT 1",
                    &[&id],
                )
                .await?
                .is_some()
        {
            return Err(Error::conflict("Blob is pinned by active data"));
        }
        let value = metadata(&row)?;
        tx.execute(
            "INSERT INTO blob_deletions(id,backend_reference) VALUES($1,$2)",
            &[&id, &id.to_string()],
        )
        .await?;
        tx.execute("DELETE FROM blobs WHERE id=$1", &[&id]).await?;
        record(&tx, &ctx, "DELETE", path, key, &fingerprint, &value, 204).await?;
        db::audit(
            &tx,
            ctx.subject,
            Some(ctx.credential),
            request_id,
            "blob.delete",
            &Resource::new("blob", id, Some(row.get("owner_id"))).json(),
            "succeeded",
            "COMMITTED",
        )
        .await?;
        if app.leader.is_closed() {
            return Err(Error::unavailable("Controller leadership is lost"));
        }
        tx.commit().await?;
        return Ok(api::response(Value::Null, 204, request_id));
    }
    let row = tx
        .query_opt(
            &format!("SELECT * FROM blobs WHERE id=$1 AND {LIVE}"),
            &[&id],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    access(&tx, &ctx, "blob.read", id, row.get("owner_id")).await?;
    backend(&row, id)?;
    let value = metadata(&row)?;
    tx.commit().await?;
    if name == "getBlob" {
        return Ok(api::response(value, 200, request_id));
    }
    let store = bound_store(&app).await?;
    let size = row.get::<_, i64>("size_bytes") as u64;
    let expected = row.get::<_, String>("digest");
    let file = tokio::task::spawn_blocking(move || store.verify(id, size, &expected))
        .await
        .map_err(|_| Error::unavailable("Blob read worker stopped"))??;
    let state = (
        tokio::fs::File::from_std(file),
        app,
        token,
        peer,
        request_id,
        id,
        size,
        tokio::time::Instant::now(),
    );
    let stream = futures_util::stream::try_unfold(
        state,
        |(mut file, app, token, peer, request, id, remaining, started)| async move {
            if remaining == 0 {
                return Ok::<_, Error>(None);
            }
            if started.elapsed() > Duration::from_secs(300) {
                return Err(Error::unavailable("Blob download deadline exceeded"));
            }
            let mut c = app.db.connect().await?;
            let tx = c.transaction().await?;
            let ctx = context(&app, &tx, &token, peer, request).await?;
            let row = tx
                .query_opt(
                    &format!("SELECT owner_id FROM blobs WHERE id=$1 AND {LIVE}"),
                    &[&id],
                )
                .await?
                .ok_or_else(Error::not_found)?;
            access(&tx, &ctx, "blob.read", id, row.get(0)).await?;
            tx.commit().await?;
            let mut bytes = vec![0; remaining.min(65536) as usize];
            let n = file.read(&mut bytes).await?;
            if n == 0 {
                return Err(Error::unavailable("Blob download was interrupted"));
            }
            bytes.truncate(n);
            Ok(Some((
                bytes,
                (
                    file,
                    app,
                    token,
                    peer,
                    request,
                    id,
                    remaining - n as u64,
                    started,
                ),
            )))
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    for (name, value) in [
        ("content-type", "application/octet-stream".to_string()),
        ("content-length", size.to_string()),
        (
            "content-disposition",
            format!("attachment; filename=\"{id}\""),
        ),
        ("x-content-type-options", "nosniff".to_string()),
        ("x-content-sha256", row.get::<_, String>("digest")),
        ("cache-control", "no-store".to_string()),
        ("x-request-id", request_id.to_string()),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    Ok(response)
}

pub async fn collect(app: &App) -> Result<()> {
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
    let store = bound_store(app).await?;
    let mut c = app.db.connect().await?;
    let tx = c.transaction().await?;
    let locked: bool = tx
        .query_one(
            "SELECT pg_try_advisory_xact_lock($1)",
            &[&0x58454c424c4f4247_i64],
        )
        .await?
        .get(0);
    if !locked {
        return Ok(());
    }
    let rows=tx.query("SELECT id,backend_reference FROM blobs WHERE expires_at<=now() AND pin_count=0 AND NOT EXISTS(SELECT 1 FROM attempts WHERE checkpoint_blob_id=blobs.id) ORDER BY expires_at,id LIMIT 16 FOR UPDATE SKIP LOCKED",&[]).await?;
    for row in rows {
        let id: Uuid = row.get(0);
        backend(&row, id)?;
        tx.execute(
            "INSERT INTO blob_deletions(id,backend_reference) VALUES($1,$2)",
            &[&id, &id.to_string()],
        )
        .await?;
        tx.execute("DELETE FROM blobs WHERE id=$1", &[&id]).await?;
    }
    tx.commit().await?;
    for _ in 0..16 {
        let tx = c.transaction().await?;
        let Some(row)=tx.query_opt("SELECT id,backend_reference FROM blob_deletions ORDER BY created_at,id LIMIT 1 FOR UPDATE SKIP LOCKED",&[]).await? else{break};
        let id: Uuid = row.get(0);
        backend(&row, id)?;
        if tx
            .query_opt(
                "SELECT id FROM blobs WHERE id=$1 OR backend_reference=$2",
                &[&id, &id.to_string()],
            )
            .await?
            .is_some()
        {
            return Err(Error::conflict(
                "Blob deletion conflicts with live metadata",
            ));
        }
        let store = store.clone();
        let removed = tokio::task::spawn_blocking(move || store.remove(id))
            .await
            .map_err(|_| Error::unavailable("Blob deletion worker stopped"))??;
        if !removed {
            break;
        }
        tx.execute("DELETE FROM blob_deletions WHERE id=$1", &[&id])
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
pub async fn collect_orphans(app: &App, cursor: &mut Option<std::fs::ReadDir>) -> Result<()> {
    if app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
    let store = bound_store(app).await?;
    if cursor.is_none() {
        *cursor = Some(store.entries()?);
    }
    let mut c = app.db.connect().await?;
    for _ in 0..16 {
        let Some(entry) = cursor.as_mut().unwrap().next() else {
            *cursor = None;
            break;
        };
        let store = store.clone();
        let candidate = tokio::task::spawn_blocking(move || store.orphan(entry?))
            .await
            .map_err(|_| Error::unavailable("Blob scan worker stopped"))??;
        if let Some(candidate) = candidate {
            let tx = c.transaction().await?;
            if let Some(id) = candidate.id {
                // Filesystem locks alone do not fence an in-flight COMMIT after
                // a client disconnect. The publisher holds this database lock
                // until PostgreSQL has decided commit versus rollback.
                let locked: bool = tx
                    .query_one(
                        "SELECT pg_try_advisory_xact_lock(hashtextextended($1,$2))",
                        &[&id.to_string(), &0x58454c424c4f4246_i64],
                    )
                    .await?
                    .get(0);
                if !locked {
                    continue;
                }
                if tx.query_opt("SELECT id FROM blobs WHERE id=$1 OR backend_reference=$2 UNION ALL SELECT id FROM blob_deletions WHERE id=$1 OR backend_reference=$2 LIMIT 1",&[&id,&id.to_string()]).await?.is_some(){continue;}
            }
            if app.leader.is_closed() {
                return Err(Error::unavailable("Controller leadership is lost"));
            }
            tokio::task::spawn_blocking(move || candidate.remove())
                .await
                .map_err(|_| Error::unavailable("Blob scan worker stopped"))??;
            tx.commit().await?;
        }
    }
    Ok(())
}
pub async fn run(app: Arc<App>) {
    let mut cursor = None;
    while !app.shutdown.load(Ordering::SeqCst) && !app.leader.is_closed() {
        if let Err(error) = collect(&app).await {
            tracing::warn!(reason_code = error.code, "Blob cleanup is incomplete");
        }
        if let Err(error) = collect_orphans(&app, &mut cursor).await {
            tracing::warn!(
                reason_code = error.code,
                "Blob orphan cleanup is incomplete"
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
