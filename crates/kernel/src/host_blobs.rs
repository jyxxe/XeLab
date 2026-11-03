//! Blob proxy authority is the intersection of a live call, its original source
//! alternatives, the signed workload and its independently approved proxy cap.
use crate::{
    api::App,
    auth::{self, Context, Resource},
    contracts, db,
    error::{Error, Result},
    policy::{self, Path, Policy},
    rpc,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    net::IpAddr,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio_postgres::GenericClient;
use uuid::Uuid;

fn caps(rules: &Value) -> Result<Vec<Path>> {
    rules
        .as_array()
        .ok_or_else(Error::denied)?
        .iter()
        .map(|rule| {
            Ok(Path {
                rule: rule.clone(),
                sources: Default::default(),
            })
        })
        .collect()
}
fn saved(value: Value) -> Result<Vec<Path>> {
    serde_json::from_value(value)
        .map_err(|_| Error::unavailable("Stored proxy source paths are invalid"))
}
fn requested(manifest: &Value, action: &str) -> bool {
    manifest["requested_rules"].as_array().is_some_and(|rs| {
        rs.iter().any(|r| {
            r["required"] == false
                && r["rule"]["action"] == action
                && r["rule"]["resource"] == json!({"kind":"blob","match":"all"})
                && r["rule"].get("conditions").is_none_or(|v| v == &json!({}))
        })
    })
}
pub fn supported(approval: &Value) -> bool {
    approval["egress"] == json!([])
        && approval["rules"].as_array().is_some_and(|rs| {
            rs.iter().all(|r| {
                matches!(r["action"].as_str(), Some("blob.read" | "blob.create"))
                    && r["resource"]["kind"] == "blob"
                    && r.get("conditions").is_none_or(|v| v == &json!({}))
            })
        })
}
pub async fn approve(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    deployment: Uuid,
    manifest: &Value,
    approval: &Value,
    owner: Uuid,
) -> Result<()> {
    if !supported(approval)
        || approval["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| !requested(manifest, r["action"].as_str().unwrap()))
    {
        return Err(Error::invalid(
            "Only explicitly requested Blob proxy rules with no egress are supported",
        ));
    }
    if approval["rules"] == json!([]) {
        c.execute(
            "DELETE FROM host_proxy_approvals WHERE deployment_id=$1",
            &[&deployment],
        )
        .await?;
        return Ok(());
    }
    let mut p = Policy::new(c, ctx.peer);
    p.require_subset(ctx, approval["rules"].as_array().unwrap())
        .await?;
    let mut bounds = approval["rules"].as_array().unwrap().clone();
    bounds.push(json!({"action":"plugin.permission.grant","resource":{"kind":"deployment","match":"ids","ids":[deployment]}}));
    auth::authorize(
        c,
        ctx,
        "plugin.permission.grant",
        &Resource::new("deployment", deployment, Some(owner)),
    )
    .await?;
    let current = p.credential(ctx.credential, 0).await?;
    let source_paths = p.intersection(&current, &caps(&json!(bounds))?).await?;
    let peer = ctx.peer.to_string();
    let digest = contracts::digest(&serde_json::to_vec(approval)?);
    c.execute("INSERT INTO host_proxy_approvals(deployment_id,credential_id,peer,approval_digest,source_paths) VALUES($1,$2,$3::text::inet,$4,$5) ON CONFLICT(deployment_id) DO UPDATE SET credential_id=$2,peer=$3::text::inet,approval_digest=$4,source_paths=$5,created_at=now()",&[&deployment,&ctx.credential,&peer,&digest,&json!(source_paths)]).await?;
    Ok(())
}
async fn approval_sources(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
) -> Result<(Value, Value, IpAddr)> {
    let row=c.query_opt("SELECT d.approval,d.owner_id,a.credential_id,host(a.peer) AS peer,a.approval_digest,a.source_paths FROM deployments d JOIN host_proxy_approvals a ON a.deployment_id=d.id WHERE d.id=$1",&[&deployment]).await?.ok_or_else(Error::denied)?;
    let approval: Value = row.get("approval");
    if !supported(&approval)
        || contracts::digest(&serde_json::to_vec(&approval)?)
            != row.get::<_, String>("approval_digest")
        || approval["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| !requested(manifest, r["action"].as_str().unwrap()))
    {
        return Err(Error::denied());
    }
    let peer: IpAddr = row
        .get::<_, String>("peer")
        .parse()
        .map_err(|_| Error::denied())?;
    let ctx = app
        .auth
        .resume(c, row.get("credential_id"), peer, Uuid::new_v4())
        .await?;
    let paths = Policy::new(c, peer)
        .revalidate(&saved(row.get("source_paths"))?, ctx.credential)
        .await?;
    let live = policy::rules(&paths);
    if !auth::rules_allow(
        &live,
        "plugin.permission.grant",
        &Resource::new("deployment", deployment, Some(row.get("owner_id"))),
        peer,
    ) {
        return Err(Error::denied());
    }
    Ok((approval, live, peer))
}
pub async fn check_approval(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
) -> Result<()> {
    let approval: Value = c
        .query_one(
            "SELECT approval FROM deployments WHERE id=$1",
            &[&deployment],
        )
        .await?
        .get(0);
    if approval == json!({"rules":[],"egress":[]}) {
        return Ok(());
    }
    approval_sources(c, app, deployment, manifest)
        .await
        .map(|_| ())
}
async fn proxy_access(
    c: &(impl GenericClient + Sync),
    app: &App,
    deployment: Uuid,
    manifest: &Value,
    action: &str,
    resource: &Resource,
    caller_peer: IpAddr,
) -> Result<()> {
    let (approval, live, approved_peer) = approval_sources(c, app, deployment, manifest).await?;
    if !requested(manifest, action)
        || !auth::rules_allow(&approval["rules"], action, resource, caller_peer)
        || !auth::rules_allow(&live, action, resource, approved_peer)
    {
        return Err(Error::denied());
    }
    Ok(())
}
// Supported resource markers have one structural meaning. Reject annotations
// under unions/conditionals instead of granting every branch's identifiers.
pub fn resources(schema: &Value, input: &Value, out: &mut BTreeSet<Uuid>) -> Result<()> {
    if schema.get("x-xelab-resource-kind").is_some() {
        if schema["x-xelab-resource-kind"] != "blob" || schema["type"] != "string" {
            return Err(Error::invalid("Unsupported proxy resource marker"));
        }
        let raw = input
            .as_str()
            .ok_or_else(|| Error::invalid("Blob resource must be a UUID string"))?;
        let id: Uuid = raw
            .parse()
            .map_err(|_| Error::invalid("Blob resource must be a UUID"))?;
        if id.to_string() != raw {
            return Err(Error::invalid("Invalid Blob resource scope"));
        }
        out.insert(id);
        if out.len() > 256 {
            return Err(Error::invalid("At most 256 source Blobs per call"));
        }
    }
    for keyword in [
        "anyOf",
        "oneOf",
        "allOf",
        "if",
        "then",
        "else",
        "not",
        "$ref",
        "patternProperties",
        "dependentSchemas",
    ] {
        if schema.get(keyword).is_some_and(has_resource_marker) {
            return Err(Error::invalid("Ambiguous proxy resource schema"));
        }
    }
    if let Some(object) = input.as_object() {
        for (key, value) in object {
            if let Some(child) = schema["properties"].get(key) {
                resources(child, value, out)?;
            } else if schema["additionalProperties"].is_object() {
                resources(&schema["additionalProperties"], value, out)?;
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), input.as_array()) {
        for value in values {
            resources(items, value, out)?;
        }
    }
    Ok(())
}
fn has_resource_marker(v: &Value) -> bool {
    match v {
        Value::Object(o) => {
            o.contains_key("x-xelab-resource-kind") || o.values().any(has_resource_marker)
        }
        Value::Array(a) => a.iter().any(has_resource_marker),
        _ => false,
    }
}
#[allow(clippy::too_many_arguments)] // Bind the complete admitted call and signed input contract.
pub async fn grant_call(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    call: Uuid,
    deployment: Uuid,
    manifest: &Value,
    definition: &Value,
    input: &Value,
) -> Result<()> {
    if !requested(manifest, "blob.read") && !requested(manifest, "blob.create") {
        return Ok(());
    }
    check_approval(c, app, deployment, manifest).await?;
    let mut ids = BTreeSet::new();
    resources(&definition["input_schema"], input, &mut ids)?;
    let mut bounds = vec![
        json!({"action":"capability.invoke","resource":{"kind":"capability","match":"ids","ids":[format!("{}@{}",definition["contract"]["name"].as_str().unwrap(),definition["contract"]["version"].as_str().unwrap())]}}),
    ];
    for id in &ids {
        let row = c
            .query_opt(
                &format!(
                    "SELECT owner_id FROM blobs WHERE id=$1 AND {}",
                    crate::blobs::LIVE
                ),
                &[id],
            )
            .await?
            .ok_or_else(Error::not_found)?;
        let resource = Resource::new("blob", id, Some(row.get(0)));
        auth::authorize(c, ctx, "blob.read", &resource).await?;
        proxy_access(
            c,
            app,
            deployment,
            manifest,
            "blob.read",
            &resource,
            ctx.peer,
        )
        .await?;
    }
    if !ids.is_empty() {
        bounds
            .push(json!({"action":"blob.read","resource":{"kind":"blob","match":"ids","ids":ids}}));
    }
    if requested(manifest, "blob.create") {
        let resource = Resource::new("blob", "new", Some(ctx.subject));
        let result = async {
            auth::authorize(c, ctx, "blob.create", &resource).await?;
            proxy_access(
                c,
                app,
                deployment,
                manifest,
                "blob.create",
                &resource,
                ctx.peer,
            )
            .await
        }
        .await;
        match result {Ok(())=>bounds.push(json!({"action":"blob.create","resource":{"kind":"blob","match":"owned_by","owner_id":ctx.subject}})),Err(e) if matches!(e.code,"PERMISSION_DENIED"|"RESOURCE_NOT_FOUND")=>(),Err(e)=>return Err(e)}
    }
    let mut p = Policy::new(c, ctx.peer);
    let current = p.credential(ctx.credential, 0).await?;
    let paths = p.intersection(&current, &caps(&json!(bounds))?).await?;
    let peer = ctx.peer.to_string();
    c.execute("INSERT INTO host_rpc_contexts(call_id,peer,source_paths,read_blob_ids) VALUES($1,$2::text::inet,$3,$4)",&[&call,&peer,&json!(paths),&json!(ids)]).await?;
    c.execute("UPDATE execution_delegations SET maximum_rules=$2 WHERE id=(SELECT delegation_id FROM calls WHERE id=$1)",&[&call,&policy::rules(&paths)]).await?;
    Ok(())
}
#[derive(Clone, Copy)]
pub(crate) struct Workload {
    pub instance: Uuid,
    pub generation: i64,
}
struct Authority {
    ctx: Context,
    call: Uuid,
    until_ms: i64,
}
async fn authority(
    c: &(impl GenericClient + Sync),
    app: &App,
    workload: Workload,
    context: &rpc::RpcContext,
    action: &str,
    blob: Option<Uuid>,
) -> Result<Authority> {
    if app.leader.is_closed()
        || app.shutdown.load(Ordering::SeqCst)
        || !app.ready.load(Ordering::SeqCst)
    {
        return Err(Error::unavailable("Kernel is not ready"));
    }
    db::auth_lock(c, false).await?;
    let request: Uuid = context
        .request_id
        .parse()
        .map_err(|_| Error::invalid("RPC request_id must be a UUID"))?;
    if context.deadline_unix_ms <= chrono::Utc::now().timestamp_millis() {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Blob deadline exceeded",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    if context.delegation_token.len() != 43
        || (!context.actor_instance_id.is_empty()
            && context.actor_instance_id != workload.instance.to_string())
    {
        return Err(Error::denied());
    }
    let hash = app.auth.token_digest(&context.delegation_token);
    // Match terminal-call and controller row ordering: call, deployment,
    // instance, then delegation records. A reverse order can deadlock against
    // completion (call -> delegation) or health publication (deployment -> instance).
    let target=c.query_opt("SELECT r.call_id,i.deployment_id FROM rpc_delegations r JOIN instances i ON i.id=r.instance_id WHERE r.hmac_key_id=$1 AND r.verification_digest=$2 AND r.instance_id=$3",&[&app.auth.key_id.as_str(),&hash,&workload.instance]).await?.ok_or_else(Error::denied)?;
    c.query_one(
        "SELECT id FROM calls WHERE id=$1 FOR SHARE",
        &[&target.get::<_, Uuid>("call_id")],
    )
    .await?;
    c.query_one(
        "SELECT id FROM deployments WHERE id=$1 FOR SHARE",
        &[&target.get::<_, Uuid>("deployment_id")],
    )
    .await?;
    c.query_one(
        "SELECT id FROM instances WHERE id=$1 FOR SHARE",
        &[&workload.instance],
    )
    .await?;
    let row=c.query_opt("SELECT LEAST(r.expires_at,e.expires_at,c.deadline_at) AS until,r.call_id,r.subject_id,r.issuer_credential_id,e.maximum_rules,c.contract,i.deployment_id,t.manifest,x.source_paths,x.read_blob_ids,host(x.peer) AS peer FROM rpc_delegations r JOIN execution_delegations e ON e.id=r.execution_delegation_id JOIN calls c ON c.id=r.call_id JOIN host_rpc_contexts x ON x.call_id=c.id JOIN instances i ON i.id=r.instance_id JOIN deployments d ON d.id=i.deployment_id JOIN installations t ON t.id=d.installation_id JOIN publishers p ON p.id=t.publisher_id WHERE r.hmac_key_id=$1 AND r.verification_digest=$2 AND r.instance_id=$3 AND r.generation=$4 AND r.status='active' AND r.expires_at>now() AND e.status='active' AND e.expires_at>now() AND e.subject_id=r.subject_id AND e.issuer_credential_id=r.issuer_credential_id AND c.state='running' AND c.deadline_at>now() AND c.provider_instance_id=i.id AND c.owner_id=r.subject_id AND c.credential_id=r.issuer_credential_id AND c.delegation_id=e.id AND i.generation=r.generation AND i.runtime_id=i.id::text AND i.phase IN ('ready','degraded') AND d.spec->>'enabled'='true' AND d.archived_at IS NULL AND t.state='installed' AND p.status='trusted' FOR SHARE OF r,e,t,p",&[&app.auth.key_id.as_str(),&hash,&workload.instance,&workload.generation]).await?.ok_or_else(Error::denied)?;
    let peer: IpAddr = row
        .get::<_, String>("peer")
        .parse()
        .map_err(|_| Error::denied())?;
    let ctx = app
        .auth
        .resume(c, row.get("issuer_credential_id"), peer, request)
        .await?;
    if ctx.subject != row.get::<_, Uuid>("subject_id")
        || (!context.subject_id.is_empty() && context.subject_id != ctx.subject.to_string())
    {
        return Err(Error::denied());
    }
    let live = Policy::new(c, peer)
        .revalidate(&saved(row.get("source_paths"))?, ctx.credential)
        .await?;
    let rules = policy::rules(&live);
    let maximum: Value = row.get("maximum_rules");
    let contract: Value = row.get("contract");
    let capability = Resource::new(
        "capability",
        format!(
            "{}@{}",
            contract["name"].as_str().unwrap(),
            contract["version"].as_str().unwrap()
        ),
        None,
    );
    if !auth::rules_allow(&rules, "capability.invoke", &capability, peer) {
        return Err(Error::denied());
    }
    let resource = if let Some(blob) = blob {
        let allowed: Vec<Uuid> =
            serde_json::from_value(row.get("read_blob_ids")).map_err(|_| Error::denied())?;
        if !allowed.contains(&blob) {
            return Err(Error::denied());
        }
        let blobrow = c
            .query_opt(
                &format!(
                    "SELECT owner_id FROM blobs WHERE id=$1 AND {}",
                    crate::blobs::LIVE
                ),
                &[&blob],
            )
            .await?
            .ok_or_else(Error::denied)?;
        Resource::new("blob", blob, Some(blobrow.get(0)))
    } else {
        Resource::new("blob", "new", Some(ctx.subject))
    };
    if !auth::rules_allow(&rules, action, &resource, peer)
        || !auth::rules_allow(&maximum, action, &resource, peer)
    {
        return Err(Error::denied());
    }
    proxy_access(
        c,
        app,
        row.get("deployment_id"),
        &row.get("manifest"),
        action,
        &resource,
        peer,
    )
    .await?;
    let until_ms = row
        .get::<_, chrono::DateTime<chrono::Utc>>("until")
        .timestamp_millis();
    if until_ms <= chrono::Utc::now().timestamp_millis() {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Call deadline exceeded",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    Ok(Authority {
        ctx,
        call: row.get("call_id"),
        until_ms,
    })
}

#[allow(clippy::result_large_err)]
pub(crate) fn status(error: Error) -> tonic::Status {
    if error.code == "DEADLINE_EXCEEDED" {
        return tonic::Status::deadline_exceeded(error.message);
    }
    match error.status.as_u16() {
        400 => tonic::Status::invalid_argument(error.message),
        401 => tonic::Status::unauthenticated(error.message),
        403 | 404 => tonic::Status::permission_denied("Blob proxy is not authorized"),
        409 => tonic::Status::failed_precondition(error.message),
        _ => {
            if error.code == "RESOURCE_EXHAUSTED" {
                tonic::Status::resource_exhausted(error.message)
            } else {
                tonic::Status::unavailable(error.message)
            }
        }
    }
}
async fn audit(
    c: &impl GenericClient,
    workload: Workload,
    authority: &Authority,
    action: &str,
    blob: Uuid,
) -> Result<()> {
    c.execute("INSERT INTO audit_events(id,subject_id,actor_instance_id,credential_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'succeeded',(SELECT authz_revision FROM accounts WHERE id=$2),'HOST_BLOB',now(),now())",&[&Uuid::new_v4(),&authority.ctx.subject,&workload.instance,&authority.ctx.credential,&authority.ctx.request,&action,&Resource::new("blob",blob,Some(authority.ctx.subject)).json()]).await?;
    Ok(())
}
fn validate_header(header: &rpc::WriteBlobHeader) -> Result<()> {
    if header.size_bytes > 67108864
        || header.content_type.is_empty()
        || header.content_type.len() > 128
        || !header.content_type.is_ascii()
        || header.content_type.bytes().any(|b| b.is_ascii_control())
        || header.digest.len() != 71
        || !header.digest.starts_with("sha256:")
        || !header.digest[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid("Invalid Blob size, digest or content type"));
    }
    let context = header
        .context
        .as_ref()
        .ok_or_else(|| Error::invalid("RPC context is required"))?;
    if context.idempotency_key.is_empty()
        || context.idempotency_key.len() > 128
        || !context
            .idempotency_key
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b))
    {
        return Err(Error::invalid("Blob idempotency key is required"));
    }
    Ok(())
}
#[allow(clippy::result_large_err)] // The transport API returns Tonic Status.
pub(crate) async fn write(
    app: &App,
    workload: Workload,
    mut input: tonic::Streaming<rpc::WriteBlobRequest>,
    worker: tokio::sync::OwnedSemaphorePermit,
    transport_deadline: tokio::time::Instant,
) -> std::result::Result<rpc::BlobReference, tonic::Status> {
    use rpc::write_blob_request::Body;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    let _permit = app
        .blob_uploads
        .try_acquire()
        .map_err(|_| tonic::Status::resource_exhausted("Blob upload capacity is unavailable"))?;
    let first = tokio::time::timeout_at(
        transport_deadline.min(tokio::time::Instant::now() + Duration::from_secs(2)),
        input.message(),
    )
    .await
    .map_err(|_| tonic::Status::deadline_exceeded("Blob header deadline exceeded"))??;
    let header = match first.and_then(|m| m.body) {
        Some(Body::Header(h)) => h,
        _ => {
            return Err(tonic::Status::invalid_argument(
                "First Blob frame must contain only its header",
            ))
        }
    };
    validate_header(&header).map_err(status)?;
    let context = header.context.as_ref().unwrap();
    let remaining = context
        .deadline_unix_ms
        .saturating_sub(chrono::Utc::now().timestamp_millis())
        .clamp(0, 10000) as u64;
    let limit =
        transport_deadline.min(tokio::time::Instant::now() + Duration::from_millis(remaining));
    tokio::time::timeout_at(limit, async move {
    let mut c = app.db.connect().await.map_err(status)?;
    let tx = c.transaction().await.map_err(Error::from).map_err(status)?;
    let initial=authority(&tx, app, workload, context, "blob.create", None).await.map_err(status)?;
    let limit=limit.min(tokio::time::Instant::now()+Duration::from_millis(initial.until_ms.saturating_sub(chrono::Utc::now().timestamp_millis()).max(0) as u64));
    tx.commit().await.map_err(Error::from).map_err(status)?;
    let store = crate::blobs::bound_store(app).await.map_err(status)?;
    let (mut staged, _worker) = tokio::task::spawn_blocking(move || {
        store.stage(header.size_bytes).map(|stage| (stage, worker))
    })
    .await
    .map_err(|_| tonic::Status::unavailable("Blob staging stopped"))?
    .map_err(status)?;
    let mut size = 0u64;
    let mut hash = Sha256::new();
    let mut frames = 0u32;
    while let Some(frame) = tokio::time::timeout_at(limit, input.message())
        .await
        .map_err(|_| tonic::Status::deadline_exceeded("Blob transfer deadline exceeded"))??
    {
        frames += 1;
        let chunk = match frame.body {
            Some(Body::Data(chunk)) => chunk,
            _ => {
                return Err(tonic::Status::invalid_argument(
                    "Only data frames may follow the Blob header",
                ))
            }
        };
        if frames > 16384
            || chunk.offset != size
            || chunk.data.is_empty()
            || chunk.data.len() > 262144
        {
            return Err(tonic::Status::invalid_argument(
                "Blob frames must have continuous offsets and bounded nonempty data",
            ));
        }
        size = size
            .checked_add(chunk.data.len() as u64)
            .filter(|n| *n <= header.size_bytes)
            .ok_or_else(|| tonic::Status::invalid_argument("Blob length differs"))?;
        hash.update(&chunk.data);
        tokio::time::timeout_at(limit, staged.file.as_mut().unwrap().write_all(&chunk.data))
            .await
            .map_err(|_| tonic::Status::deadline_exceeded("Blob transfer deadline exceeded"))?
            .map_err(Error::from)
            .map_err(status)?;
    }
    if size != header.size_bytes || format!("sha256:{:x}", hash.finalize()) != header.digest {
        return Err(tonic::Status::invalid_argument(
            "Blob length or digest differs",
        ));
    }
    let tx = c.transaction().await.map_err(Error::from).map_err(status)?;
    db::auth_lock(&tx, false).await.map_err(status)?;
    // Serialize outputs for this call before checking its active row. The call
    // row remains shared through publication, preventing an ambiguous terminal
    // transition from opening a post-call write window.
    let hash = app.auth.token_digest(&context.delegation_token);
    let call: Uuid = tx
        .query_opt(
            "SELECT call_id FROM rpc_delegations WHERE hmac_key_id=$1 AND verification_digest=$2",
            &[&app.auth.key_id.as_str(), &hash],
        )
        .await
        .map_err(Error::from)
        .map_err(status)?
        .ok_or_else(|| tonic::Status::permission_denied("Blob delegation is invalid"))?
        .get(0);
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, $2))",
        &[&call.to_string(), &0x58454c48424f5554_i64],
    )
    .await
    .map_err(Error::from)
    .map_err(status)?;
    let auth = authority(&tx, app, workload, context, "blob.create", None)
        .await
        .map_err(status)?;
    if auth.call != call {
        return Err(tonic::Status::permission_denied("Blob delegation changed"));
    }
    let fingerprint = contracts::digest(
        &serde_json::to_vec(
            &json!({"size":size,"digest":header.digest,"content_type":header.content_type}),
        )
        .map_err(Error::from)
        .map_err(status)?,
    );
    let key = &context.idempotency_key;
    if let Some(row)=tx.query_opt("SELECT fingerprint,reference,blob_id FROM host_blob_outputs WHERE call_id=$1 AND idempotency_key=$2",&[&call,&key]).await.map_err(Error::from).map_err(status)? {
        if row.get::<_,String>("fingerprint")!=fingerprint {return Err(tonic::Status::already_exists("Blob idempotency fingerprint differs"));}
        let id:Uuid=row.get("blob_id");
        if tx.query_opt(&format!("SELECT id FROM blobs WHERE id=$1 AND {}",crate::blobs::LIVE),&[&id]).await.map_err(Error::from).map_err(status)?.is_none(){return Err(tonic::Status::failed_precondition("Original Blob has expired or been deleted"));}
        let value:Value=row.get("reference");
        return Ok(rpc::BlobReference{blob_id:id.to_string(),size_bytes:value["size_bytes"].as_u64().unwrap(),digest:value["digest"].as_str().unwrap().into()});
    }
    if tx
        .query_one(
            "SELECT count(*) FROM host_blob_outputs WHERE call_id=$1",
            &[&call],
        )
        .await
        .map_err(Error::from)
        .map_err(status)?
        .get::<_, i64>(0)
        >= 16
    {
        return Err(tonic::Status::resource_exhausted(
            "Call Blob output budget is exhausted",
        ));
    }
    let id = Uuid::new_v4();
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,$2))",
        &[&id.to_string(), &0x58454c424c4f4246_i64],
    )
    .await
    .map_err(Error::from)
    .map_err(status)?;
    tokio::time::timeout_at(limit, staged.publish(id))
        .await
        .map_err(|_| tonic::Status::deadline_exceeded("Blob publication deadline exceeded"))?
        .map_err(status)?;
    tx.execute("INSERT INTO blobs(id,owner_id,content_type,size_bytes,digest,backend_reference,expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,now()+interval '24 hours',now(),now())",&[&id,&auth.ctx.subject,&header.content_type,&(size as i64),&header.digest,&id.to_string()]).await.map_err(Error::from).map_err(status)?;
    let value = json!({"blob_id":id,"size_bytes":size,"digest":header.digest});
    tx.execute("INSERT INTO host_blob_outputs(call_id,idempotency_key,blob_id,fingerprint,reference) VALUES($1,$2,$3,$4,$5)",&[&call,&key,&id,&fingerprint,&value]).await.map_err(Error::from).map_err(status)?;
    audit(&tx, workload, &auth, "blob.create", id)
        .await
        .map_err(status)?;
    if app.leader.is_closed()
        || chrono::Utc::now().timestamp_millis() >= context.deadline_unix_ms
        || tokio::time::Instant::now() >= limit
    {
        return Err(tonic::Status::deadline_exceeded(
            "Blob authority expired before commit",
        ));
    }
    authority(&tx, app, workload, context, "blob.create", None)
        .await
        .map_err(status)?;
    tx.commit().await.map_err(Error::from).map_err(status)?;
    Ok(rpc::BlobReference {
        blob_id: id.to_string(),
        size_bytes: size,
        digest: header.digest,
    })
    }).await.map_err(|_|tonic::Status::deadline_exceeded("Blob transfer deadline exceeded"))?
}

