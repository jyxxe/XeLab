use axum::{
    extract::RawQuery,
    http::{header, StatusCode},
    routing::get,
    Router,
};
use serde_json::Value;
use std::time::Duration;
use tokio::process::Command;

const ERROR_JSON: &str = r#"{"error":{"code":"NOT_READY"}}"#;
const SUCCESS_JSON: &str = r#"{"state":"ready"}"#;

#[tokio::test]
async fn cli_preserves_http_status_when_an_error_body_is_not_json() {
    xelab_kernel::init_crypto();
    let root = tempfile::tempdir().unwrap();
    let ca_file = root.path().join("ca.crt");
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(&ca_file, cert.pem()).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/api/platform/v1/node",
        get(|RawQuery(query): RawQuery| async move {
            let (status, content_type, body) = match query.as_deref() {
                Some("fixture=html-error") => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "text/html",
                    "<html>upstream unavailable</html>",
                ),
                Some("fixture=text-error") => (
                    StatusCode::GATEWAY_TIMEOUT,
                    "text/plain",
                    "gateway request timed out",
                ),
                Some("fixture=json-error") => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "application/json",
                    ERROR_JSON,
                ),
                Some("fixture=malformed-error") => {
                    (StatusCode::SERVICE_UNAVAILABLE, "application/json", "{")
                }
                Some("fixture=empty-error") => (StatusCode::SERVICE_UNAVAILABLE, "text/plain", ""),
                Some("fixture=malformed-success") => (StatusCode::OK, "application/json", "{"),
                Some("fixture=json-success") => (StatusCode::OK, "application/json", SUCCESS_JSON),
                _ => panic!("unexpected fixture request"),
            };
            (status, [(header::CONTENT_TYPE, content_type)], body)
        }),
    );
    let server =
        tokio::spawn(axum_server::from_tcp_rustls(listener, tls).serve(app.into_make_service()));

    let http_error = "HTTP_ERROR: Server rejected the operation\n";
    for (fixture, exit, stdout, stderr) in [
        ("html-error", 7, "", http_error),
        ("text-error", 6, "", http_error),
        ("json-error", 7, ERROR_JSON, http_error),
        ("malformed-error", 7, "", http_error),
        ("empty-error", 7, "", http_error),
        (
            "malformed-success",
            2,
            "",
            "INVALID_ARGUMENT: Invalid JSON\n",
        ),
        ("json-success", 0, SUCCESS_JSON, ""),
    ] {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_xelabctl"));
        cli.args(["api", "getNode", "--endpoint", &endpoint, "--ca-file"])
            .arg(&ca_file)
            .arg("--query")
            .arg(format!("fixture={fixture}"))
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(10), cli.output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                output.status.code(),
                String::from_utf8(output.stderr).unwrap().as_str()
            ),
            (Some(exit), stderr),
            "{fixture}",
        );
        if stdout.is_empty() {
            assert!(output.stdout.is_empty(), "{fixture}");
        } else {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                serde_json::from_str::<Value>(stdout).unwrap(),
                "{fixture}",
            );
        }
    }
    server.abort();
}
