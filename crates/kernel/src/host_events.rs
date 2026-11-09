//! Host event RPCs share the existing live-call, signed proxy-approval and
//! source-chain gate. Neither subject nor actor identity comes from the body.
use crate::{
    api::App,
    auth::Context,
    contracts, db,
    error::{Error, Result},
    events,
    host_calls::{self, Workload},
    rpc,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::{
    pin::Pin,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::time::Instant;
use tokio_postgres::GenericClient;
use tonic::Status;
use uuid::Uuid;

pub(crate) type Stream = Pin<
    Box<dyn tokio_stream::Stream<Item = std::result::Result<rpc::EventDelivery, Status>> + Send>,
>;

fn context(value: Option<&rpc::RpcContext>) -> Result<&rpc::RpcContext> {
    value.ok_or_else(|| Error::invalid("RPC context required"))
}
fn uuid(value: &str) -> Result<Uuid> {
    let id: Uuid = value
        .parse()
        .map_err(|_| Error::invalid("Invalid event resource UUID"))?;
    if id.to_string() != value {
        return Err(Error::invalid("Noncanonical event resource UUID"));
    }
    Ok(id)
}

async fn replay(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    context: &rpc::RpcContext,
    path: &str,
    body: &Value,
) -> Result<Option<Value>> {
    if context.idempotency_key.is_empty()
        || context.idempotency_key.len() > 128
        || !context
            .idempotency_key
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(Error::invalid("RPC idempotency key required"));
    }
    let lock = format!(
        "{}:{}:RPC:{path}:{}",
        ctx.subject, ctx.credential, context.idempotency_key
    );
    c.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        &[&lock],
    )
    .await?;
    let fingerprint = contracts::digest(&serde_json::to_vec(body)?);
    if let Some(row)=c.query_opt("SELECT fingerprint,safe_response FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='RPC' AND path=$3 AND key=$4 AND expires_at>now()",&[&ctx.subject,&ctx.credential,&path,&context.idempotency_key]).await?{
        if row.get::<_,String>("fingerprint")!=fingerprint{return Err(Error::new("IDEMPOTENCY_CONFLICT","RPC idempotency key has different request",axum::http::StatusCode::CONFLICT));}
        return Ok(row.get("safe_response"));
    }
    Ok(None)
}
async fn remember(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    context: &rpc::RpcContext,
    path: &str,
    body: &Value,
    response: &Value,
) -> Result<()> {
    let fingerprint = contracts::digest(&serde_json::to_vec(body)?);
    c.execute("DELETE FROM idempotency_records WHERE subject_id=$1 AND credential_id=$2 AND method='RPC' AND path=$3 AND key=$4 AND expires_at<=now()",&[&ctx.subject,&ctx.credential,&path,&context.idempotency_key]).await?;
    c.execute("INSERT INTO idempotency_records(subject_id,credential_id,method,path,key,fingerprint,response_status,safe_response,expires_at,created_at) VALUES($1,$2,'RPC',$3,$4,$5,200,$6,now()+interval '24 hours',now())",&[&ctx.subject,&ctx.credential,&path,&context.idempotency_key,&fingerprint,&response]).await?;
    Ok(())
}