pub(crate) type BlobStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = std::result::Result<rpc::BlobChunk, tonic::Status>> + Send>,
>;
#[allow(clippy::result_large_err)] // The transport API returns Tonic Status.
pub(crate) async fn read(
    app: Arc<App>,
    workload: Workload,
    request: rpc::ReadBlobRequest,
    permit: tokio::sync::OwnedSemaphorePermit,
    transport_deadline: tokio::time::Instant,
) -> std::result::Result<BlobStream, tonic::Status> {
    let context = request
        .context
        .ok_or_else(|| tonic::Status::invalid_argument("RPC context is required"))?;
    let id: Uuid = request
        .blob_id
        .parse()
        .map_err(|_| tonic::Status::invalid_argument("Invalid Blob ID"))?;
    if id.to_string() != request.blob_id {
        return Err(tonic::Status::invalid_argument("Noncanonical Blob ID"));
    }
    let duration = context
        .deadline_unix_ms
        .saturating_sub(chrono::Utc::now().timestamp_millis())
        .clamp(0, 300000) as u64;
    let deadline =
        transport_deadline.min(tokio::time::Instant::now() + Duration::from_millis(duration));
    tokio::time::timeout_at(deadline, async move {
    let mut c = app.db.connect().await.map_err(status)?;
    let tx = c.transaction().await.map_err(Error::from).map_err(status)?;
    let auth = authority(&tx, &app, workload, &context, "blob.read", Some(id))
        .await
        .map_err(status)?;
    let row = tx
        .query_one(
            "SELECT size_bytes,digest,backend_reference FROM blobs WHERE id=$1",
            &[&id],
        )
        .await
        .map_err(Error::from)
        .map_err(status)?;
    if row.get::<_, String>("backend_reference") != id.to_string() {
        return Err(tonic::Status::unavailable("Blob backend identity differs"));
    }
    let deadline=deadline.min(tokio::time::Instant::now()+Duration::from_millis(auth.until_ms.saturating_sub(chrono::Utc::now().timestamp_millis()).max(0) as u64));
    let size = row.get::<_, i64>("size_bytes") as u64;
    let expected: String = row.get("digest");
    audit(&tx, workload, &auth, "blob.read", id)
        .await
        .map_err(status)?;
    tx.commit().await.map_err(Error::from).map_err(status)?;
    let store = crate::blobs::bound_store(&app).await.map_err(status)?;
    let (file, permit) = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            store.verify(id, size, &expected).map(|file| (file, permit))
        }),
    )
    .await
    .map_err(|_| tonic::Status::deadline_exceeded("Blob verification deadline exceeded"))?
    .map_err(|_| tonic::Status::unavailable("Blob verification worker stopped"))?
    .map_err(status)?;
    let state = (
        tokio::fs::File::from_std(file),
        app,
        workload,
        context,
        id,
        0u64,
        size,
        deadline,
        permit,
    );
    use futures_util::TryStreamExt;
    let stream:BlobStream=Box::pin(
        futures_util::stream::try_unfold(
            state,
            |(mut file, app, workload, context, id, offset, size, deadline, permit)| async move {
                if offset == size {
                    return Ok(None);
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(tonic::Status::deadline_exceeded(
                        "Blob read deadline exceeded",
                    ));
                }
                tokio::time::timeout_at(deadline,async {
            let mut c = app.db.connect().await.map_err(status)?;
                let tx = c.transaction().await.map_err(Error::from).map_err(status)?;
                authority(&tx, &app, workload, &context, "blob.read", Some(id))
                    .await
                    .map_err(status)?;
                tx.commit().await.map_err(Error::from).map_err(status)?;
                Ok::<(),tonic::Status>(())
            }).await.map_err(|_|tonic::Status::deadline_exceeded("Blob authorization deadline exceeded"))??;
            use tokio::io::AsyncReadExt;
                let mut data = vec![0; ((size - offset).min(65536)) as usize];
                let n = tokio::time::timeout_at(deadline, file.read(&mut data))
                    .await
                    .map_err(|_| tonic::Status::deadline_exceeded("Blob read deadline exceeded"))?
                    .map_err(Error::from)
                    .map_err(status)?;
                if n == 0 {
                    return Err(tonic::Status::unavailable("Blob read was interrupted"));
                }
                data.truncate(n);
                Ok(Some((
                    rpc::BlobChunk { offset, data },
                    (
                        file,
                        app,
                        workload,
                        context,
                        id,
                        offset + n as u64,
                        size,
                        deadline,
                        permit,
                    ),
                )))
            },
        )
        .map_err(rpc_error),
    );
    Ok(stream)
    }).await.map_err(|_|tonic::Status::deadline_exceeded("Blob read deadline exceeded"))?
}

