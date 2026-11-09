//! Transactional at-least-once event store. Public cursors are event IDs; the
//! private monotonic position survives retention and orders outbox visibility.
use crate::{
    api::{self, App},
    auth::{self, Auth, Context, Resource},
    contracts::{self, id, string, validate},
    db,
    error::{Error, Result},
    policy::{Path, Policy},
};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::{BTreeSet, HashMap};
use tokio_postgres::{GenericClient, Row};
use uuid::Uuid;

pub(crate) fn ensure_serving(app: &App) -> Result<()> {
    use std::sync::atomic::Ordering;
    if app.leader.is_closed()
        || app.shutdown.load(Ordering::SeqCst)
        || !app.ready.load(Ordering::SeqCst)
    {
        return Err(Error::unavailable("Event transport is stopping"));
    }
    Ok(())
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "publishEvent"
            | "listSubscriptions"
            | "createSubscription"
            | "getSubscription"
            | "deleteSubscription"
            | "ackDelivery"
    )
}

pub fn validate_manifest(manifest: &Value) -> Result<()> {
    let plugin = string(manifest, "plugin_id")?;
    let mut topics = BTreeSet::new();
    for topic in manifest["event_topics"].as_array().into_iter().flatten() {
        validate("EventTopicSpec", topic)?;
        let name = string(topic, "topic")?;
        if !name.starts_with(&format!("{plugin}."))
            || !topics.insert((name, string(topic, "version")?))
        {
            return Err(Error::invalid(
                "Event topics require unique plugin namespace and version",
            ));
        }
        contracts::compile_business(&topic["payload_schema"])?;
        if crate::skills::definition_digest(&topic["payload_schema"])?
            != string(topic, "schema_digest")?
        {
            return Err(Error::invalid("Event payload schema digest differs"));
        }
    }
    Ok(())
}

pub async fn store(
    c: &(impl GenericClient + Sync),
    installation: Uuid,
    manifest: &Value,
) -> Result<()> {
    validate_manifest(manifest)?;
    for topic in manifest["event_topics"].as_array().into_iter().flatten() {
        let name = string(topic, "topic")?;
        let version = string(topic, "version")?;
        let digest = string(topic, "schema_digest")?;
        c.execute("INSERT INTO event_contracts(topic,version,publisher_installation_id,schema_digest,payload_schema) VALUES($1,$2,$3,$4,$5) ON CONFLICT(topic,version) DO NOTHING",&[&name,&version,&installation,&digest,&topic["payload_schema"]]).await?;
        let row=c.query_one("SELECT schema_digest,payload_schema FROM event_contracts WHERE topic=$1 AND version=$2 FOR SHARE",&[&name,&version]).await?;
        if row.get::<_, String>("schema_digest") != digest
            || row.get::<_, Value>("payload_schema") != topic["payload_schema"]
        {
            return Err(Error::conflict(
                "Event topic version already has different immutable schema",
            ));
        }
    }
    Ok(())
}

pub fn resource(topic: &str) -> Resource {
    // Topics are global namespaces; subscription ownership is checked separately.
    Resource::new("event", topic, None)
}

async fn lock(c: &(impl GenericClient + Sync)) -> Result<Row> {
    c.execute("INSERT INTO event_store_limits(singleton,next_sequence) SELECT true,COALESCE(max(sequence),0)+1 FROM events ON CONFLICT(singleton) DO NOTHING",&[]).await?;
    Ok(c.query_one(
        "SELECT * FROM event_store_limits WHERE singleton FOR UPDATE",
        &[],
    )
    .await?)
}

pub(crate) async fn fence(c: &(impl GenericClient + Sync)) -> Result<()> {
    lock(c).await.map(|_| ())
}

fn event(row: &Row) -> Result<Value> {
    let mut value = db::stamp(row);
    for field in ["topic", "version", "dedupe_key"] {
        value[field] = json!(row.get::<_, String>(field));
    }
    value["publisher_id"] = json!(row.get::<_, Uuid>("publisher_id"));
    value["payload"] = row.get("payload");
    validate("Event", &value)?;
    Ok(value)
}

