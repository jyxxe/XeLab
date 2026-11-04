//! Bounded Skill SSE transport. The Broker remains the sole admission authority.
use crate::{
    api::{self, App},
    contracts::{strict_json, validate, validate_business},
    error::{Error, Result},
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use uuid::Uuid;

const MAX_FRAME: usize = 1024 * 1024;
// At most two queued frames, one producer frame, and one consumer frame.
const BUFFER_FRAMES: usize = 2;

pub(crate) fn ensure_active(app: &App) -> Result<()> {
    if app.leader.is_closed()
        || app.shutdown.load(std::sync::atomic::Ordering::SeqCst)
        || !app.ready.load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(Error::unavailable("Kernel is not ready"));
    }
    Ok(())
}

#[derive(Clone)]
struct Identity {
    skill: Uuid,
    version: Value,
    digest: String,
    call: Uuid,
    deadline: chrono::DateTime<chrono::Utc>,
}
struct State {
    identity: Option<Identity>,
    sequence: u64,
    terminal: bool,
    accepted: bool,
}
#[derive(Clone)]
pub(crate) struct Frame {
    pub call: Uuid,
    pub sequence: u64,
    deadline: chrono::DateTime<chrono::Utc>,
    kind: &'static str,
    data: String,
}
impl Frame {
    fn event(&self) -> Event {
        Event::default()
            .event(self.kind)
            .id(format!("{}:{}", self.call, self.sequence))
            .data(self.data.clone())
    }
    fn fail(&self, error: Error) -> Result<Self> {
        let mut value = strict_json(self.data.as_bytes(), MAX_FRAME)?;
        let (kind, payload) = match error.code {
            "CANCELED" => ("canceled", json!({"reason":"requested"})),
            "DEADLINE_EXCEEDED" => ("canceled", json!({"reason":"deadline"})),
            _ => ("failed", error.detail()),
        };
        value["event"]["kind"] = json!(kind);
        value["event"]["payload"] = payload;
        value["event"]["terminal"] = json!(true);
        frame(&value, kind, self.call, self.sequence, self.deadline)
    }
}
fn frame(
    value: &Value,
    kind: &'static str,
    call: Uuid,
    sequence: u64,
    deadline: chrono::DateTime<chrono::Utc>,
) -> Result<Frame> {
    validate("SkillStreamEvent", value)?;
    let data = serde_json::to_string(value)?;
    if data.len() > MAX_FRAME {
        return Err(Error::invalid("Skill stream frame exceeds 1 MiB"));
    }
    Ok(Frame {
        call,
        sequence,
        deadline,
        kind,
        data,
    })
}