pub(crate) async fn publish(
    app: &App,
    workload: Workload,
    request: rpc::PublishEventRequest,
) -> Result<rpc::PublishEventResponse> {
    let context = context(request.context.as_ref())?;
    let payload = contracts::strict_json(&request.payload_json, 1048576)?;
    let body = json!({"topic":request.topic,"version":request.version,"payload":payload,"dedupe_key":request.dedupe_key});
    contracts::validate("EventPublish", &body)?;
    let mut c = app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let resource = events::resource(&request.topic);
    let authority =
        host_calls::authority(&tx, app, workload, context, "event.publish", &resource).await?;
    let until = authority.until_ms.min(context.deadline_unix_ms) - Utc::now().timestamp_millis();
    if until <= 0 {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Event authority expired",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(until as u64);
    tokio::time::timeout_at(deadline,async {
        events::fence(&tx).await?;
        let authority=host_calls::authority(&tx,app,workload,context,"event.publish",&resource).await?;
    let actor=tx.query_one("SELECT t.manifest,t.publisher_id FROM deployments d JOIN installations t ON t.id=d.installation_id WHERE d.id=$1",&[&authority.deployment]).await?;
    let manifest: Value = actor.get("manifest");
    let original_publisher:Uuid=tx.query_one("SELECT t.publisher_id FROM event_contracts e JOIN installations t ON t.id=e.publisher_installation_id WHERE e.topic=$1 AND e.version=$2",&[&request.topic,&request.version]).await?.get(0);
    if actor.get::<_, Uuid>("publisher_id") != original_publisher {
        return Err(Error::denied());
    }
    events::validate_manifest(&manifest)?;
    if !manifest["event_topics"].as_array().is_some_and(|topics| {
        topics
            .iter()
            .any(|topic| topic["topic"] == request.topic && topic["version"] == request.version)
    }) {
        return Err(Error::denied());
    }
    events::authorize(
        &tx,
        &app.auth,
        &authority.ctx,
        "publishEvent",
        &std::collections::HashMap::new(),
        &body,
    )
    .await?;
    let path = format!(
        "/xelab.v1.HostService/PublishEvent/{}/{}",
        workload.instance, workload.generation
    );
    if let Some(saved) = replay(&tx, &authority.ctx, context, &path, &body).await? {
        let result = rpc::PublishEventResponse {
            event_id: saved["event_id"].as_str().ok_or_else(Error::denied)?.into(),
            duplicate: true,
        };
        tx.commit().await?;
        return Ok(result);
    }
    let (event, duplicate) = events::append(&tx, &app.auth, &authority.ctx, &body).await?;
    let response = json!({"event_id":event["id"],"duplicate":duplicate});
    remember(&tx, &authority.ctx, context, &path, &body, &response).await?;
    audit(&tx, &authority.ctx, workload, "event.publish", &resource).await?;
    tx.commit().await?;
    Ok(rpc::PublishEventResponse {
        event_id: event["id"].as_str().unwrap_or("").into(),
        duplicate,
    })
    }).await.map_err(|_|Error::new("DEADLINE_EXCEEDED","Event authority expired while waiting",axum::http::StatusCode::GATEWAY_TIMEOUT))?
}

pub(crate) async fn ack(
    app: &App,
    workload: Workload,
    request: rpc::AckEventRequest,
) -> Result<rpc::Ack> {
    let context = context(request.context.as_ref())?;
    let subscription = uuid(&request.subscription_id)?;
    let outcome = match request.outcome {
        1 => "ack",
        2 => "retry",
        3 => "dead_letter",
        _ => return Err(Error::invalid("Event outcome required")),
    };
    let body = json!({"delivery_id":request.delivery_id,"lease_token":request.lease_token,"outcome":outcome});
    contracts::validate("AckDelivery", &body)?;
    let mut c = app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let row = tx
        .query_opt(
            "SELECT owner_id,spec FROM subscriptions WHERE id=$1",
            &[&subscription],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let spec: Value = row.get("spec");
    let resource = events::resource(contracts::string(&spec, "topic")?);
    let authority =
        host_calls::authority(&tx, app, workload, context, "event.subscribe", &resource).await?;
    let until = authority.until_ms.min(context.deadline_unix_ms) - Utc::now().timestamp_millis();
    if until <= 0 {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Event authority expired",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(until as u64);
    tokio::time::timeout_at(deadline, async {
        events::fence(&tx).await?;
        let authority =
            host_calls::authority(&tx, app, workload, context, "event.subscribe", &resource)
                .await?;
        events::check_subscription(&tx, &app.auth, &authority.ctx, subscription).await?;
        let path = format!(
            "/xelab.v1.HostService/AckEvent/{}/{}/{subscription}",
            workload.instance, workload.generation
        );
        if replay(&tx, &authority.ctx, context, &path, &body)
            .await?
            .is_none()
        {
            events::ack(&tx, &app.auth, &authority.ctx, subscription, &body).await?;
            remember(
                &tx,
                &authority.ctx,
                context,
                &path,
                &body,
                &json!({"accepted":true}),
            )
            .await?;
            audit(&tx, &authority.ctx, workload, "event.subscribe", &resource).await?;
        }
        tx.commit().await?;
        Ok(rpc::Ack {
            request_id: context.request_id.clone(),
            accepted: true,
        })
    })
    .await
    .map_err(|_| {
        Error::new(
            "DEADLINE_EXCEEDED",
            "Event authority expired while waiting",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        )
    })?
}

async fn audit(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    workload: Workload,
    action: &str,
    resource: &crate::auth::Resource,
) -> Result<()> {
    c.execute("INSERT INTO audit_events(id,subject_id,actor_instance_id,credential_id,request_id,action,resource,outcome,policy_revision,reason_code,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'succeeded',(SELECT authz_revision FROM accounts WHERE id=$2),'HOST_EVENT',now(),now())",&[&Uuid::new_v4(),&ctx.subject,&workload.instance,&ctx.credential,&ctx.request,&action,&resource.json()]).await?;
    Ok(())
}

struct SubscriptionStream {
    app: Arc<App>,
    workload: Workload,
    context: rpc::RpcContext,
    subscription: Uuid,
    until: Instant,
    last_poll: Instant,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
async fn next(state: &SubscriptionStream) -> Result<Option<rpc::EventDelivery>> {
    let mut c = state.app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let row = tx
        .query_opt(
            "SELECT owner_id,spec FROM subscriptions WHERE id=$1",
            &[&state.subscription],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let spec: Value = row.get("spec");
    let resource = events::resource(contracts::string(&spec, "topic")?);
    let authority = host_calls::authority(
        &tx,
        &state.app,
        state.workload,
        &state.context,
        "event.subscribe",
        &resource,
    )
    .await?;
    let value = events::lease(&tx, &state.app.auth, &authority.ctx, state.subscription).await?;
    if value.is_some() {
        audit(
            &tx,
            &authority.ctx,
            state.workload,
            "event.subscribe",
            &resource,
        )
        .await?;
    }
    tx.commit().await?;
    value
        .map(|value| {
            let event = &value["event"];
            Ok(rpc::EventDelivery {
                delivery_id: contracts::string(&value, "delivery_id")?.into(),
                event_id: contracts::string(event, "id")?.into(),
                topic: contracts::string(event, "topic")?.into(),
                version: contracts::string(event, "version")?.into(),
                payload_json: serde_json::to_vec(&event["payload"])?,
                attempt: value["attempt"].as_u64().ok_or_else(Error::denied)? as u32,
                lease_token: contracts::string(&value, "lease_token")?.into(),
                lease_expires_unix_ms: DateTime::parse_from_rfc3339(contracts::string(
                    &value,
                    "lease_expires_at",
                )?)
                .map_err(|_| Error::denied())?
                .timestamp_millis(),
            })
        })
        .transpose()
}

pub(crate) async fn subscribe(
    app: Arc<App>,
    workload: Workload,
    request: rpc::SubscribeRequest,
    transport: Instant,
) -> Result<Stream> {
    let context = context(request.context.as_ref())?.clone();
    let subscription = uuid(&request.subscription_id)?;
    let mut c = app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let row = tx
        .query_opt(
            "SELECT owner_id,spec FROM subscriptions WHERE id=$1",
            &[&subscription],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    let spec: Value = row.get("spec");
    let resource = events::resource(contracts::string(&spec, "topic")?);
    let authority =
        host_calls::authority(&tx, &app, workload, &context, "event.subscribe", &resource).await?;
    events::check_subscription(&tx, &app.auth, &authority.ctx, subscription).await?;
    let milliseconds =
        authority.until_ms.min(context.deadline_unix_ms) - Utc::now().timestamp_millis();
    if milliseconds <= 0 {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Event subscription deadline expired",
            axum::http::StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    tx.commit().await?;
    static STREAMS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = STREAMS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(32)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::exhausted())?;
    let until = transport
        .min(Instant::now() + Duration::from_millis(milliseconds as u64))
        .min(Instant::now() + Duration::from_secs(3600));
    let state = SubscriptionStream {
        app,
        workload,
        context,
        subscription,
        until,
        last_poll: Instant::now(),
        _permit: permit,
    };
    Ok(Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            if state.last_poll.elapsed() > Duration::from_secs(30) || Instant::now() >= state.until
            {
                return None;
            }
            state.last_poll = Instant::now();
            loop {
                if Instant::now() >= state.until {
                    return None;
                }
                match tokio::time::timeout_at(
                    state.until.min(Instant::now() + Duration::from_secs(10)),
                    next(&state),
                )
                .await
                {
                    Ok(Ok(Some(event))) => {
                        state.last_poll = Instant::now();
                        return Some((Ok(event), state));
                    }
                    Ok(Ok(None)) => tokio::time::sleep(Duration::from_millis(250)).await,
                    Ok(Err(error)) => {
                        state.until = Instant::now();
                        return Some((Err(crate::host_calls::status(error)), state));
                    }
                    Err(_) => return None,
                }
            }
        },
    )))
}