async fn verified_contract(
    c: &(impl GenericClient + Sync),
    topic: &str,
    version: &str,
) -> Result<Value> {
    // Retain the first immutable namespace publisher, while permitting a newer
    // installation from that same publisher to declare the exact schema.
    let row=c.query_opt("SELECT e.payload_schema,e.schema_digest,t.publisher_id,t.plugin_id FROM event_contracts e JOIN installations t ON t.id=e.publisher_installation_id JOIN publishers p ON p.id=t.publisher_id WHERE e.topic=$1 AND e.version=$2 AND p.status='trusted' FOR SHARE OF e,p",&[&topic,&version]).await?.ok_or_else(Error::not_found)?;
    let declaration = json!([{"topic":topic,"version":version,"schema_digest":row.get::<_,String>("schema_digest"),"payload_schema":row.get::<_,Value>("payload_schema")}]);
    let installed=c.query_opt("SELECT manifest FROM installations WHERE publisher_id=$1 AND plugin_id=$2 AND state='installed' AND manifest->'event_topics' @> $3 ORDER BY created_at DESC,id DESC LIMIT 1 FOR SHARE",&[&row.get::<_,Uuid>("publisher_id"),&row.get::<_,String>("plugin_id"),&declaration]).await?.ok_or_else(Error::not_found)?;
    let manifest: Value = installed.get("manifest");
    validate_manifest(&manifest)?;
    Ok(row.get("payload_schema"))
}

/// The caller owns one transaction and the shared authorization fence. A durable
/// envelope and its outbox intent commit together; delivery starts after materialization.
pub async fn append(
    c: &(impl GenericClient + Sync),
    auth: &Auth,
    ctx: &Context,
    body: &Value,
) -> Result<(Value, bool)> {
    let _ = auth;
    validate("EventPublish", body)?;
    let topic = string(body, "topic")?;
    let version = string(body, "version")?;
    auth::authorize(c, ctx, "event.publish", &resource(topic)).await?;
    let schema = verified_contract(c, topic, version).await?;
    contracts::validate_business(&schema, &body["payload"])?;
    if serde_json::to_vec(&body["payload"])?.len() > 1048576 {
        return Err(Error::invalid("Event payload exceeds 1 MiB"));
    }
    let normalized: Value = c
        .query_one("SELECT $1::jsonb AS payload", &[&body["payload"]])
        .await?
        .get("payload");
    contracts::validate_business(&schema, &normalized)?;
    let payload = serde_json::to_vec(&normalized)?;
    if payload.len() > 1048576 {
        return Err(Error::invalid("Event payload exceeds 1 MiB"));
    }
    let limits = lock(c).await?;
    // A wall-clock expiry can pass while the storage fence is contended even
    // though the shared authorization lock excludes concurrent revocation.
    auth::authorize(c, ctx, "event.publish", &resource(topic)).await?;
    let key = string(body, "dedupe_key")?;
    let digest = contracts::digest(&payload);
    if let Some(row)=c.query_opt("SELECT e.*,d.payload_digest FROM event_dedupe d JOIN events e ON e.id=d.event_id WHERE d.publisher_id=$1 AND d.topic=$2 AND d.version=$3 AND d.dedupe_key=$4 AND d.expires_at>now()",&[&ctx.subject,&topic,&version,&key]).await? {
        if row.get::<_,String>("payload_digest")!=digest { return Err(Error::conflict("Event dedupe key has different payload")); }
        db::audit(c,ctx.subject,Some(ctx.credential),ctx.request,"event.publish",&resource(topic).json(),"succeeded","EVENT_DUPLICATE").await?;
        return Ok((event(&row)?,true));
    }
    let size = (payload.len() + 1024) as i64;
    let usage = c
        .query_one(
            "SELECT count(*) AS count,COALESCE(sum(storage_bytes),0)::bigint AS bytes FROM events",
            &[],
        )
        .await?;
    if usage.get::<_, i64>("count") >= limits.get::<_, i64>("maximum_events")
        || usage
            .get::<_, i64>("bytes")
            .checked_add(size)
            .is_none_or(|bytes| bytes > limits.get::<_, i64>("maximum_bytes"))
    {
        tracing::warn!(
            reason_code = "EVENT_STORAGE_FULL",
            "Event publication rejected by storage quota"
        );
        return Err(Error::exhausted());
    }
    c.execute("DELETE FROM event_dedupe WHERE publisher_id=$1 AND topic=$2 AND version=$3 AND dedupe_key=$4 AND expires_at<=now()",&[&ctx.subject,&topic,&version,&key]).await?;
    let event_id = Uuid::new_v4();
    let sequence:i64=c.query_one("UPDATE event_store_limits SET next_sequence=next_sequence+1 WHERE singleton RETURNING next_sequence-1",&[]).await?.get(0);
    c.execute("INSERT INTO events(id,publisher_id,topic,version,payload,dedupe_key,expires_at,storage_bytes,sequence,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,now()+interval '7 days',$7,$8,now(),now())",&[&event_id,&ctx.subject,&topic,&version,&normalized,&key,&size,&sequence]).await?;
    c.execute("INSERT INTO event_dedupe(publisher_id,topic,version,dedupe_key,event_id,payload_digest,expires_at) VALUES($1,$2,$3,$4,$5,$6,now()+interval '24 hours')",&[&ctx.subject,&topic,&version,&key,&event_id,&digest]).await?;
    db::outbox(
        c,
        "kernel.event.publish",
        event_id,
        &json!({"event_id":event_id}),
    )
    .await?;
    let row = c
        .query_one("SELECT * FROM events WHERE id=$1", &[&event_id])
        .await?;
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        ctx.request,
        "event.publish",
        &resource(topic).json(),
        "succeeded",
        "EVENT_ACCEPTED",
    )
    .await?;
    Ok((event(&row)?, false))
}