/// One serialized producer advances sequence and terminal state only after a
/// successful send. A terminal frame can never be followed by another item.
pub(crate) struct Output {
    sender: mpsc::Sender<Result<Frame>>,
    state: Mutex<State>,
    emission: tokio::sync::Mutex<()>,
}
impl Output {
    fn new(sender: mpsc::Sender<Result<Frame>>) -> Self {
        Self {
            sender,
            emission: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                identity: None,
                sequence: 0,
                terminal: false,
                accepted: false,
            }),
        }
    }
    pub(crate) fn bind(
        &self,
        skill: &super::skills::ResolvedSkill,
        call: Uuid,
        deadline: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::unavailable("Stream state is unavailable"))?;
        if state.identity.is_some() {
            return Err(Error::conflict("Stream identity was already bound"));
        }
        state.identity = Some(Identity {
            skill: skill.id,
            version: skill.definition["version"].clone(),
            digest: skill.definition_digest.clone(),
            call,
            deadline,
        });
        Ok(())
    }
    pub(crate) fn validate_completed(&self, payload: &Value) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| Error::unavailable("Stream state is unavailable"))?;
        let identity = state
            .identity
            .as_ref()
            .ok_or_else(|| Error::conflict("Stream has no admitted call"))?;
        let value = json!({"skill_id":identity.skill,"skill_version":identity.version,"definition_digest":identity.digest,
            "event":{"call_id":identity.call,"sequence":state.sequence+1,"kind":"completed","payload":payload,"terminal":true}});
        frame(
            &value,
            "completed",
            identity.call,
            state.sequence + 1,
            identity.deadline,
        )?;
        Ok(())
    }
    pub(crate) async fn closed(&self) {
        self.sender.closed().await;
    }
    pub(crate) fn ensure_open(&self) -> Result<()> {
        if self.sender.is_closed() {
            return Err(Error::new(
                "CANCELED",
                "Skill stream consumer disconnected",
                StatusCode::CONFLICT,
            ));
        }
        Ok(())
    }
    pub(crate) async fn abandoned_before_admission(&self) {
        self.closed().await;
        let admitted = self
            .state
            .lock()
            .map(|state| state.identity.is_some())
            .unwrap_or(false);
        if admitted {
            // Once admitted the Broker finalizer must retain accounting and
            // revoke its delegation; its existing execution monitor owns cancel.
            std::future::pending::<()>().await;
        }
    }
    pub(crate) async fn emit(&self, kind: &'static str, payload: Value) -> Result<()> {
        let _emission = self.emission.lock().await;
        let (message, deadline, sequence) = {
            let state = self
                .state
                .lock()
                .map_err(|_| Error::unavailable("Stream state is unavailable"))?;
            if state.terminal {
                return Err(Error::conflict("Stream is already terminal"));
            }
            let identity = state
                .identity
                .clone()
                .ok_or_else(|| Error::conflict("Stream has no admitted call"))?;
            if state.sequence >= 9007199254740991 {
                return Err(Error::invalid(
                    "Skill stream sequence exceeds its contract limit",
                ));
            }
            let sequence = state.sequence + 1;
            let terminal = matches!(kind, "completed" | "failed" | "canceled");
            let value = json!({"skill_id":identity.skill,"skill_version":identity.version,"definition_digest":identity.digest,
                "event":{"call_id":identity.call,"sequence":sequence,"kind":kind,"payload":payload,"terminal":terminal}});
            let message = frame(&value, kind, identity.call, sequence, identity.deadline)?;
            (message, identity.deadline, sequence)
        };
        let remaining = (deadline - chrono::Utc::now()).num_milliseconds().max(0) as u64;
        if remaining == 0 {
            if matches!(kind, "failed" | "canceled") {
                self.sender.try_send(Ok(message)).map_err(|_| {
                    Error::new(
                        "CANCELED",
                        "Skill stream consumer did not advance before deadline",
                        StatusCode::CONFLICT,
                    )
                })?;
                self.mark_sent(kind, sequence)?;
                return Ok(());
            }
            return Err(Error::new(
                "DEADLINE_EXCEEDED",
                "Skill stream deadline elapsed",
                StatusCode::GATEWAY_TIMEOUT,
            ));
        }
        tokio::time::timeout(
            Duration::from_millis(remaining.min(30000)),
            self.sender.send(Ok(message)),
        )
        .await
        .map_err(|_| {
            Error::new(
                "RESOURCE_EXHAUSTED",
                "Skill stream consumer did not advance",
                StatusCode::SERVICE_UNAVAILABLE,
            )
        })?
        .map_err(|_| {
            Error::new(
                "CANCELED",
                "Skill stream consumer disconnected",
                StatusCode::CONFLICT,
            )
        })?;
        self.mark_sent(kind, sequence)?;
        Ok(())
    }
    fn mark_sent(&self, kind: &'static str, sequence: u64) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::unavailable("Stream state is unavailable"))?;
        state.sequence = sequence;
        state.terminal = matches!(kind, "completed" | "failed" | "canceled");
        state.accepted |= kind == "accepted";
        Ok(())
    }
    async fn finish(&self, result: Result<Value>) {
        let admitted = self
            .state
            .lock()
            .map(|state| state.accepted)
            .unwrap_or(false);
        if admitted {
            match result {
                Ok(value) => {
                    let _ = self.emit("completed", value["call"].clone()).await;
                }
                Err(error) => match error.code {
                    "CANCELED" => {
                        let _ = self.emit("canceled", json!({"reason":"requested"})).await;
                    }
                    "DEADLINE_EXCEEDED" => {
                        let _ = self.emit("canceled", json!({"reason":"deadline"})).await;
                    }
                    _ => {
                        let _ = self.emit("failed", error.detail()).await;
                    }
                },
            }
        } else if let Err(error) = result {
            let _ = self.sender.send(Err(error)).await;
        }
    }
}

