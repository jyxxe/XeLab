use axum::{body::Bytes, http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};
use std::{
    ffi::CString,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::process::Command;

#[tokio::test]
async fn blob_upload_rejects_fifo_and_streams_regular_files() {
    xelab_kernel::init_crypto();
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("no-writer.fifo");
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(
        unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) },
        0,
        "{}",
        std::io::Error::last_os_error(),
    );
    let input = root.path().join("payload.bin");
    let alias = root.path().join("payload-link.bin");
    let data = b"\x00\xffblob";
    std::fs::write(&input, data).unwrap();
    std::os::unix::fs::symlink(&input, &alias).unwrap();

    let ca_file = root.path().join("ca.crt");
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
    let received = Arc::new(AtomicUsize::new(0));
    let requests = received.clone();
    let app = Router::new().route(
        "/api/platform/v1/blobs",
        post(move |headers: HeaderMap, body: Bytes| {
            let requests = requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                Json(json!({
                    "content_type": headers["content-type"].to_str().unwrap(),
                    "length": headers["content-length"].to_str().unwrap(),
                    "digest": headers["x-content-sha256"].to_str().unwrap(),
                    "body": body.as_ref(),
                }))
            }
        }),
    );
    let server =
        tokio::spawn(axum_server::from_tcp_rustls(listener, tls).serve(app.into_make_service()));

    let upload = |file: &Path| {
        let mut cli = Command::new(env!("CARGO_BIN_EXE_xelabctl"));
        cli.args(["api", "uploadBlob", "--endpoint", &endpoint, "--ca-file"])
            .arg(&ca_file)
            .arg("--body-file")
            .arg(file)
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .kill_on_drop(true);
        async move {
            tokio::time::timeout(Duration::from_secs(3), cli.output())
                .await
                .expect("Blob upload did not complete or reject the input file")
                .unwrap()
        }
    };
    let invalid = upload(&fifo).await;
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stdout.is_empty());
    assert_eq!(
        String::from_utf8(invalid.stderr).unwrap(),
        "INVALID_ARGUMENT: Blob upload requires a file of at most 64 MiB\n",
    );
    let stdin = upload(Path::new("-")).await;
    assert_eq!(stdin.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(stdin.stderr).unwrap(),
        "INVALID_ARGUMENT: Binary upload requires a regular file with known length\n",
    );
    assert_eq!(received.load(Ordering::SeqCst), 0);

    for file in [&input, &alias] {
        let output = upload(file).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            response,
            json!({
                "content_type": "application/octet-stream",
                "length": data.len().to_string(),
                "digest": xelab_kernel::contracts::digest(data),
                "body": data.as_slice(),
            })
        );
    }
    assert_eq!(received.load(Ordering::SeqCst), 2);
    server.abort();
}