/// Idempotent bounded replay. It never transforms unrelated lifecycle outboxes
/// into public events or guesses their publisher/contract authority.
pub async fn materialize(c: &(impl GenericClient + Sync)) -> Result<()> {
    fence(c).await?;
    let rows=c.query("SELECT b.id,b.aggregate_id FROM outbox b JOIN events e ON e.id=b.aggregate_id WHERE b.topic='kernel.event.publish' AND b.delivered_at IS NULL ORDER BY e.sequence LIMIT 64 FOR UPDATE OF b",&[]).await?;
    for row in rows {
        let id: Uuid = row.get("id");
        c.execute(
            "UPDATE events SET published=true WHERE id=$1",
            &[&row.get::<_, Uuid>("aggregate_id")],
        )
        .await?;
        c.execute(
            "UPDATE outbox SET delivered_at=now(),attempts=attempts+1 WHERE id=$1",
            &[&id],
        )
        .await?;
    }
    Ok(())
}

async fn subscription(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    subscription: Uuid,
    active: bool,
) -> Result<Row> {
    let row = c
        .query_opt("SELECT * FROM subscriptions WHERE id=$1", &[&subscription])
        .await?
        .ok_or_else(Error::not_found)?;
    if row.get::<_, Uuid>("owner_id") != ctx.subject {
        return Err(Error::not_found());
    }
    let spec: Value = row.get("spec");
    auth::authorize(
        c,
        ctx,
        "event.subscribe",
        &resource(string(&spec, "topic")?),
    )
    .await?;
    if active && row.get::<_, String>("status") != "active" {
        return Err(Error::conflict("Subscription is disabled"));
    }
    Ok(row)
}

pub async fn check_subscription(
    c: &(impl GenericClient + Sync),
    _app_auth: &Auth,
    ctx: &Context,
    id: Uuid,
) -> Result<()> {
    subscription(c, ctx, id, true).await.map(|_| ())
}

fn subscription_value(row: &Row) -> Result<Value> {
    let mut value = db::stamp(row);
    value["owner_id"] = json!(row.get::<_, Uuid>("owner_id"));
    value["spec"] = row.get("spec");
    value["status"] = json!(row.get::<_, String>("status"));
    value["cursor"] = json!(row.get::<_, Option<Uuid>>("cursor"));
    validate("Subscription", &value)?;
    Ok(value)
}

