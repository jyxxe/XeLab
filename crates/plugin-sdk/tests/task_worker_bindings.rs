#![allow(clippy::result_large_err)] // Generated Tonic service signatures use tonic::Status.

use futures_util::StreamExt;
use std::{pin::Pin, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Notify, task::JoinHandle, time::Instant};
use tokio_stream::{wrappers::TcpListenerStream, Stream};
use tonic::{
    transport::{Channel, Endpoint, Server},
    Code, Request, Response, Status,
};
use xelab_plugin_sdk::rpc::{
    task_progress::Body,
    task_result::Outcome,
    task_worker_client::TaskWorkerClient,
    task_worker_server::{TaskWorker, TaskWorkerServer},
    Ack, CancelTaskRequest, Checkpoint, ErrorDetail, ExecuteTaskRequest, ExecutionState, Heartbeat,
    QueryTaskRequest, TaskProgress, TaskResult,
};
use xelab_plugin_sdk::{
    task_worker::{ProgressError, ProgressEvent, TaskOutcome, TaskProgressReader},
    transport::MAX_RPC_MESSAGE_BYTES,
};

const EXECUTION: &str = "00000000-0000-4000-8000-000000000001";
const CHECKPOINT: &str = "00000000-0000-4000-8000-000000000002";
const FENCE: u64 = 7;

enum FixtureWorker {
    Echo,
    Script(Vec<Result<TaskProgress, Status>>),
    DeferredEnd(TaskProgress, Arc<Notify>),
}

#[tonic::async_trait]
impl TaskWorker for FixtureWorker {
    type ExecuteStream = Pin<Box<dyn Stream<Item = Result<TaskProgress, Status>> + Send>>;

    async fn execute(
        &self,
        request: Request<ExecuteTaskRequest>,
    ) -> Result<Response<Self::ExecuteStream>, Status> {
        match self {
            Self::Echo => (),
            Self::Script(script) => {
                return Ok(Response::new(Box::pin(tokio_stream::iter(script.clone()))))
            }
            Self::DeferredEnd(result, complete) => {
                let complete = complete.clone();
                let end = futures_util::stream::once(async move {
                    complete.notified().await;
                    None::<Result<TaskProgress, Status>>
                })
                .filter_map(futures_util::future::ready);
                return Ok(Response::new(Box::pin(
                    futures_util::stream::once(futures_util::future::ready(Ok(result.clone())))
                        .chain(end),
                )));
            }
        }
        let request = request.into_inner();
        let progress = |sequence, body| {
            Ok(TaskProgress {
                execution_id: request.execution_id.clone(),
                fence: request.fence,
                sequence,
                body: Some(body),
            })
        };
        let events = [
            progress(
                1,
                Body::Heartbeat(Heartbeat {
                    progress_percent: 10,
                    phase: "work".into(),
                }),
            ),
            progress(
                2,
                Body::Checkpoint(Checkpoint {
                    blob_id: request.checkpoint_blob_id.unwrap_or_default(),
                    data_schema_version: "1".into(),
                    plugin_version: "1.0.0".into(),
                }),
            ),
            progress(
                3,
                Body::Result(TaskResult {
                    execution_id: request.execution_id.clone(),
                    fence: request.fence,
                    state: ExecutionState::Succeeded as i32,
                    outcome: Some(Outcome::OutputJson(request.input_json)),
                }),
            ),
        ];
        Ok(Response::new(Box::pin(tokio_stream::iter(events))))
    }

    async fn cancel(&self, request: Request<CancelTaskRequest>) -> Result<Response<Ack>, Status> {
        let request = request.into_inner();
        Ok(Response::new(Ack {
            accepted: request.execution_id == EXECUTION && request.fence == FENCE,
            ..Ack::default()
        }))
    }

    async fn query(
        &self,
        request: Request<QueryTaskRequest>,
    ) -> Result<Response<TaskResult>, Status> {
        Ok(Response::new(TaskResult {
            execution_id: request.into_inner().execution_id,
            fence: FENCE,
            state: ExecutionState::Unknown as i32,
            outcome: None,
        }))
    }
}

