//! Pull-driven SSE has no background payload queue. Every body poll resumes
//! current credentials and their complete source graph before leasing data.
use crate::{
    api::{self, App},
    contracts, db,
    error::{Error, Result},
    events,
};
use axum::{
    body::{to_bytes, Body},
    http::{HeaderMap, Request},
    response::{sse::Event, IntoResponse, Response, Sse},
};
use serde_json::json;
use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{atomic::Ordering, Arc, OnceLock},
    time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

struct State {
    app: Arc<App>,
    headers: HeaderMap,
    peer: SocketAddr,
    request: Uuid,
    subscription: Uuid,
    until: Instant,
    last_poll: Instant,
    heartbeat: Instant,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

async fn next(state: &State) -> Result<Option<serde_json::Value>> {
    if state.app.leader.is_closed()
        || state.app.shutdown.load(Ordering::SeqCst)
        || !state.app.ready.load(Ordering::SeqCst)
    {
        return Err(Error::unavailable("Event stream is stopping"));
    }
    let mut c = state.app.db.connect_scoped(Duration::from_secs(10)).await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let ctx = crate::oauth_http::credential_context(
        &tx,
        &state.app.auth,
        &state.headers,
        state.peer,
        state.request,
        state.app.config.text("server", "public_origin"),
    )
    .await?;
    let value = events::lease(&tx, &state.app.auth, &ctx, state.subscription).await?;
    if state.app.leader.is_closed() {
        return Err(Error::unavailable("Controller leadership is lost"));
    }
    tx.commit().await?;
    Ok(value)
}

pub(crate) async fn handle(
    app: Arc<App>,
    peer: SocketAddr,
    request_id: Uuid,
    request: Request<Body>,
) -> Result<Response> {
    if request.method() != axum::http::Method::GET
        || request.uri().query().is_some()
        || request.headers().contains_key("last-event-id")
    {
        return Err(Error::invalid(
            "Event streams require a canonical GET without Last-Event-ID",
        ));
    }
    let (operation, params) =
        api::operation("GET", request.uri().path()).ok_or_else(Error::not_found)?;
    if operation["operationId"] != "streamDeliveries" {
        return Err(Error::not_found());
    }
    let subscription = api::path_id(&params, "subscription_id")?;
    if request.uri().path() != format!("/api/platform/v1/subscriptions/{subscription}/deliveries") {
        return Err(Error::invalid("Noncanonical subscription path"));
    }
    let headers = request.headers().clone();
    contracts::validate_business(
        &json!({"type":"string","minLength":1}),
        &json!(api::bearer(&headers)?),
    )?;
    let bytes = tokio::time::timeout(Duration::from_secs(3), to_bytes(request.into_body(), 0))
        .await
        .map_err(|_| Error::invalid("Event stream request body timed out"))?
        .map_err(|_| Error::invalid("Event streams do not accept a request body"))?;
    if !bytes.is_empty() {
        return Err(Error::invalid("Event streams do not accept a request body"));
    }
    static STREAMS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = STREAMS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(32)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::exhausted())?;
    let state = State {
        app,
        headers,
        peer,
        request: request_id,
        subscription,
        until: Instant::now() + Duration::from_secs(3600),
        last_poll: Instant::now(),
        heartbeat: Instant::now(),
        _permit: permit,
    };
    // Validate current ownership/source before returning HTTP 200, without
    // creating an unseen lease when a client never polls its response body.
    {
        let mut c = state.app.db.connect_scoped(Duration::from_secs(10)).await?;
        let tx = c.transaction().await?;
        db::auth_lock(&tx, false).await?;
        let ctx = crate::oauth_http::credential_context(
            &tx,
            &state.app.auth,
            &state.headers,
            state.peer,
            state.request,
            state.app.config.text("server", "public_origin"),
        )
        .await?;
        events::check_subscription(&tx, &state.app.auth, &ctx, subscription).await?;
        tx.commit().await?;
    }
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        if Instant::now() >= state.until || state.last_poll.elapsed() > Duration::from_secs(30) {
            return None;
        }
        state.last_poll = Instant::now();
        loop {
            if Instant::now() >= state.until {
                return None;
            }
            let value = tokio::time::timeout(Duration::from_secs(10), next(&state)).await;
            match value {
                Ok(Ok(Some(delivery))) => {
                    let event = Event::default()
                        .event("delivery")
                        .id(delivery["delivery_id"].as_str().unwrap_or(""))
                        .data(delivery.to_string());
                    state.last_poll = Instant::now();
                    return Some((Ok::<Event, Infallible>(event), state));
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    tracing::debug!(
                        reason_code = error.code,
                        "Event stream authority or state ended"
                    );
                    return None;
                }
                Err(_) => return None,
            }
            if state.heartbeat.elapsed() >= Duration::from_secs(15) {
                state.heartbeat = Instant::now();
                state.last_poll = Instant::now();
                return Some((Ok(Event::default().comment("ping")), state));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    let mut response = Sse::new(stream).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-request-id", request_id.to_string().parse().unwrap());
    Ok(response)
}