pub(crate) async fn handle(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
) -> Result<Response> {
    let path = request.uri().path();
    if path.contains('%') || path.contains("//") || request.uri().query().is_some() {
        return Err(Error::invalid(
            "Noncanonical Skill stream path or unsupported query",
        ));
    }
    let (operation, params) =
        api::operation(request.method().as_str(), path).ok_or_else(Error::not_found)?;
    if operation["operationId"] != "streamSkill" {
        return Err(Error::not_found());
    }
    let headers = request.headers();
    if headers.contains_key("last-event-id") {
        return Err(Error::invalid("Skill streams do not support Last-Event-ID"));
    }
    if headers.get_all("origin").iter().count() > 1 {
        return Err(Error::denied());
    }
    let origin = headers.get("origin").and_then(|value| value.to_str().ok());
    let trusted_origin = app.config.text("server", "public_origin");
    if (headers.contains_key("origin") && origin != Some(trusted_origin))
        || (!headers.contains_key("authorization") && origin != Some(trusted_origin))
    {
        return Err(Error::denied());
    }
    ensure_active(&app)?;
    let token = api::bearer(headers)?.to_owned();
    let key = api::header(headers, "idempotency-key")?.to_owned();
    validate_business(
        &json!({"type":"string","minLength":1,"maxLength":128}),
        &json!(key),
    )?;
    if api::header(headers, "content-type")?.split(';').next() != Some("application/json") {
        return Err(Error::new(
            "UNSUPPORTED_MEDIA_TYPE",
            "Expected application/json",
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ));
    }
    let skill = api::path_id(&params, "skill_id")?;
    let body = to_bytes(request.into_body(), 4194304).await.map_err(|_| {
        Error::new(
            "PAYLOAD_TOO_LARGE",
            "Request body exceeds 4 MiB",
            StatusCode::PAYLOAD_TOO_LARGE,
        )
    })?;
    let input = strict_json(&body, 4194304)?;
    validate("SkillInvokeRequest", &input)?;
    let (sender, mut receiver) = mpsc::channel(BUFFER_FRAMES);
    let output = Arc::new(Output::new(sender));
    let actor = app.clone();
    let actor_token = token.clone();
    tokio::spawn(async move {
        let result = crate::broker::stream_skill(
            &actor,
            &actor_token,
            peer.ip(),
            request_id,
            skill,
            &input,
            &key,
            &output,
        )
        .await;
        output.finish(result).await;
    });
    // Admission errors retain their HTTP status. accepted is emitted only after
    // the same durable transaction used by ordinary Broker calls commits.
    let first = receiver
        .recv()
        .await
        .ok_or_else(|| Error::unavailable("Skill stream admission ended"))??;
    authorize_delivery(&app, &token, peer.ip(), request_id, &first).await?;
    let stream = futures_util::stream::unfold(
        (Some(first), receiver, false),
        move |(first, mut receiver, ended)| {
            let app = app.clone();
            let token = token.clone();
            async move {
                if ended {
                    return None;
                }
                let next = if let Some(first) = first {
                    Some(Ok(first))
                } else {
                    receiver.recv().await
                }?;
                let frame = match next {
                    Ok(frame) => frame,
                    Err(_) => return None,
                };
                let checked = if frame.kind == "accepted" {
                    Ok(())
                } else {
                    authorize_delivery(&app, &token, peer.ip(), request_id, &frame).await
                };
                let frame = match checked {
                    Ok(()) => frame,
                    Err(error) => match frame.fail(error) {
                        Ok(frame) => frame,
                        Err(_) => return None,
                    },
                };
                let ended = matches!(frame.kind, "completed" | "failed" | "canceled");
                Some((Ok::<_, Infallible>(frame.event()), (None, receiver, ended)))
            }
        },
    );
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-request-id", request_id.to_string().parse().unwrap());
    Ok(response)
}