pub async fn authorize(
    c: &(impl GenericClient + Sync),
    _app_auth: &Auth,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
) -> Result<()> {
    match name {
        "listSubscriptions" => Ok(()),
        "publishEvent" => {
            validate("EventPublish", body)?;
            let topic = string(body, "topic")?;
            auth::authorize(c, ctx, "event.publish", &resource(topic)).await?;
            let schema = verified_contract(c, topic, string(body, "version")?).await?;
            contracts::validate_business(&schema, &body["payload"])?;
            if serde_json::to_vec(&body["payload"])?.len() > 1048576 {
                return Err(Error::invalid("Event payload exceeds 1 MiB"));
            }
            Ok(())
        }
        "createSubscription" => {
            auth::authorize(c, ctx, "event.subscribe", &resource(string(body, "topic")?)).await
        }
        _ => {
            subscription(
                c,
                ctx,
                api::path_id(params, "subscription_id")?,
                name == "ackDelivery",
            )
            .await?;
            Ok(())
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    subject: Uuid,
    credential: Uuid,
    origin: Option<String>,
    after: Uuid,
    expires: DateTime<Utc>,
}

#[allow(clippy::too_many_arguments)] // Matches the typed management dispatcher.
pub async fn execute(
    c: &(impl GenericClient + Sync),
    app: &App,
    ctx: &Context,
    name: &str,
    params: &HashMap<String, String>,
    body: &Value,
    headers: &HeaderMap,
    query: &str,
) -> Result<(Value, u16)> {
    match name {
        "publishEvent" => Ok((append(c, &app.auth, ctx, body).await?.0, 201)),
        "createSubscription" => {
            validate("SubscriptionSpec", body)?;
            let limits = lock(c).await?;
            let count: i64 = c
                .query_one("SELECT count(*) FROM subscriptions", &[])
                .await?
                .get(0);
            if count >= limits.get::<_, i64>("maximum_subscriptions") {
                return Err(Error::exhausted());
            }
            let mut spec = body.clone();
            if spec.get("start").is_none() {
                spec["start"] = json!("latest");
            }
            if spec.get("maximum_attempts").is_none() {
                spec["maximum_attempts"] = json!(5);
            }
            let topic = string(&spec, "topic")?;
            auth::authorize(c, ctx, "event.subscribe", &resource(topic)).await?;
            let watermark: i64 = if spec["start"] == "latest" {
                c.query_one("SELECT COALESCE(max(sequence),0) FROM events", &[])
                    .await?
                    .get(0)
            } else {
                0
            };
            let id = Uuid::new_v4();
            c.execute("INSERT INTO subscriptions(id,owner_id,spec,status,cursor_sequence,created_at,updated_at) VALUES($1,$2,$3,'active',$4,now(),now())",&[&id,&ctx.subject,&spec,&watermark]).await?;
            let event_resource = resource(topic);
            let mut p = Policy::for_context(c, ctx).with_resource_owner(&event_resource);
            let current = p.credential(ctx.credential, 0).await?;
            let cap = Path {
                rule: json!({"action":"event.subscribe","resource":{"kind":"event","match":"ids","ids":[topic]}}),
                sources: Default::default(),
            };
            let paths = p.intersection(&current, &[cap]).await?;
            if serde_json::to_vec(&paths)?.len() > 4194304 {
                return Err(Error::exhausted());
            }
            c.execute("INSERT INTO subscription_authority(subscription_id,credential_id,peer,source_paths,browser_origin) VALUES($1,$2,$3::text::inet,$4,$5)",&[&id,&ctx.credential,&ctx.peer.to_string(),&json!(paths),&ctx.browser_origin]).await?;
            Ok((
                subscription_value(
                    &c.query_one("SELECT * FROM subscriptions WHERE id=$1", &[&id])
                        .await?,
                )?,
                201,
            ))
        }
        "listSubscriptions" => {
            let mut limit = 50usize;
            let mut after = Uuid::nil();
            if !query.is_empty() {
                let q = contracts::strict_urlencoded(query.as_bytes(), 4096, 2)?;
                for (key, value) in q {
                    match key.as_str() {
                        "limit" => {
                            limit = value
                                .as_str()
                                .unwrap_or("")
                                .parse()
                                .map_err(|_| Error::invalid("Invalid page size"))?;
                            if !(1..=200).contains(&limit) {
                                return Err(Error::invalid("Invalid page size"));
                            }
                        }
                        "cursor" => {
                            let saved: Cursor = crate::plugin_catalog::decode_opaque_cursor(
                                &app.auth,
                                "xelab-event-subscriptions-v1",
                                "e1",
                                value.as_str().unwrap_or(""),
                            )?;
                            if saved.subject != ctx.subject
                                || saved.credential != ctx.credential
                                || saved.origin != ctx.browser_origin
                                || saved.expires < Utc::now()
                            {
                                return Err(Error::invalid("Subscription cursor scope changed"));
                            }
                            after = saved.after;
                        }
                        _ => return Err(Error::invalid("Unknown subscriptions query")),
                    }
                }
            }
            let rows=c.query("SELECT * FROM subscriptions WHERE owner_id=$1 AND id>$2 ORDER BY id LIMIT 1001",&[&ctx.subject,&after]).await?;
            let mut items = Vec::new();
            let mut scanned = after;
            let mut more = false;
            for row in rows {
                if items.len() == limit {
                    more = true;
                    break;
                }
                scanned = row.get("id");
                let spec: Value = row.get("spec");
                match auth::authorize(
                    c,
                    ctx,
                    "event.subscribe",
                    &resource(string(&spec, "topic")?),
                )
                .await
                {
                    Ok(()) => items.push(subscription_value(&row)?),
                    Err(e) if matches!(e.code, "PERMISSION_DENIED" | "RESOURCE_NOT_FOUND") => (),
                    Err(e) => return Err(e),
                }
            }
            let next = if more {
                Some(crate::plugin_catalog::encode_opaque_cursor(
                    &app.auth,
                    "xelab-event-subscriptions-v1",
                    "e1",
                    &Cursor {
                        subject: ctx.subject,
                        credential: ctx.credential,
                        origin: ctx.browser_origin.clone(),
                        after: scanned,
                        expires: Utc::now() + chrono::Duration::hours(24),
                    },
                )?)
            } else {
                None
            };
            Ok((json!({"items":items,"next_cursor":next}), 200))
        }
        "getSubscription" => Ok((
            subscription_value(
                &subscription(c, ctx, api::path_id(params, "subscription_id")?, false).await?,
            )?,
            200,
        )),
        "deleteSubscription" => {
            let sub_id = api::path_id(params, "subscription_id")?;
            c.query_opt(
                "SELECT id FROM subscriptions WHERE id=$1 FOR UPDATE",
                &[&sub_id],
            )
            .await?;
            let row = subscription(c, ctx, sub_id, false).await?;
            api::etag(headers, row.get("revision"))?;
            c.execute("UPDATE subscriptions SET status='disabled',revision=revision+1,updated_at=now() WHERE id=$1",&[&row.get::<_,Uuid>("id")]).await?;
            Ok((Value::Null, 204))
        }
        "ackDelivery" => {
            ack(
                c,
                &app.auth,
                ctx,
                api::path_id(params, "subscription_id")?,
                body,
            )
            .await?;
            Ok((Value::Null, 204))
        }
        _ => Err(Error::invalid("Unknown event operation")),
    }
}

fn backoff(attempt: i32) -> chrono::Duration {
    let base = 1i64 << ((attempt - 1).clamp(0, 6));
    // Full additive jitter up to 25%; persisted once, never moved on polls.
    let jitter = rand::random::<u32>() as i64 % ((base * 250) + 1);
    chrono::Duration::milliseconds((base * 1000 + jitter).min(60000))
}

async fn finish(
    c: &(impl GenericClient + Sync),
    subscription: Uuid,
    delivery: &Row,
    outcome: &str,
    reason: Option<&str>,
) -> Result<()> {
    let delivery_id: Uuid = delivery.get("id");
    let event_id: Uuid = delivery.get("event_id");
    let state = if outcome == "ack" {
        "acked"
    } else {
        "dead_letter"
    };
    c.execute(
        "UPDATE event_deliveries SET state=$2,outcome=$3,completed_at=now() WHERE id=$1",
        &[&delivery_id, &state, &outcome],
    )
    .await?;
    if let Some(reason) = reason {
        c.execute("INSERT INTO event_dead_letters(delivery_id,subscription_id,event_id,reason) VALUES($1,$2,$3,$4)",&[&delivery_id,&subscription,&event_id,&reason]).await?;
        c.execute("UPDATE events SET expires_at=greatest(expires_at,now()+interval '30 days') WHERE id=$1",&[&event_id]).await?;
    }
    c.execute("UPDATE subscriptions SET cursor=$2,cursor_sequence=(SELECT sequence FROM events WHERE id=$2),revision=revision+1,updated_at=now() WHERE id=$1",&[&subscription,&event_id]).await?;
    Ok(())
}

/// Scheduling hint only. It grants no authority and never changes a cursor.
/// The actual lease still locks and reauthorizes before exposing payload bytes.
pub(crate) async fn delivery_ready(c: &(impl GenericClient + Sync), sub: &Row) -> Result<bool> {
    let subscription: Uuid = sub.get("id");
    if let Some(row)=c.query_opt("SELECT state,lease_expires_at,retry_at,clock_timestamp() AS instant FROM event_deliveries WHERE subscription_id=$1 AND state IN ('leased','retry') AND completed_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 1",&[&subscription]).await? {
        let now:DateTime<Utc>=row.get("instant");
        return Ok(if row.get::<_,String>("state")=="leased" {row.get::<_,DateTime<Utc>>("lease_expires_at")<=now}else{row.get::<_,Option<DateTime<Utc>>>("retry_at").is_some_and(|time|time<=now)});
    }
    let spec: Value = sub.get("spec");
    Ok(c.query_opt("SELECT published FROM events WHERE topic=$1 AND sequence>$2 AND expires_at>$3 ORDER BY sequence LIMIT 1",&[&string(&spec,"topic")?,&sub.get::<_,i64>("cursor_sequence"),&sub.get::<_,DateTime<Utc>>("created_at")]).await?.is_some_and(|row|row.get(0)))
}

pub async fn lease(
    c: &(impl GenericClient + Sync),
    app_auth: &Auth,
    ctx: &Context,
    subscription_id: Uuid,
) -> Result<Option<Value>> {
    let limits = lock(c).await?;
    c.query_opt(
        "SELECT id FROM subscriptions WHERE id=$1 FOR UPDATE",
        &[&subscription_id],
    )
    .await?;
    let sub = subscription(c, ctx, subscription_id, true).await?;
    let spec: Value = sub.get("spec");
    let maximum = spec["maximum_attempts"].as_i64().unwrap_or(5) as i32;
    let current=c.query_opt("SELECT d.*,clock_timestamp() AS db_now FROM event_deliveries d WHERE subscription_id=$1 AND state IN ('leased','retry') AND completed_at IS NULL ORDER BY created_at DESC,id DESC LIMIT 1 FOR UPDATE",&[&subscription_id]).await?;
    let mut next_attempt = 1;
    let mut selected = None;
    if let Some(row) = current {
        let now: DateTime<Utc> = row.get("db_now");
        if row.get::<_, String>("state") == "leased" {
            if row.get::<_, DateTime<Utc>>("lease_expires_at") > now {
                return Ok(None);
            }
            if row.get::<_, i32>("attempt") >= maximum {
                finish(
                    c,
                    subscription_id,
                    &row,
                    "dead_letter",
                    Some("maximum_attempts"),
                )
                .await?;
                return Ok(None);
            }
            let retry_at =
                row.get::<_, DateTime<Utc>>("lease_expires_at") + backoff(row.get("attempt"));
            c.execute(
                "UPDATE event_deliveries SET state='retry',retry_at=$2 WHERE id=$1",
                &[&row.get::<_, Uuid>("id"), &retry_at],
            )
            .await?;
            if retry_at > now {
                return Ok(None);
            }
        } else if row
            .get::<_, Option<DateTime<Utc>>>("retry_at")
            .is_none_or(|time| time > now)
        {
            return Ok(None);
        }
        next_attempt = row.get::<_, i32>("attempt") + 1;
        selected = Some(row.get::<_, Uuid>("event_id"));
    }
    let event_row = if let Some(id) = selected {
        c.query_opt("SELECT * FROM events WHERE id=$1", &[&id])
            .await?
    } else {
        c.query_opt("SELECT * FROM events WHERE topic=$1 AND sequence>$2 AND expires_at>$3 ORDER BY sequence LIMIT 1",&[&string(&spec,"topic")?,&sub.get::<_,i64>("cursor_sequence"),&sub.get::<_,DateTime<Utc>>("created_at")]).await?
    };
    let Some(event_row) = event_row else {
        return Ok(None);
    };
    if !event_row.get::<_, bool>("published") {
        return Ok(None);
    }
    let count: i64 = c
        .query_one("SELECT count(*) FROM event_deliveries", &[])
        .await?
        .get(0);
    if count >= limits.get::<_, i64>("maximum_deliveries") {
        return Err(Error::exhausted());
    }
    // Old retry records are historical; only the newest attempt can be pending.
    c.execute("UPDATE event_deliveries SET completed_at=COALESCE(completed_at,now()) WHERE subscription_id=$1 AND state='retry' AND completed_at IS NULL",&[&subscription_id]).await?;
    let id = Uuid::new_v4();
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);
    let mut mac = Hmac::<Sha256>::new_from_slice(&app_auth.token_digest("xelab-event-lease-v1"))
        .map_err(|_| Error::denied())?;
    mac.update(token.as_bytes());
    let hash = mac.finalize().into_bytes().to_vec();
    let event_id: Uuid = event_row.get("id");
    let row=c.query_one("INSERT INTO event_deliveries(id,subscription_id,event_id,attempt,lease_digest,lease_expires_at,state,created_at) VALUES($1,$2,$3,$4,$5,clock_timestamp()+interval '30 seconds','leased',now()) RETURNING lease_expires_at",&[&id,&subscription_id,&event_id,&next_attempt,&hash]).await?;
    let value = json!({"delivery_id":id,"subscription_id":subscription_id,"event":event(&event_row)?,"attempt":next_attempt,"lease_token":token,"lease_expires_at":row.get::<_,DateTime<Utc>>(0)});
    validate("Delivery", &value)?;
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        ctx.request,
        "event.subscribe",
        &resource(string(&spec, "topic")?).json(),
        "succeeded",
        "EVENT_LEASED",
    )
    .await?;
    Ok(Some(value))
}

