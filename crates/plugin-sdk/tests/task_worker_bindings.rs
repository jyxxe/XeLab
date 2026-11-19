#![allow(clippy::result_large_err)] // Generated Tonic service signatures use tonic::Status.

use std::{pin::Pin, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::{wrappers::TcpListenerStream, Stream};
use tonic::{
    transport::{Endpoint, Server},
    Request, Response, Status,
};
use xelab_plugin_sdk::rpc::{
    task_progress::Body,
    task_result::Outcome,
    task_worker_client::TaskWorkerClient,
    task_worker_server::{TaskWorker, TaskWorkerServer},
    Ack, CancelTaskRequest, Checkpoint, ExecuteTaskRequest, ExecutionState, Heartbeat,
    QueryTaskRequest, TaskProgress, TaskResult,
};

const EXECUTION: &str = "00000000-0000-4000-8000-000000000001";
const CHECKPOINT: &str = "00000000-0000-4000-8000-000000000002";
const FENCE: u64 = 7;

struct FixtureWorker;

#[tonic::async_trait]
impl TaskWorker for FixtureWorker {
    type ExecuteStream = Pin<Box<dyn Stream<Item = Result<TaskProgress, Status>> + Send>>;

    async fn execute(
        &self,
        request: Request<ExecuteTaskRequest>,
    ) -> Result<Response<Self::ExecuteStream>, Status> {
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

#[tokio::test]
async fn generated_taskworker_bindings_round_trip_on_loopback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(
        Server::builder()
            .add_service(TaskWorkerServer::new(FixtureWorker))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    let channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(3))
        .connect()
        .await
        .unwrap();
    let mut worker = TaskWorkerClient::new(channel);

    let input = br#"{"fixture":true}"#.to_vec();
    let mut stream = worker
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
    let expected = [
        Body::Heartbeat(Heartbeat {
            progress_percent: 10,
            phase: "work".into(),
        }),
        Body::Checkpoint(Checkpoint {
            blob_id: CHECKPOINT.into(),
            data_schema_version: "1".into(),
            plugin_version: "1.0.0".into(),
        }),
        Body::Result(TaskResult {
            execution_id: EXECUTION.into(),
            fence: FENCE,
            state: ExecutionState::Succeeded as i32,
            outcome: Some(Outcome::OutputJson(input)),
        }),
    ];
    for (index, body) in expected.into_iter().enumerate() {
        let frame = stream.message().await.unwrap().unwrap();
        assert_eq!(frame.execution_id, EXECUTION);
        assert_eq!(frame.fence, FENCE);
        assert_eq!(frame.sequence, index as u64 + 1);
        assert_eq!(frame.body, Some(body));
    }
    assert!(stream.message().await.unwrap().is_none());
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
