//! Actual api::router + PostgreSQL + TLS + Chromium OAuth acceptance. The core
//! listener serves the production router directly, including its Host, login,
//! OAuth, Origin and Identity checks. The separate UI is a callback fixture;
//! this does not claim independent OCI WebUI delivery.
use super::{Fixture, PASSWORD};
use crate::api;
use axum::{
    body::Body, extract::State, http::StatusCode, response::Response, routing::get, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rcgen::{CertificateParams, KeyPair};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    net::{SocketAddr, TcpListener},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    time::Duration,
};

async fn callback(State(core): State<String>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .header("referrer-policy", "no-referrer")
        .header(
            "content-security-policy",
            format!("default-src 'none'; connect-src {core}; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"),
        )
        .body(Body::from("<!doctype html><html><head><title>Actual OAuth callback fixture</title></head><body><main>OAuth callback received by the independent UI origin.</main></body></html>"))
        .unwrap()
}

#[tokio::test]
#[ignore = "requires actual PostgreSQL, Python Playwright and Chromium; production main router over TLS, callback fixture is not an OCI WebUI"]
async fn actual_main_router_chromium_trusted_login_native_consent_pkce_cors_and_refresh_reuse() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let core_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    core_listener.set_nonblocking(true).unwrap();
    ui_listener.set_nonblocking(true).unwrap();
    let core = format!(
        "https://core.localhost:{}",
        core_listener.local_addr().unwrap().port()
    );
    let ui = format!(
        "https://ui.localhost:{}",
        ui_listener.local_addr().unwrap().port()
    );
    let fixture = Fixture::new(&core, &ui)
        .await
        .expect("explicit browser acceptance requires XELAB_TEST_DATABASE_URL");
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["core.localhost".into(), "ui.localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let spki = STANDARD.encode(Sha256::digest(key.public_key_der()));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let core_handle = axum_server::Handle::new();
    let ui_handle = axum_server::Handle::new();
    let core_router = api::router(fixture.app.clone());
    let core_server = tokio::spawn(
        axum_server::from_tcp_rustls(core_listener, tls.clone())
            .handle(core_handle.clone())
            .serve(core_router.into_make_service_with_connect_info::<SocketAddr>()),
    );
    let ui_server = tokio::spawn(
        axum_server::from_tcp_rustls(ui_listener, tls)
            .handle(ui_handle.clone())
            .serve(
                Router::new()
                    .route("/callback", get(callback))
                    .with_state(core.clone())
                    .into_make_service(),
            ),
    );
    let directory = tempfile::tempdir().unwrap();
    let configuration = directory.path().join("oauth-browser-fixture.json");
    let output_path = directory.path().join("oauth-browser-result.json");
    let configuration_body = json!({
        "core_origin":core,"ui_origin":ui,"redirect_uri":format!("{ui}/callback"),
        "client_id":fixture.client["client_id"],"account_id":fixture.owner,
        "password":PASSWORD,"scope":format!("client:{}",fixture.client["client_id"].as_str().unwrap()),
        "api_read_path":format!("/api/platform/v1/accounts/{}",fixture.owner),
        "expected_account_id":fixture.owner,"connect_ip":"127.0.0.1","tls_spki_sha256":spki,
        "chromium_path":std::env::var("XELAB_OAUTH_BROWSER_CHROMIUM").unwrap_or_else(|_|"/usr/bin/chromium".into()),
    });
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&configuration)
        .unwrap()
        .write_all(&serde_json::to_vec(&configuration_body).unwrap())
        .unwrap();
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/oauth-browser.py");
    let outcome = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new(
            std::env::var("XELAB_OAUTH_BROWSER_PYTHON").unwrap_or_else(|_| "python3".into()),
        )
        .arg(driver)
        .arg("--fixture")
        .arg(&configuration)
        .arg("--output")
        .arg(&output_path)
        .kill_on_drop(true)
        .output(),
    )
    .await;
    core_handle.graceful_shutdown(Some(Duration::from_secs(1)));
    ui_handle.graceful_shutdown(Some(Duration::from_secs(1)));
    let _ = tokio::time::timeout(Duration::from_secs(3), core_server).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), ui_server).await;
    let output = outcome
        .expect("actual OAuth browser subprocess exceeded 180 seconds")
        .expect("actual OAuth browser subprocess could not start");
    // Read only the driver's structured, secret-free report. Child stderr or a
    // Playwright exception can include form data/URLs and must never be printed.
    let report: Value = fs::read(&output_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| json!({"status":"failed","step":"browser_report_missing"}));
    let mut safe_report = json!({"component":"actual_main_router_pg_tls_browser_not_oci_webui",
        "status":report["status"],"step":report["step"],"error_type":report["error_type"],
        "browser_version":report["browser_version"],"playwright_version":report["playwright_version"],
        "checks":report["checks"],"http":report["http"],"elapsed_seconds":report["elapsed_seconds"]});
    let evidence = fixture.app.db.connect().await.unwrap();
    let counts=evidence.query_one("SELECT (SELECT count(*) FROM authorization_requests WHERE client_id=$1 AND status IN('approved','consumed')) AS decided,(SELECT count(*) FROM idempotency_records i JOIN authorization_requests r ON i.resource_id=r.id::text WHERE r.client_id=$1 AND i.path='/api/platform/v1/auth/consents' AND i.response_status=302 AND i.safe_response IS NULL) AS redirects",&[&fixture.client["client_id"].as_str().unwrap()]).await.unwrap();
    safe_report["http"]["server_approved_or_consumed_requests"] =
        json!(counts.get::<_, i64>("decided"));
    safe_report["http"]["server_secret_free_consent_302_records"] =
        json!(counts.get::<_, i64>("redirects"));
    if let Ok(path) = std::env::var("XELAB_OAUTH_BROWSER_RESULT") {
        fs::write(&path, serde_json::to_vec_pretty(&safe_report).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    println!("{safe_report}");
    assert!(
        output.status.success(),
        "actual main router browser acceptance failed: {safe_report}"
    );
    assert_eq!(report["status"], "passed", "{safe_report}");
    for check in [
        "actual_password_login_http_200",
        "exact_registered_callback",
        "callback_state_and_code",
        "pkce_exchange_cors_http_200",
        "actual_account_cors_read",
        "actual_account_query_cors_read",
        "wrong_pkce_rejected",
        "authorization_code_replay_rejected",
        "refresh_credentials_rotated",
        "refresh_family_revoked",
        "reused_family_access_denied",
        "password_never_sent_to_ui",
        "core_cookie_never_sent_to_ui",
    ] {
        assert_eq!(
            report["checks"][check], true,
            "missing browser check: {check}"
        );
    }
}