async fn authorize_delivery(
    app: &App,
    token: &str,
    peer: IpAddr,
    request: Uuid,
    frame: &Frame,
) -> Result<()> {
    ensure_active(app).map_err(|error| error.with_resource(frame.call))?;
    let remaining = (frame.deadline - chrono::Utc::now())
        .to_std()
        .unwrap_or(Duration::ZERO);
    let result = tokio::time::timeout(
        remaining,
        authorize_delivery_inner(
            app,
            token,
            peer,
            request,
            frame.call,
            matches!(frame.kind, "item" | "completed"),
            remaining,
        ),
    )
    .await
    .unwrap_or_else(|_| {
        Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Skill stream deadline elapsed during delivery authorization",
            StatusCode::GATEWAY_TIMEOUT,
        ))
    });
    ensure_active(app)
        .and(result)
        .map_err(|error| error.with_resource(frame.call))
}

async fn authorize_delivery_inner(
    app: &App,
    token: &str,
    peer: IpAddr,
    request: Uuid,
    call: Uuid,
    business: bool,
    remaining: Duration,
) -> Result<()> {
    let mut connection = app.db.connect_scoped(remaining).await?;
    let transaction = connection.transaction().await?;
    let ctx = app
        .auth
        .authenticate(&transaction, token, peer, request)
        .await?;
    crate::broker::check_skill_authority(&transaction, &ctx, call).await?;
    crate::host_blobs::check_call_sources(&transaction, &ctx, call).await?;
    let status = transaction
        .query_one("SELECT deadline_at,error FROM calls WHERE id=$1", &[&call])
        .await?;
    if status
        .get::<_, Option<Value>>("error")
        .as_ref()
        .is_some_and(|error| error["code"] == "CANCELED")
    {
        return Err(Error::new(
            "CANCELED",
            "Call cancellation was requested; physical outcome remains unknown",
            StatusCode::CONFLICT,
        ));
    }
    if status.get::<_, chrono::DateTime<chrono::Utc>>("deadline_at") <= chrono::Utc::now() {
        return Err(Error::new(
            "DEADLINE_EXCEEDED",
            "Skill stream deadline elapsed",
            StatusCode::GATEWAY_TIMEOUT,
        ));
    }
    if business {
        // A queue is not authority to keep delivering after its exact physical
        // provider stops, changes generation, or loses deployment approval.
        let live: bool = transaction.query_one(
            "SELECT EXISTS(SELECT 1 FROM calls c JOIN rpc_delegations r ON r.call_id=c.id JOIN instances i ON i.id=c.provider_instance_id AND i.id=r.instance_id AND i.generation=r.generation JOIN deployments d ON d.id=i.deployment_id WHERE c.id=$1 AND i.phase='ready' AND d.archived_at IS NULL AND d.spec->>'enabled'='true')",
            &[&call],
        ).await?.get(0);
        if !live {
            return Err(Error::unavailable("Fixed Skill deployment is unavailable"));
        }
    }
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn skill() -> crate::skills::ResolvedSkill {
        crate::skills::ResolvedSkill {
            id: Uuid::new_v4(),
            installation_id: Uuid::new_v4(),
            definition: json!({"version":"1.0.0"}),
            definition_digest: crate::contracts::digest(b"stream-definition"),
            owner: Uuid::new_v4(),
        }
    }
    #[tokio::test]
    async fn bounded_stream_buffer_requires_consumer_progress_and_terminal_is_final() {
        let (sender, mut receiver) = mpsc::channel(BUFFER_FRAMES);
        let output = Arc::new(Output::new(sender));
        output
            .bind(
                &skill(),
                Uuid::new_v4(),
                chrono::Utc::now() + chrono::Duration::seconds(30),
            )
            .unwrap();
        output.emit("accepted", json!({})).await.unwrap();
        output.emit("item", json!({"value":1})).await.unwrap();
        let pending = {
            let output = output.clone();
            tokio::spawn(async move { output.emit("item", json!({"value":2})).await })
        };
        tokio::task::yield_now().await;
        assert!(
            !pending.is_finished(),
            "a full bounded buffer must apply backpressure"
        );
        let first = receiver.recv().await.unwrap().unwrap();
        assert_eq!(first.sequence, 1);
        pending.await.unwrap().unwrap();
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 2);
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 3);
        output
            .emit("canceled", json!({"reason":"requested"}))
            .await
            .unwrap();
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 4);
        assert!(output.emit("item", json!("after terminal")).await.is_err());
        assert!(receiver.try_recv().is_err());
    }
    #[tokio::test]
    async fn oversized_frame_is_not_emitted_or_allowed_to_consume_a_sequence() {
        let (sender, mut receiver) = mpsc::channel(BUFFER_FRAMES);
        let output = Output::new(sender);
        output
            .bind(
                &skill(),
                Uuid::new_v4(),
                chrono::Utc::now() + chrono::Duration::seconds(30),
            )
            .unwrap();
        output.emit("accepted", json!({})).await.unwrap();
        assert!(output
            .emit("item", json!("x".repeat(MAX_FRAME)))
            .await
            .is_err());
        output
            .emit("failed", Error::invalid("Frame too large").detail())
            .await
            .unwrap();
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 1);
        let failed = receiver.recv().await.unwrap().unwrap();
        assert_eq!(failed.sequence, 2);
        assert_eq!(failed.kind, "failed");
        drop(receiver);
        output.closed().await;
    }
    #[tokio::test]
    async fn deadline_stops_backpressure_and_preserves_the_terminal_sequence() {
        let (sender, mut receiver) = mpsc::channel(BUFFER_FRAMES);
        let output = Output::new(sender);
        output
            .bind(
                &skill(),
                Uuid::new_v4(),
                chrono::Utc::now() + chrono::Duration::seconds(1),
            )
            .unwrap();
        output.emit("accepted", json!({})).await.unwrap();
        output.emit("item", json!({"value":1})).await.unwrap();
        assert!(output.emit("item", json!({"value":2})).await.is_err());
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 1);
        assert_eq!(receiver.recv().await.unwrap().unwrap().sequence, 2);
        output
            .emit("canceled", json!({"reason":"deadline"}))
            .await
            .unwrap();
        let terminal = receiver.recv().await.unwrap().unwrap();
        assert_eq!(terminal.sequence, 3);
        assert_eq!(terminal.kind, "canceled");
        assert!(output.emit("item", json!({"value":3})).await.is_err());
    }
    #[tokio::test]
    async fn queued_business_data_is_fenced_by_readiness_shutdown_and_actual_leader_loss() {
        let Some(f) = crate::backups::tests::Fixture::new().await else {
            return;
        };
        let (sender, mut receiver) = mpsc::channel(BUFFER_FRAMES);
        let output = Output::new(sender);
        output
            .bind(
                &skill(),
                Uuid::new_v4(),
                chrono::Utc::now() + chrono::Duration::seconds(30),
            )
            .unwrap();
        output.emit("accepted", json!({})).await.unwrap();
        output
            .emit("item", json!({"private":"must not escape"}))
            .await
            .unwrap();
        receiver.recv().await.unwrap().unwrap();
        let queued = receiver.recv().await.unwrap().unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        f.app
            .ready
            .store(false, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            authorize_delivery(&f.app, &f.token, peer, Uuid::new_v4(), &queued)
                .await
                .unwrap_err()
                .code,
            "NOT_READY"
        );
        f.app.ready.store(true, std::sync::atomic::Ordering::SeqCst);
        f.app
            .shutdown
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            authorize_delivery(&f.app, &f.token, peer, Uuid::new_v4(), &queued)
                .await
                .unwrap_err()
                .code,
            "NOT_READY"
        );
        f.app
            .shutdown
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let pid: i32 = f
            .app
            .leader
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        let c = f.app.db.connect().await.unwrap();
        assert!(c
            .query_one("SELECT pg_terminate_backend($1)", &[&pid])
            .await
            .unwrap()
            .get::<_, bool>(0));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !f.app.leader.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let error = authorize_delivery(&f.app, &f.token, peer, Uuid::new_v4(), &queued)
            .await
            .unwrap_err();
        assert_eq!(error.code, "NOT_READY");
        let failed = queued.fail(error).unwrap();
        assert_eq!(failed.kind, "failed");
        assert_eq!(failed.sequence, queued.sequence);
        assert!(!failed.data.contains("must not escape"));
    }
}
