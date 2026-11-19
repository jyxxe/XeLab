//! Progress framing for an already-started TaskWorker Execute stream.

use crate::rpc::{
    task_progress::Body, task_result::Outcome, Checkpoint, ErrorDetail, ExecutionState, Heartbeat,
    TaskProgress, TaskResult,
};
use std::{fmt, future::Future};
use tokio::time::{timeout_at, Instant};
use tonic::{metadata::MetadataMap, Code, Status, Streaming};

const MAX_SAFE_COUNTER: u64 = (1 << 53) - 1;
const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum ProgressEvent {
    Heartbeat(Heartbeat),
    Checkpoint(Checkpoint),
    Terminal(TaskOutcome),
}

/// The peer's reported outcome. Validate JSON and remote error codes against
/// the original execution contract before trusting their business meaning.
#[derive(Debug, PartialEq)]
pub enum TaskOutcome {
    Succeeded(Vec<u8>),
    Failed(ErrorDetail),
    Canceled,
    /// The worker reports that the execution outcome is still unknown.
    Unknown,
}

/// An unreadable or unconfirmed stream. An error does not establish whether
/// the worker executed the task or whether it is safe to submit it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressError {
    InvalidExpectedIdentity,
    WrongIdentity,
    WrongSequence,
    MissingBody,
    InvalidHeartbeat,
    InvalidCheckpoint,
    InvalidTerminal,
    MissingTerminal,
    TrailingFrame,
    InvalidFinalStatus,
    DeadlineExceeded,
    Transport(Code),
}

impl fmt::Display for ProgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidExpectedIdentity => "Invalid expected TaskWorker execution ID or fence",
            Self::WrongIdentity => "TaskWorker execution ID or fence does not match",
            Self::WrongSequence => "TaskWorker sequence is out of order or out of range",
            Self::MissingBody => "TaskWorker progress frame has no body",
            Self::InvalidHeartbeat => "TaskWorker heartbeat percent is out of range",
            Self::InvalidCheckpoint => "TaskWorker checkpoint fields are invalid",
            Self::InvalidTerminal => "TaskWorker terminal state or outcome is invalid",
            Self::MissingTerminal => "TaskWorker stream ended without a result",
            Self::TrailingFrame => "TaskWorker sent a frame after the result",
            Self::InvalidFinalStatus => "TaskWorker stream ended without an unambiguous OK status",
            Self::DeadlineExceeded => "TaskWorker stream deadline expired",
            Self::Transport(code) => return write!(f, "TaskWorker stream failed with {code}"),
        })
    }
}

impl std::error::Error for ProgressError {}

/// Reads a single Execute stream bound to the identity sent in its request.
/// A terminal outcome is emitted only after clean gRPC completion. No call
/// is issued or retried and no checkpoint is persisted by this reader.
pub struct TaskProgressReader {
    stream: Option<Streaming<TaskProgress>>,
    execution_id: String,
    fence: u64,
    deadline: Instant,
    next_sequence: u64,
    pending_result: Option<TaskOutcome>,
    error: Option<ProgressError>,
}

impl TaskProgressReader {
    /// Use the execution ID and fence of the original Execute request and its
    /// already-bounded absolute deadline. Configure the generated client with
    /// the SDK RPC message limit before starting the request.
    pub fn new(
        stream: Streaming<TaskProgress>,
        execution_id: &str,
        fence: u64,
        deadline: Instant,
    ) -> Result<Self, ProgressError> {
        if !is_uuid(execution_id) || !(1..=MAX_SAFE_COUNTER).contains(&fence) {
            return Err(ProgressError::InvalidExpectedIdentity);
        }
        Ok(Self {
            stream: Some(stream),
            execution_id: execution_id.to_owned(),
            fence,
            deadline,
            next_sequence: 1,
            pending_result: None,
            error: None,
        })
    }