#[cfg(test)]
#[path = "host_blobs_tests.rs"]
mod tests;

/// Stable error metadata contains no token, request body or filesystem path.
pub(crate) fn rpc_error(mut status: tonic::Status) -> tonic::Status {
    use prost::Message;
    if status.metadata().get_bin("xelab-error-bin").is_none() {
        let code = match status.code() {
            tonic::Code::InvalidArgument => "INVALID_ARGUMENT",
            tonic::Code::Unauthenticated => "INVALID_CREDENTIAL",
            tonic::Code::PermissionDenied => "PERMISSION_DENIED",
            tonic::Code::AlreadyExists => "IDEMPOTENCY_CONFLICT",
            tonic::Code::FailedPrecondition => "STATE_CONFLICT",
            tonic::Code::ResourceExhausted => "RESOURCE_EXHAUSTED",
            tonic::Code::DeadlineExceeded => "DEADLINE_EXCEEDED",
            _ => "NOT_READY",
        };
        let detail = rpc::ErrorDetail {
            code: code.into(),
            message: status.message().into(),
            retryable: false,
            retry_after_ms: 0,
            violations: Vec::new(),
        };
        status.metadata_mut().insert_bin(
            "xelab-error-bin",
            tonic::metadata::MetadataValue::from_bytes(&detail.encode_to_vec()),
        );
    }
    status
}