pub async fn ack(
    c: &(impl GenericClient + Sync),
    app_auth: &Auth,
    ctx: &Context,
    subscription_id: Uuid,
    body: &Value,
) -> Result<()> {
    validate("AckDelivery", body)?;
    lock(c).await?;
    c.query_opt(
        "SELECT id FROM subscriptions WHERE id=$1 FOR UPDATE",
        &[&subscription_id],
    )
    .await?;
    let sub = subscription(c, ctx, subscription_id, true).await?;
    let delivery = id(body, "delivery_id")?;
    let row=c.query_opt("SELECT d.*,clock_timestamp() AS db_now FROM event_deliveries d WHERE id=$1 AND subscription_id=$2 FOR UPDATE",&[&delivery,&subscription_id]).await?.ok_or_else(Error::not_found)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&app_auth.token_digest("xelab-event-lease-v1"))
        .map_err(|_| Error::denied())?;
    mac.update(string(body, "lease_token")?.as_bytes());
    if mac
        .verify_slice(&row.get::<_, Vec<u8>>("lease_digest"))
        .is_err()
    {
        return Err(Error::conflict("Delivery lease does not match"));
    }
    let outcome = string(body, "outcome")?;
    if row.get::<_, String>("state") == "acked"
        && row.get::<_, Option<String>>("outcome").as_deref() == Some("ack")
        && outcome == "ack"
    {
        return Ok(());
    }
    if row.get::<_, String>("state") != "leased"
        || row.get::<_, DateTime<Utc>>("lease_expires_at") <= row.get::<_, DateTime<Utc>>("db_now")
    {
        return Err(Error::conflict(
            "Delivery lease is expired or no longer current",
        ));
    }
    let maximum = sub.get::<_, Value>("spec")["maximum_attempts"]
        .as_i64()
        .unwrap_or(5) as i32;
    match outcome {
        "ack" => finish(c, subscription_id, &row, "ack", None).await?,
        "dead_letter" => {
            finish(
                c,
                subscription_id,
                &row,
                "dead_letter",
                Some("consumer_dead_letter"),
            )
            .await?
        }
        "retry" if row.get::<_, i32>("attempt") >= maximum => {
            finish(
                c,
                subscription_id,
                &row,
                "dead_letter",
                Some("maximum_attempts"),
            )
            .await?
        }
        "retry" => {
            let retry_at = row.get::<_, DateTime<Utc>>("db_now") + backoff(row.get("attempt"));
            c.execute(
                "UPDATE event_deliveries SET state='retry',outcome='retry',retry_at=$2 WHERE id=$1",
                &[&delivery, &retry_at],
            )
            .await?;
        }
        _ => return Err(Error::invalid("Invalid delivery outcome")),
    }
    let spec: Value = sub.get("spec");
    db::audit(
        c,
        ctx.subject,
        Some(ctx.credential),
        ctx.request,
        "event.subscribe",
        &resource(string(&spec, "topic")?).json(),
        "succeeded",
        match outcome {
            "ack" => "EVENT_ACKED",
            "retry" => "EVENT_RETRY",
            _ => "EVENT_DEAD_LETTER",
        },
    )
    .await?;
    Ok(())
}