    /// Return one validated event. `None` follows only a terminal event;
    /// after any failure the same error is returned on subsequent reads.
    pub async fn next_event(&mut self) -> Result<Option<ProgressEvent>, ProgressError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self.stream.is_none() {
            return Ok(None);
        }
        let event = self.read_event().await;
        match &event {
            Ok(ProgressEvent::Terminal(_)) => self.stream = None,
            Err(error) => {
                self.stream = None;
                self.pending_result = None;
                self.error = Some(*error);
            }
            _ => (),
        }
        event.map(Some)
    }

    async fn read_event(&mut self) -> Result<ProgressEvent, ProgressError> {
        if self.pending_result.is_none() {
            let frame = self
                .receive()
                .await?
                .ok_or(ProgressError::MissingTerminal)?;
            self.check_identity(&frame.execution_id, frame.fence)?;
            if frame.sequence != self.next_sequence || frame.sequence > MAX_SAFE_COUNTER {
                return Err(ProgressError::WrongSequence);
            }
            self.next_sequence += 1;

            match frame.body.ok_or(ProgressError::MissingBody)? {
                Body::Heartbeat(heartbeat) => {
                    return if heartbeat.progress_percent <= 100 {
                        Ok(ProgressEvent::Heartbeat(heartbeat))
                    } else {
                        Err(ProgressError::InvalidHeartbeat)
                    };
                }
                Body::Checkpoint(checkpoint) => {
                    return if is_uuid(&checkpoint.blob_id)
                        && !checkpoint.data_schema_version.is_empty()
                        && !checkpoint.plugin_version.is_empty()
                    {
                        Ok(ProgressEvent::Checkpoint(checkpoint))
                    } else {
                        Err(ProgressError::InvalidCheckpoint)
                    };
                }
                Body::Result(result) => {
                    self.check_identity(&result.execution_id, result.fence)?;
                    // Keep the result across canceled polls while awaiting trailers.
                    self.pending_result = Some(terminal(result)?);
                }
            }
        }
        if self.receive().await?.is_some() {
            return Err(ProgressError::TrailingFrame);
        }
        // Tonic can return EOF for an HTTP 200 response without gRPC status.
        // Require an explicit successful trailer before exposing the result.
        let stream = self.stream.as_mut().expect("live progress stream");
        validate_final_status(before_deadline(self.deadline, stream.trailers()).await?)?;
        Ok(ProgressEvent::Terminal(
            self.pending_result
                .take()
                .expect("validated terminal frame"),
        ))
    }

    fn check_identity(&self, execution_id: &str, fence: u64) -> Result<(), ProgressError> {
        if fence == self.fence && execution_id.eq_ignore_ascii_case(&self.execution_id) {
            Ok(())
        } else {
            Err(ProgressError::WrongIdentity)
        }
    }

    async fn receive(&mut self) -> Result<Option<TaskProgress>, ProgressError> {
        let stream = self.stream.as_mut().expect("live progress stream");
        before_deadline(self.deadline, stream.message()).await
    }
}

async fn before_deadline<T>(
    deadline: Instant,
    read: impl Future<Output = Result<T, Status>>,
) -> Result<T, ProgressError> {
    if Instant::now() >= deadline {
        return Err(ProgressError::DeadlineExceeded);
    }
    let result = timeout_at(deadline, read).await;
    if Instant::now() >= deadline {
        return Err(ProgressError::DeadlineExceeded);
    }
    result
        .map_err(|_| ProgressError::DeadlineExceeded)?
        .map_err(|status| ProgressError::Transport(status.code()))
}

fn validate_final_status(trailers: Option<MetadataMap>) -> Result<(), ProgressError> {
    let trailers = trailers.ok_or(ProgressError::InvalidFinalStatus)?;
    let statuses = trailers.get_all("grpc-status");
    let mut values = statuses.iter();
    if matches!(values.next(), Some(value) if value.as_encoded_bytes() == b"0")
        && values.next().is_none()
    {
        Ok(())
    } else {
        Err(ProgressError::InvalidFinalStatus)
    }
}

fn terminal(result: TaskResult) -> Result<TaskOutcome, ProgressError> {
    match (ExecutionState::try_from(result.state), result.outcome) {
        (Ok(ExecutionState::Succeeded), Some(Outcome::OutputJson(output)))
            if !output.is_empty()
                && output.len() <= MAX_OUTPUT_BYTES
                && std::str::from_utf8(&output).is_ok() =>
        {
            Ok(TaskOutcome::Succeeded(output))
        }
        (Ok(ExecutionState::Failed), Some(Outcome::Error(error)))
            if !error.code.is_empty() && !error.message.is_empty() =>
        {
            Ok(TaskOutcome::Failed(error))
        }
        (Ok(ExecutionState::Canceled), None) => Ok(TaskOutcome::Canceled),
        (Ok(ExecutionState::Unknown), None) => Ok(TaskOutcome::Unknown),
        _ => Err(ProgressError::InvalidTerminal),
    }
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_requires_one_explicit_ok_status() {
        let mut trailers = MetadataMap::new();
        let invalid = Err(ProgressError::InvalidFinalStatus);
        assert_eq!(validate_final_status(None), invalid);
        assert_eq!(validate_final_status(Some(trailers.clone())), invalid);
        trailers.insert("grpc-status", "0".parse().unwrap());
        assert_eq!(validate_final_status(Some(trailers.clone())), Ok(()));
        trailers.append("grpc-status", "14".parse().unwrap());
        assert_eq!(validate_final_status(Some(trailers)), invalid);
    }
}