#[allow(clippy::result_large_err)]
pub(crate) fn transport_deadline<T>(
    request: &tonic::Request<T>,
) -> std::result::Result<tokio::time::Instant, tonic::Status> {
    let duration = if let Some(raw) = request.metadata().get("grpc-timeout") {
        let raw = raw
            .to_str()
            .map_err(|_| tonic::Status::invalid_argument("Invalid gRPC deadline"))?;
        if !(2..=9).contains(&raw.len()) {
            return Err(tonic::Status::invalid_argument("Invalid gRPC deadline"));
        }
        let (digits, unit) = raw.split_at(raw.len() - 1);
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(tonic::Status::invalid_argument("Invalid gRPC deadline"));
        }
        let n: u64 = digits
            .parse()
            .map_err(|_| tonic::Status::invalid_argument("Invalid gRPC deadline"))?;
        match unit {
            "H" => Duration::from_secs(n.saturating_mul(3600)),
            "M" => Duration::from_secs(n.saturating_mul(60)),
            "S" => Duration::from_secs(n),
            "m" => Duration::from_millis(n),
            "u" => Duration::from_micros(n),
            "n" => Duration::from_nanos(n),
            _ => return Err(tonic::Status::invalid_argument("Invalid gRPC deadline")),
        }
    } else {
        Duration::from_secs(300)
    };
    Ok(tokio::time::Instant::now() + duration.min(Duration::from_secs(300)))
}
