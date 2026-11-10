//! Admission before request-body polling, held through the response lifetime.
use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
pub(crate) struct Budget {
    requests: Arc<Semaphore>,
    probes: Arc<Semaphore>,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            requests: Arc::new(Semaphore::new(2)),
            probes: Arc::new(Semaphore::new(1)),
        }
    }
    fn acquire(&self, request: &Request<Body>) -> crate::error::Result<OwnedSemaphorePermit> {
        // This lane grants no authority: normal Host/security routing still runs.
        let pool = if request.method() == "GET" && request.uri().path() == "/health/live" {
            &self.probes
        } else {
            &self.requests
        };
        pool.clone()
            .try_acquire_owned()
            .map_err(|_| crate::error::Error::exhausted())
    }
}

pub(crate) async fn limit(
    State(budget): State<Budget>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let permit = match budget.acquire(&request) {
        Ok(permit) => permit,
        Err(error) => {
            let id = uuid::Uuid::new_v4();
            let mut response = crate::api::response(
                json!({"error":error.detail(),"request_id":id}),
                error.status.as_u16(),
                id,
            );
            response
                .headers_mut()
                .insert("retry-after", "1".parse().unwrap());
            return response;
        }
    };
    // Binary Blob uploads already stream through their own byte/quota bounds.
    // Everything else is limited while read, without buffering another copy.
    if !crate::api::operation(request.method().as_str(), request.uri().path())
        .is_some_and(|(operation, _)| operation["operationId"] == "uploadBlob")
    {
        let body = std::mem::replace(request.body_mut(), Body::empty());
        *request.body_mut() = Body::new(http_body_util::Limited::new(body, 256 * 1024));
    }
    retain(next.run(request).await, permit)
}

fn retain(response: Response, permit: OwnedSemaphorePermit) -> Response {
    let (parts, body) = response.into_parts();
    let body = Body::new(crate::compact_rpc::retain_body(body, permit));
    Response::from_parts(parts, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{middleware, Router};
    use futures_util::StreamExt;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn request(path: &str) -> Request<Body> {
        Request::builder().uri(path).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn overload_rejects_before_body_poll_and_preserves_probe_lane() {
        let budget = Budget::new();
        let app = Router::new()
            .fallback(|| async { "ok" })
            .layer(middleware::from_fn_with_state(budget.clone(), limit));
        let one = app.clone().oneshot(request("/one")).await.unwrap();
        let two = app.clone().oneshot(request("/two")).await.unwrap();
        let body = Body::from_stream(futures_util::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<axum::body::Bytes, std::io::Error>>> {
                panic!("overloaded body was polled")
            },
        ));
        let rejected = app
            .clone()
            .oneshot(Request::builder().uri("/three").body(body).unwrap())
            .await
            .unwrap();
        assert_eq!(rejected.status(), crate::error::Error::exhausted().status);
        let probe = app.clone().oneshot(request("/health/live")).await.unwrap();
        assert_eq!(probe.status(), 200);
        drop((one, two, probe));
        assert_eq!(app.oneshot(request("/again")).await.unwrap().status(), 200);
    }

    #[tokio::test]
    async fn permit_is_retained_after_eof_until_response_body_is_dropped() {
        let budget = Budget::new();
        let permit = budget.acquire(&request("/")).unwrap();
        let response = retain(Response::new(Body::from("payload")), permit);
        let mut body = response.into_body().into_data_stream();
        assert_eq!(body.next().await.unwrap().unwrap(), "payload");
        assert!(body.next().await.is_none());
        assert_eq!(budget.requests.available_permits(), 1);
        drop(body);
        assert_eq!(budget.requests.available_permits(), 2);
    }

    #[tokio::test]
    async fn response_trailers_are_preserved() {
        let budget = Budget::new();
        let permit = budget.acquire(&request("/")).unwrap();
        let mut trailers = axum::http::HeaderMap::new();
        trailers.insert("x-checksum", "verified".parse().unwrap());
        let frames = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(http_body::Frame::data(axum::body::Bytes::from_static(
                b"payload",
            ))),
            Ok(http_body::Frame::trailers(trailers)),
        ]);
        let response = retain(
            Response::new(Body::new(http_body_util::StreamBody::new(frames))),
            permit,
        );
        let collected = response.into_body().collect().await.unwrap();
        assert_eq!(collected.trailers().unwrap()["x-checksum"], "verified");
        assert_eq!(collected.to_bytes(), "payload");
        assert_eq!(budget.requests.available_permits(), 2);
    }

    #[tokio::test]
    async fn compact_json_is_bounded_while_reading_but_raw_blob_uploads_stream() {
        let app = Router::new()
            .fallback(|request: Request<Body>| async move {
                match request.into_body().collect().await {
                    Ok(_) => axum::http::StatusCode::OK,
                    Err(_) => axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                }
            })
            .layer(middleware::from_fn_with_state(Budget::new(), limit));
        for (method, path, expected) in [
            ("POST", "/api/platform/v1/calls", 413),
            ("POST", "/plugins/org.example.test/run", 413),
            ("POST", "/api/platform/v1/blobs", 200),
        ] {
            let chunks = futures_util::stream::iter([
                Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![b' '; 128 * 1024])),
                Ok(axum::body::Bytes::from(vec![b' '; 128 * 1024])),
                Ok(axum::body::Bytes::from_static(b"x")),
            ]);
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::from_stream(chunks))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), expected, "{path}");
            drop(response);
        }
    }
}