async fn connect(
    fixture: FixtureWorker,
) -> (
    TaskWorkerClient<Channel>,
    JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        Server::builder()
            .add_service(TaskWorkerServer::new(fixture))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(3))
        .connect()
        .await
        .unwrap();
    (
        TaskWorkerClient::new(channel).max_decoding_message_size(MAX_RPC_MESSAGE_BYTES),
        server,
    )
}

#[tokio::test]
async fn generated_taskworker_bindings_round_trip_on_loopback() {
    let (mut worker, server) = connect(FixtureWorker::Echo).await;

    let input = br#"{"fixture":true}"#.to_vec();
    let stream = worker
        .execute(ExecuteTaskRequest {
            execution_id: EXECUTION.into(),
            fence: FENCE,
            input_json: input.clone(),
            checkpoint_blob_id: Some(CHECKPOINT.into()),
            ..ExecuteTaskRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    let mut stream = TaskProgressReader::new(
        stream,
        EXECUTION,
        FENCE,
        Instant::now() + Duration::from_secs(3),
    )
    .unwrap();
    let expected = [
        ProgressEvent::Heartbeat(Heartbeat {
            progress_percent: 10,
            phase: "work".into(),
        }),
        ProgressEvent::Checkpoint(Checkpoint {
            blob_id: CHECKPOINT.into(),
            data_schema_version: "1".into(),
            plugin_version: "1.0.0".into(),
        }),
        ProgressEvent::Terminal(TaskOutcome::Succeeded(input)),
    ];
    for event in expected {
        assert_eq!(stream.next_event().await.unwrap(), Some(event));
    }
    assert!(stream.next_event().await.unwrap().is_none());
    assert!(
        worker
            .cancel(CancelTaskRequest {
                execution_id: EXECUTION.into(),
                fence: FENCE,
                ..CancelTaskRequest::default()
            })
            .await
            .unwrap()
            .into_inner()
            .accepted
    );
    let result = worker
        .query(QueryTaskRequest {
            execution_id: EXECUTION.into(),
            ..QueryTaskRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(result.execution_id, EXECUTION);
    assert_eq!(result.fence, FENCE);
    assert_eq!(result.state(), ExecutionState::Unknown);
    assert_eq!(result.outcome, None);
    server.abort();
}

fn frame(sequence: u64, body: Body) -> TaskProgress {
    TaskProgress {
        execution_id: EXECUTION.into(),
        fence: FENCE,
        sequence,
        body: Some(body),
    }
}

fn result(state: ExecutionState, outcome: Option<Outcome>) -> TaskProgress {
    frame(
        1,
        Body::Result(TaskResult {
            execution_id: EXECUTION.into(),
            fence: FENCE,
            state: state as i32,
            outcome,
        }),
    )
}

async fn read_script(
    fixture: FixtureWorker,
    budget: Duration,
) -> (
    TaskProgressReader,
    JoinHandle<Result<(), tonic::transport::Error>>,
) {
    let (mut worker, server) = connect(fixture).await;
    let stream = worker
        .execute(ExecuteTaskRequest {
            execution_id: EXECUTION.into(),
            fence: FENCE,
            ..ExecuteTaskRequest::default()
        })
        .await
        .unwrap()
        .into_inner();
    (
        TaskProgressReader::new(stream, EXECUTION, FENCE, Instant::now() + budget).unwrap(),
        server,
    )
}

#[tokio::test]
async fn invalid_fence_sequence_and_payloads_never_become_progress() {
    let beat = frame(1, Body::Heartbeat(Heartbeat::default()));
    let bad_result = TaskResult {
        execution_id: EXECUTION.into(),
        fence: FENCE + 1,
        state: ExecutionState::Unknown as i32,
        outcome: None,
    };
    for (frame, error) in [
        (
            TaskProgress {
                fence: FENCE + 1,
                ..beat.clone()
            },
            ProgressError::WrongIdentity,
        ),
        (
            TaskProgress {
                execution_id: CHECKPOINT.into(),
                ..beat.clone()
            },
            ProgressError::WrongIdentity,
        ),
        (
            frame(1, Body::Result(bad_result)),
            ProgressError::WrongIdentity,
        ),
        (
            TaskProgress {
                sequence: 2,
                ..beat.clone()
            },
            ProgressError::WrongSequence,
        ),
        (
            TaskProgress { body: None, ..beat },
            ProgressError::MissingBody,
        ),
        (
            frame(
                1,
                Body::Heartbeat(Heartbeat {
                    progress_percent: 101,
                    phase: String::new(),
                }),
            ),
            ProgressError::InvalidHeartbeat,
        ),
        (
            frame(1, Body::Checkpoint(Checkpoint::default())),
            ProgressError::InvalidCheckpoint,
        ),
        (
            result(ExecutionState::Succeeded, None),
            ProgressError::InvalidTerminal,
        ),
        (
            result(
                ExecutionState::Succeeded,
                Some(Outcome::OutputJson(vec![0xff])),
            ),
            ProgressError::InvalidTerminal,
        ),
        (
            result(ExecutionState::Accepted, None),
            ProgressError::InvalidTerminal,
        ),
        (
            result(
                ExecutionState::Unknown,
                Some(Outcome::Error(ErrorDetail::default())),
            ),
            ProgressError::InvalidTerminal,
        ),
    ] {
        let (mut stream, server) = read_script(
            FixtureWorker::Script(vec![Ok(frame)]),
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(stream.next_event().await, Err(error));
        assert_eq!(stream.next_event().await, Err(error));
        server.abort();
    }
}

#[tokio::test]
async fn terminal_requires_clean_eof_and_preserves_outcome_uncertainty() {
    let (mut stream, server) = read_script(
        FixtureWorker::Script(vec![Ok(frame(1, Body::Heartbeat(Heartbeat::default())))]),
        Duration::from_secs(3),
    )
    .await;
    assert!(matches!(
        stream.next_event().await,
        Ok(Some(ProgressEvent::Heartbeat(_)))
    ));
    assert_eq!(
        stream.next_event().await,
        Err(ProgressError::MissingTerminal)
    );
    server.abort();

    let terminal = result(ExecutionState::Unknown, None);
    for (extra, error) in [
        (
            Ok(TaskProgress {
                sequence: 2,
                ..terminal.clone()
            }),
            ProgressError::TrailingFrame,
        ),
        (
            Err(Status::unavailable("test trailer")),
            ProgressError::Transport(Code::Unavailable),
        ),
    ] {
        let (mut stream, server) = read_script(
            FixtureWorker::Script(vec![Ok(terminal.clone()), extra]),
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(stream.next_event().await, Err(error));
        server.abort();
    }

    let failed = ErrorDetail {
        code: "INVALID_ARGUMENT".into(),
        message: "fixture failure".into(),
        ..ErrorDetail::default()
    };
    for (terminal, outcome) in [
        (terminal, TaskOutcome::Unknown),
        (
            result(ExecutionState::Canceled, None),
            TaskOutcome::Canceled,
        ),
        (
            result(ExecutionState::Failed, Some(Outcome::Error(failed.clone()))),
            TaskOutcome::Failed(failed),
        ),
    ] {
        let (mut stream, server) = read_script(
            FixtureWorker::Script(vec![Ok(terminal)]),
            Duration::from_secs(3),
        )
        .await;
        assert_eq!(
            stream.next_event().await,
            Ok(Some(ProgressEvent::Terminal(outcome)))
        );
        assert_eq!(stream.next_event().await, Ok(None));
        server.abort();
    }
}

#[tokio::test]
async fn result_wait_can_be_repolled_and_is_bounded_by_the_original_deadline() {
    let terminal = result(ExecutionState::Unknown, None);
    let release = Arc::new(Notify::new());
    let (mut stream, server) = read_script(
        FixtureWorker::DeferredEnd(terminal.clone(), release.clone()),
        Duration::from_secs(3),
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), stream.next_event())
            .await
            .is_err()
    );
    release.notify_one();
    assert_eq!(
        stream.next_event().await,
        Ok(Some(ProgressEvent::Terminal(TaskOutcome::Unknown)))
    );
    assert_eq!(stream.next_event().await, Ok(None));
    server.abort();

    let (mut stream, server) = read_script(
        FixtureWorker::DeferredEnd(terminal, Arc::new(Notify::new())),
        Duration::from_millis(30),
    )
    .await;
    assert_eq!(
        stream.next_event().await,
        Err(ProgressError::DeadlineExceeded)
    );
    server.abort();
}
