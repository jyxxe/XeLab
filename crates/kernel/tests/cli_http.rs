use axum::{
    body::{Body, Bytes},
    http::{header, Response, StatusCode},
    routing::get,
    Router,
};
use futures_util::{stream, StreamExt};
use std::{convert::Infallible, future::pending, process::Output, time::Duration};
use tokio::process::Command;

fn stalled_body(prefix: &'static [u8]) -> Body {
    Body::from_stream(
        stream::iter([Ok::<_, Infallible>(Bytes::from_static(prefix))]).chain(stream::pending()),
    )
}

#[tokio::test]
async fn cli_reports_http_deadlines_and_cleans_incomplete_blob() {
    xelab_kernel::init_crypto();
    let root = tempfile::tempdir().unwrap();
    let ca_file = root.path().join("ca.crt");
    let blob_file = root.path().join("download.bin");

    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(&ca_file, certificate.pem()).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/platform/v1/node", get(pending::<StatusCode>))
        .route(
            "/api/platform/v1/skills",
            get(|| async {
                Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(stalled_body(b"{"))
                    .unwrap()
            }),
        )
        .route(
            "/api/platform/v1/blobs/:id/content",
            get(|| async {
                Response::builder()
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .header(header::CONTENT_LENGTH, 3)
                    .header("x-content-sha256", xelab_kernel::contracts::digest(b"abc"))
                    .body(stalled_body(b"a"))
                    .unwrap()
            }),
        );
    let server =
        tokio::spawn(axum_server::from_tcp_rustls(listener, tls).serve(app.into_make_service()));

    let cli = |operation| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_xelabctl"));
        command
            .args([
                "api",
                operation,
                "--endpoint",
                &endpoint,
                "--timeout-ms",
                "1000",
            ])
            .arg("--ca-file")
            .arg(&ca_file)
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .kill_on_drop(true);
        command
    };
    let run = |mut command: Command| async move {
        tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .unwrap()
            .unwrap()
    };
    let headers = run(cli("getNode")).await;
    let json = run(cli("listSkills")).await;
    let mut blob = cli("downloadBlob");
    blob.args([
        "--path",
        "blob_id=00000000-0000-4000-8000-000000000001",
        "--output-file",
    ])
    .arg(&blob_file);
    let blob = run(blob).await;
    server.abort();

    let describe = |output: Output| {
        assert!(output.stdout.is_empty());
        (
            output.status.code(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    assert_eq!(
        [describe(headers), describe(json), describe(blob)],
        [
            (
                Some(6),
                "DEADLINE_EXCEEDED: HTTP request failed; operation outcome may need inspection\n"
                    .to_string(),
            ),
            (
                Some(6),
                "DEADLINE_EXCEEDED: HTTP response was interrupted\n".into()
            ),
            (
                Some(6),
                "DEADLINE_EXCEEDED: Blob download was interrupted\n".into()
            ),
        ]
    );
    let files = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(files, ["ca.crt"]);
}
