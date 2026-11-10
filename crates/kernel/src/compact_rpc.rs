//! A shared compact HostService budget, including response bodies and trailers.
use axum::http::{Request, Response};
use futures_util::future::BoxFuture;
use http_body::Body;
use http_body_util::BodyExt;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tonic::body::BoxBody;
use tower::{Layer, Service};

/// Mapping frames retains the lease even after EOF, without discarding trailers
/// (a data-only stream adapter would silently drop gRPC status and HTTP trailers).
pub(crate) fn retain_body<B>(
    body: B,
    permit: OwnedSemaphorePermit,
) -> impl Body<Data = B::Data, Error = B::Error> + Send
where
    B: Body + Send,
{
    body.map_frame(move |frame| {
        let _held = &permit;
        frame
    })
}

#[derive(Clone)]
pub(crate) struct BudgetLayer {
    slots: Option<Arc<Semaphore>>,
}
impl BudgetLayer {
    pub(crate) fn new(compact: bool) -> Self {
        Self {
            slots: compact.then(|| Arc::new(Semaphore::new(2))),
        }
    }
}
impl<S> Layer<S> for BudgetLayer {
    type Service = BudgetService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        BudgetService {
            inner,
            slots: self.slots.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BudgetService<S> {
    inner: S,
    slots: Option<Arc<Semaphore>>,
}
impl<S, B> Service<Request<B>> for BudgetService<S>
where
    S: Service<Request<B>, Response = Response<BoxBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    B: Send + 'static,
{
    type Response = Response<BoxBody>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.slots.is_some() {
            // Admission happens in call, before decoding. Do not wait in an
            // inner concurrency layer when the shared body budget is full.
            Poll::Ready(Ok(()))
        } else {
            self.inner.poll_ready(cx)
        }
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let Some(slots) = &self.slots else {
            return Box::pin(self.inner.call(request));
        };
        let permit = match slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Box::pin(async {
                    Ok(
                        tonic::Status::resource_exhausted("HostService capacity is unavailable")
                            .into_http(),
                    )
                });
            }
        };
        let mut inner = self.inner.clone();
        Box::pin(async move {
            futures_util::future::poll_fn(|cx| inner.poll_ready(cx)).await?;
            let response = inner.call(request).await?;
            let (parts, body) = response.into_parts();
            Ok(Response::from_parts(
                parts,
                retain_body(body, permit).boxed_unsync(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use http_body::Frame;
    use http_body_util::{Full, StreamBody};
    use std::convert::Infallible;
    use tower::ServiceExt;

    fn response_body() -> BoxBody {
        Full::new(Bytes::from_static(b"reply"))
            .map_err(|error: Infallible| match error {})
            .boxed_unsync()
    }

    #[tokio::test]
    async fn retained_responses_share_a_fail_fast_budget_before_request_polling() {
        let service = BudgetLayer::new(true).layer(tower::service_fn(
            |request: Request<axum::body::Body>| async move {
                request.into_body().collect().await.unwrap();
                Ok::<_, Infallible>(Response::new(response_body()))
            },
        ));
        let first = service
            .clone()
            .oneshot(Request::new(axum::body::Body::empty()))
            .await
            .unwrap();
        let second = service
            .clone()
            .oneshot(Request::new(axum::body::Body::empty()))
            .await
            .unwrap();
        let unread = axum::body::Body::from_stream(futures_util::stream::poll_fn(
            |_| -> Poll<Option<Result<Bytes, std::io::Error>>> {
                panic!("rejected gRPC request body was polled")
            },
        ));
        let rejected = service.clone().oneshot(Request::new(unread)).await.unwrap();
        assert_eq!(rejected.headers()["grpc-status"], "8");
        assert_eq!(rejected.headers()["content-type"], "application/grpc");
        drop(first);
        let accepted = service
            .oneshot(Request::new(axum::body::Body::empty()))
            .await
            .unwrap();
        assert!(!accepted.headers().contains_key("grpc-status"));
        drop((second, accepted));
    }

    #[tokio::test]
    async fn streaming_frames_trailers_and_eof_retain_the_same_permit() {
        let layer = BudgetLayer::new(true);
        let slots = layer.slots.clone().unwrap();
        let service = layer.layer(tower::service_fn(|_: Request<()>| async {
            let mut trailers = axum::http::HeaderMap::new();
            trailers.insert("grpc-status", "0".parse().unwrap());
            let frames = vec![
                Ok::<_, tonic::Status>(Frame::data(Bytes::from_static(b"chunk"))),
                Ok(Frame::trailers(trailers)),
            ];
            Ok::<_, Infallible>(Response::new(
                StreamBody::new(futures_util::stream::iter(frames)).boxed_unsync(),
            ))
        }));
        let response = service.oneshot(Request::new(())).await.unwrap();
        let mut body = response.into_body();
        assert_eq!(slots.available_permits(), 1);
        assert_eq!(
            body.frame().await.unwrap().unwrap().into_data().unwrap(),
            "chunk"
        );
        let trailers = body
            .frame()
            .await
            .unwrap()
            .unwrap()
            .into_trailers()
            .unwrap();
        assert_eq!(trailers["grpc-status"], "0");
        assert!(body.frame().await.is_none());
        assert_eq!(slots.available_permits(), 1);
        drop(body);
        assert_eq!(slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn cancellation_returns_handler_capacity_and_standard_has_no_body_limit() {
        let layer = BudgetLayer::new(true);
        let slots = layer.slots.clone().unwrap();
        let (send, started) = tokio::sync::oneshot::channel();
        let send = Arc::new(std::sync::Mutex::new(Some(send)));
        let service = layer.layer(tower::service_fn(move |_: Request<()>| {
            let send = send.clone();
            async move {
                send.lock().unwrap().take().unwrap().send(()).unwrap();
                futures_util::future::pending::<Result<Response<BoxBody>, Infallible>>().await
            }
        }));
        let task = tokio::spawn(service.oneshot(Request::new(())));
        started.await.unwrap();
        assert_eq!(slots.available_permits(), 1);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(slots.available_permits(), 2);

        let standard = BudgetLayer::new(false).layer(tower::service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(response_body()))
        }));
        let mut responses = Vec::new();
        for _ in 0..3 {
            let response = standard.clone().oneshot(Request::new(())).await.unwrap();
            assert!(!response.headers().contains_key("grpc-status"));
            responses.push(response);
        }
    }
}
