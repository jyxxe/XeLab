use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Error {
    pub code: &'static str,
    pub message: &'static str,
    pub status: StatusCode,
}

impl Error {
    pub fn new(code: &'static str, message: &'static str, status: StatusCode) -> Self {
        Self {
            code,
            message,
            status,
        }
    }
    pub fn invalid(message: &'static str) -> Self {
        Self::new("INVALID_ARGUMENT", message, StatusCode::BAD_REQUEST)
    }
    pub fn denied() -> Self {
        Self::new(
            "PERMISSION_DENIED",
            "Action or resource is not authorized",
            StatusCode::FORBIDDEN,
        )
    }
    pub fn credential() -> Self {
        Self::new(
            "INVALID_CREDENTIAL",
            "Invalid credentials",
            StatusCode::UNAUTHORIZED,
        )
    }
    pub fn not_found() -> Self {
        Self::new(
            "RESOURCE_NOT_FOUND",
            "Resource is not available",
            StatusCode::NOT_FOUND,
        )
    }
    pub fn conflict(message: &'static str) -> Self {
        Self::new("STATE_CONFLICT", message, StatusCode::CONFLICT)
    }
    pub fn unavailable(message: &'static str) -> Self {
        Self::new("NOT_READY", message, StatusCode::SERVICE_UNAVAILABLE)
    }
    pub fn exhausted() -> Self {
        Self::new(
            "RESOURCE_EXHAUSTED",
            "Resource capacity is unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
        )
    }
    pub fn detail(&self) -> Value {
        json!({"code":self.code,"message":self.message,"retryable":false,"violations":[],"retry_after_ms":null})
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let id = Uuid::new_v4().to_string();
        let mut response = (
            self.status,
            Json(json!({"error":self.detail(),"request_id":id})),
        )
            .into_response();
        response
            .headers_mut()
            .insert("x-request-id", id.parse().unwrap());
        response
            .headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        response
    }
}
impl From<tokio_postgres::Error> for Error {
    fn from(_: tokio_postgres::Error) -> Self {
        Self::unavailable("Metadata transaction failed")
    }
}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::unavailable("Host operation failed")
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("Invalid JSON")
    }
}
impl From<tonic::Status> for Error {
    fn from(_: tonic::Status) -> Self {
        Self::unavailable("Plugin RPC failed")
    }
}
