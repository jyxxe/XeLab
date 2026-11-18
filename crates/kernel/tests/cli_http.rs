use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{header, Response, StatusCode, Uri},
    routing::get,
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde_json::json;
use std::{
    convert::Infallible,
    future::pending,
    io,
    path::Path,
    process::Output,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{process::Command, task::JoinHandle};

async fn start_https(app: Router, ca_file: &Path) -> (String, JoinHandle<io::Result<()>>) {
    xelab_kernel::init_crypto();
    let key = rcgen::KeyPair::generate().unwrap();
    let certificate = rcgen::CertificateParams::new(vec!["127.0.0.1".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    std::fs::write(ca_file, certificate.pem()).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let server =
        tokio::spawn(axum_server::from_tcp_rustls(listener, tls).serve(app.into_make_service()));
    (endpoint, server)
}

fn cli(operation: &str, endpoint: &str, ca_file: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xelabctl"));
    command
        .args(["api", operation, "--endpoint", endpoint, "--ca-file"])
        .arg(ca_file)
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .kill_on_drop(true);
    command
}

async fn run_cli(mut command: Command) -> Output {
    tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .unwrap()
        .unwrap()
}

fn stalled_body(prefix: &'static [u8]) -> Body {
    Body::from_stream(
        stream::iter([Ok::<_, Infallible>(Bytes::from_static(prefix))]).chain(stream::pending()),
    )
}

#[tokio::test]
async fn cli_reports_http_deadlines_and_cleans_incomplete_blob() {
    let root = tempfile::tempdir().unwrap();
    let ca_file = root.path().join("ca.crt");
    let blob_file = root.path().join("download.bin");
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
    let (endpoint, server) = start_https(app, &ca_file).await;

    let timed_cli = |operation| {
        let mut command = cli(operation, &endpoint, &ca_file);
        command.args(["--timeout-ms", "1000"]);
        command
    };
    let headers = run_cli(timed_cli("getNode")).await;
    let json = run_cli(timed_cli("listSkills")).await;
    let mut blob = timed_cli("downloadBlob");
    blob.args([
        "--path",
        "blob_id=00000000-0000-4000-8000-000000000001",
        "--output-file",
    ])
    .arg(&blob_file);
    let blob = run_cli(blob).await;
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

#[tokio::test]
async fn cli_rejects_unknown_and_repeated_path_arguments_before_sending() {
    let root = tempfile::tempdir().unwrap();
    let ca_file = root.path().join("ca.crt");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .fallback(
            |State(requests): State<Arc<Mutex<Vec<String>>>>, uri: Uri| async move {
                requests.lock().unwrap().push(uri.path().to_owned());
                Json(json!({"path": uri.path()}))
            },
        )
        .with_state(requests.clone());
    let (endpoint, server) = start_https(app, &ca_file).await;

    let first = "deployment_id=00000000-0000-4000-8000-000000000001";
    let second = "deployment_id=00000000-0000-4000-8000-000000000002";
    let unexpected = "INVALID_ARGUMENT: Unknown or repeated path argument\n";
    let invalid: [(&str, &[&str], &str); 4] = [
        ("getNode", &[first], unexpected),
        ("getDeployment", &[first, second], unexpected),
        ("getDeployment", &[first, "other=unused"], unexpected),
        (
            "getDeployment",
            &[],
            "INVALID_ARGUMENT: Missing path argument\n",
        ),
    ];
    for (operation, paths, diagnostic) in invalid {
        let mut command = cli(operation, &endpoint, &ca_file);
        for path in paths {
            command.args(["--path", path]);
        }
        let output = run_cli(command).await;
        assert_eq!(
            (
                output.status.code(),
                String::from_utf8(output.stderr).unwrap().as_str()
            ),
            (Some(2), diagnostic),
            "{operation} {paths:?}",
        );
        assert!(output.stdout.is_empty());
    }
    assert!(requests.lock().unwrap().is_empty());

    let mut valid = cli("getUiContribution", &endpoint, &ca_file);
    valid.args(["--path", "contribution_id=main", "--path", second]);
    let output = run_cli(valid).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = "/api/platform/v1/ui/contributions/00000000-0000-4000-8000-000000000002/main";
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response, json!({"path": expected}));
    assert_eq!(*requests.lock().unwrap(), [expected]);
    server.abort();
}