pub async fn maintenance(c: &(impl GenericClient + Sync)) -> Result<()> {
    materialize(c).await?;
    // Every lane is bounded independently, so expiry cleanup cannot monopolize
    // the storage fence or starve outbox publication under an expired prefix.
    c.execute("DELETE FROM event_dedupe WHERE ctid IN(SELECT ctid FROM event_dedupe WHERE expires_at<=clock_timestamp() LIMIT 128)",&[]).await?;
    c.execute("DELETE FROM event_dead_letters WHERE delivery_id IN(SELECT delivery_id FROM event_dead_letters WHERE expires_at<=clock_timestamp() ORDER BY expires_at LIMIT 128)",&[]).await?;
    c.execute("DELETE FROM event_deliveries WHERE id IN(SELECT d.id FROM event_deliveries d JOIN subscriptions s ON d.subscription_id=s.id WHERE NOT EXISTS(SELECT 1 FROM event_dead_letters l WHERE l.delivery_id=d.id) AND ((d.completed_at<clock_timestamp()-interval '7 days') OR (s.status='disabled' AND d.created_at<clock_timestamp()-interval '7 days')) ORDER BY d.created_at,d.id LIMIT 128)",&[]).await?;
    let expired:Vec<Uuid>=c.query("SELECT e.id FROM events e WHERE e.published AND e.expires_at<=clock_timestamp() AND NOT EXISTS(SELECT 1 FROM event_deliveries d WHERE d.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM event_dedupe d WHERE d.event_id=e.id) AND NOT EXISTS(SELECT 1 FROM subscriptions s WHERE s.status='active' AND s.spec->>'topic'=e.topic AND s.cursor_sequence<e.sequence AND e.expires_at>s.created_at) ORDER BY e.sequence LIMIT 64",&[]).await?.into_iter().map(|row|row.get(0)).collect();
    if !expired.is_empty() {
        c.execute("UPDATE subscriptions SET cursor=NULL,revision=revision+1,updated_at=now() WHERE cursor=ANY($1)",&[&expired]).await?;
        c.execute("DELETE FROM outbox WHERE topic='kernel.event.publish' AND aggregate_id=ANY($1) AND delivered_at IS NOT NULL",&[&expired]).await?;
        c.execute("DELETE FROM events WHERE id=ANY($1)", &[&expired])
            .await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;

#[path = "events_snapshot.rs"]
pub(crate) mod snapshot;
