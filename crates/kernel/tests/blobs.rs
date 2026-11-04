//! Real PostgreSQL, filesystem, HTTP and CLI Blob acceptance. Every test owns
//! a disposable database and private directory; fault injection stays there.
#[path = "support/http_host.rs"]
mod http_host;
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    sync::{atomic::AtomicBool, Arc},
};
use tower::ServiceExt;
use uuid::Uuid;
use xelab_kernel::{
    api::{router, App},
    auth::{self, Auth},
    config::Config,
    contracts,
    db::Db,
    runtime::Runtime,
};
struct Fixture {
    app: Arc<App>,
    router: Router,
    root: tempfile::TempDir,
    token: String,
    owner: Uuid,
}
impl Fixture {
    async fn new() -> Option<Self> {
        Self::new_with_origin(None).await
    }
    async fn new_with_origin(public_origin: Option<&str>) -> Option<Self> {
        xelab_kernel::init_crypto();
        let Ok(base) = std::env::var("XELAB_TEST_DATABASE_URL") else {
            eprintln!("SKIPPED: Blob integration tests require XELAB_TEST_DATABASE_URL");
            return None;
        };
        let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
        let name = format!("blob_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let db = Db::new(format!("{base} dbname={name}")).unwrap();
        db.migrate().await.unwrap();
        db.migrate().await.unwrap();
        db.check().await.unwrap();
        let root = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in("/tmp")
            .unwrap();
        let t: toml::Value =
            toml::from_str(include_str!("../../../docs/contracts/examples/kernel.toml")).unwrap();
        let mut value = serde_json::to_value(t).unwrap();
        value["storage"]["root_path"] = json!(root.path());
        if let Some(origin) = public_origin {
            value["server"]["public_origin"] = json!(origin);
            value["server"]["listen"] = json!(format!(
                "127.0.0.1:{}",
                url::Url::parse(origin)
                    .unwrap()
                    .port_or_known_default()
                    .unwrap()
            ));
        }
        let config = Config::from_value(value).unwrap();
        let mut c = db.connect().await.unwrap();
        let tx = c.transaction().await.unwrap();
        let owner = auth::bootstrap(
            &tx,
            "Owner",
            "correct horse blob test password",
            &config.value["quota_defaults"],
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let auth = Auth::new(vec![72; 32]).unwrap();
        let token = auth
            .login(
                &c,
                owner,
                "correct horse blob test password".into(),
                "127.0.0.1".parse().unwrap(),
                Uuid::new_v4(),
            )
            .await
            .unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_string();
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db: db.clone(),
            auth,
            leader: Arc::new(db.leader().await.unwrap()),
            ready: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        Some(Self {
            router: router(app.clone()),
            app,
            root,
            token,
            owner,
        })
    }
    #[allow(clippy::too_many_arguments)] // Exercise the complete HTTP request envelope.
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Body,
        length: Option<u64>,
        digest: Option<&str>,
        key: &str,
        etag: Option<i64>,
        token: Option<&str>,
    ) -> (StatusCode, Value, Vec<u8>) {
        let mut builder = Request::builder()
            .header(
                "host",
                http_host::authority(Some(self.app.config.text("server", "public_origin"))),
            )
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!("Bearer {}", token.unwrap_or(&self.token)),
            )
            .header("idempotency-key", key);
        if let Some(size) = length {
            builder = builder
                .header("content-length", size)
                .header("content-type", "application/octet-stream");
        }
        if let Some(digest) = digest {
            builder = builder.header("x-content-sha256", digest);
        }
        if let Some(rev) = etag {
            builder = builder.header("if-match", format!("\"rev-{rev}\""));
        }
        let mut request = builder.body(body).unwrap();
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
        ));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 67108865)
            .await
            .unwrap()
            .to_vec();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value, bytes)
    }
    async fn upload(&self, data: &[u8], key: &str) -> Value {
        let (status, value, _) = self
            .request(
                "POST",
                "/api/platform/v1/blobs?content_type=application%2Foctet-stream",
                Body::from(data.to_vec()),
                Some(data.len() as u64),
                Some(&contracts::digest(data)),
                key,
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{value}");
        contracts::validate("Blob", &value).unwrap();
        value
    }
    fn path(id: &Value) -> String {
        format!("/api/platform/v1/blobs/{}", id["id"].as_str().unwrap())
    }
}
#[tokio::test]
async fn blob_roundtrip_replay_pins_expiry_and_deletion_reconcile() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let data = b"binary\0payload\xff";
    let blob = f.upload(data, "one").await;
    let path = Fixture::path(&blob);
    assert_eq!(f.upload(data, "one").await, blob);
    let wrong = f
        .request(
            "POST",
            "/api/platform/v1/blobs?content_type=application%2Foctet-stream",
            Body::from(vec![0; data.len()]),
            Some(data.len() as u64),
            Some(&contracts::digest(data)),
            "one",
            None,
            None,
        )
        .await;
    assert_eq!(wrong.0, StatusCode::BAD_REQUEST);
    let different = f
        .request(
            "POST",
            "/api/platform/v1/blobs?content_type=application%2Foctet-stream",
            Body::from("other"),
            Some(5),
            Some(&contracts::digest(b"other")),
            "one",
            None,
            None,
        )
        .await;
    assert_eq!(different.0, StatusCode::CONFLICT);
    assert_eq!(
        f.request("GET", &path, Body::empty(), None, None, "get", None, None)
            .await
            .1,
        blob
    );
    let content = f
        .request(
            "GET",
            &format!("{path}/content"),
            Body::empty(),
            None,
            None,
            "download",
            None,
            None,
        )
        .await;
    assert_eq!(content.0, StatusCode::OK);
    assert_eq!(content.2, data);
    let id = contracts::id(&blob, "id").unwrap();
    let c = f.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE blobs SET pin_count=1,expires_at=now()-interval '1 second' WHERE id=$1",
        &[&id],
    )
    .await
    .unwrap();
    xelab_kernel::blobs::collect(&f.app).await.unwrap();
    assert_eq!(
        f.request(
            "GET",
            &path,
            Body::empty(),
            None,
            None,
            "pinned",
            None,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        f.request(
            "DELETE",
            &path,
            Body::empty(),
            None,
            None,
            "pinned-delete",
            Some(1),
            None
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    c.execute(
        "UPDATE blobs SET pin_count=0,expires_at=now()+interval '1 day' WHERE id=$1",
        &[&id],
    )
    .await
    .unwrap();
    assert_eq!(
        f.request(
            "DELETE",
            &path,
            Body::empty(),
            None,
            None,
            "stale",
            Some(2),
            None
        )
        .await
        .0,
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        f.request(
            "DELETE",
            &path,
            Body::empty(),
            None,
            None,
            "delete",
            Some(1),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert!(
        f.root.path().join("blobs").join(id.to_string()).exists(),
        "physical removal follows the committed intent"
    );
    assert_eq!(
        f.request(
            "DELETE",
            &path,
            Body::empty(),
            None,
            None,
            "delete",
            Some(1),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        f.request("GET", &path, Body::empty(), None, None, "gone", None, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    xelab_kernel::blobs::collect(&f.app).await.unwrap();
    xelab_kernel::blobs::collect(&f.app).await.unwrap();
    assert!(!f.root.path().join("blobs").join(id.to_string()).exists());
    let expired = f.upload(b"expiry", "expires").await;
    let expired_id = contracts::id(&expired, "id").unwrap();
    c.execute(
        "UPDATE blobs SET expires_at=now()-interval '1 second' WHERE id=$1",
        &[&expired_id],
    )
    .await
    .unwrap();
    xelab_kernel::blobs::collect(&f.app).await.unwrap();
    assert!(!f
        .root
        .path()
        .join("blobs")
        .join(expired_id.to_string())
        .exists());
    let count: i64 = c
        .query_one("SELECT count(*) FROM blobs", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 0);
}
#[tokio::test]
async fn interrupted_length_digest_and_revocation_fail_before_metadata_publication() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let path = "/api/platform/v1/blobs?content_type=application%2Foctet-stream";
    let bad = f
        .request(
            "POST",
            path,
            Body::from("abc"),
            Some(4),
            Some(&contracts::digest(b"abcd")),
            "short",
            None,
            None,
        )
        .await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let bad = f
        .request(
            "POST",
            path,
            Body::from("abc"),
            Some(2),
            Some(&contracts::digest(b"ab")),
            "long",
            None,
            None,
        )
        .await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let stream = futures_util::stream::iter(vec![
        Ok(b"a".to_vec()),
        Err(std::io::Error::other("injected disconnect")),
    ]);
    let bad = f
        .request(
            "POST",
            path,
            Body::from_stream(stream),
            Some(3),
            Some(&contracts::digest(b"abc")),
            "disconnect",
            None,
            None,
        )
        .await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    let c = f.app.db.connect().await.unwrap();
    let credential: Uuid = c
        .query_one("SELECT id FROM credentials WHERE kind='session'", &[])
        .await
        .unwrap()
        .get(0);
    let stream = futures_util::stream::unfold((0, c), move |(n, c)| async move {
        match n {
            0 => Some((Ok::<_, std::io::Error>(b"a".to_vec()), (1, c))),
            1 => {
                c.execute(
                    "UPDATE credentials SET status='revoked' WHERE id=$1",
                    &[&credential],
                )
                .await
                .unwrap();
                Some((Ok(b"bc".to_vec()), (2, c)))
            }
            _ => None,
        }
    });
    let bad = f
        .request(
            "POST",
            path,
            Body::from_stream(stream),
            Some(3),
            Some(&contracts::digest(b"abc")),
            "revoked",
            None,
            None,
        )
        .await;
    assert_eq!(bad.0, StatusCode::UNAUTHORIZED);
    let c = f.app.db.connect().await.unwrap();
    assert_eq!(c.query_one("SELECT count(*) FROM audit_events WHERE action='blob.create' AND outcome IN ('failed','denied')",&[]).await.unwrap().get::<_,i64>(0),4);
    assert_eq!(
        c.query_one("SELECT count(*) FROM blobs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    assert_eq!(
        fs::read_dir(f.root.path().join("blobs"))
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name() != ".identity")
            .count(),
        0
    );
}
#[tokio::test]
async fn unauthorized_objects_and_corrupt_content_fail_closed() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let blob = f.upload(b"actual data", "create").await;
    let path = Fixture::path(&blob);
    let body = json!({"name":"other scope","inheritance":{"mode":"selected","rules":[{"action":"blob.read","resource":{"kind":"blob","match":"ids","ids":[Uuid::new_v4()]}}]},"conditions":{"expires_at":chrono::Utc::now()+chrono::Duration::days(1)}});
    let mut request = Request::builder()
        .header(
            "host",
            http_host::authority(Some(f.app.config.text("server", "public_origin"))),
        )
        .method("POST")
        .uri(format!("/api/platform/v1/accounts/{}/api-keys", f.owner))
        .header("authorization", format!("Bearer {}", f.token))
        .header("idempotency-key", "key")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
    ));
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let issued: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1048576).await.unwrap()).unwrap();
    let token = issued["secret"].as_str().unwrap();
    assert_eq!(
        f.request(
            "GET",
            &path,
            Body::empty(),
            None,
            None,
            "get",
            None,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        f.request(
            "GET",
            &format!("{path}/content"),
            Body::empty(),
            None,
            None,
            "get",
            None,
            Some(token)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let id = contracts::id(&blob, "id").unwrap();
    fs::write(
        f.root.path().join("blobs").join(id.to_string()),
        b"corruption!",
    )
    .unwrap();
    assert_eq!(
        f.request(
            "GET",
            &format!("{path}/content"),
            Body::empty(),
            None,
            None,
            "corrupt",
            None,
            None
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}
#[tokio::test]
async fn real_https_cli_upload_and_verified_download_preserve_existing_files() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "https://localhost:{}",
        listener.local_addr().unwrap().port()
    );
    let Some(f) = Fixture::new_with_origin(Some(&endpoint)).await else {
        return;
    };
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let ca = f.root.path().join("ca.crt");
    fs::write(&ca, cert.pem()).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let server = tokio::spawn(
        axum_server::from_tcp_rustls(listener, tls).serve(
            f.router
                .clone()
                .into_make_service_with_connect_info::<SocketAddr>(),
        ),
    );
    let token = f.root.path().join("token");
    fs::write(&token, &f.token).unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let input = f.root.path().join("input.bin");
    let data = vec![129u8; 200000];
    fs::write(&input, &data).unwrap();
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .args([
            "api",
            "uploadBlob",
            "--endpoint",
            &endpoint,
            "--ca-file",
            ca.to_str().unwrap(),
            "--token-file",
            token.to_str().unwrap(),
            "--body-file",
            input.to_str().unwrap(),
            "--query",
            "content_type=application/octet-stream",
            "--idempotency-key",
            "cli",
        ])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let blob: Value = serde_json::from_slice(&output.stdout).unwrap();
    let path = format!("blob_id={}", blob["id"].as_str().unwrap());
    let target = f.root.path().join("download.bin");
    let args = [
        "api",
        "downloadBlob",
        "--endpoint",
        &endpoint,
        "--ca-file",
        ca.to_str().unwrap(),
        "--token-file",
        token.to_str().unwrap(),
        "--path",
        &path,
        "--output-file",
        target.to_str().unwrap(),
    ];
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(&target).unwrap(), data);
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_xelabctl"))
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(&target).unwrap(), data);
    server.abort();
}

fn age(file: &impl std::os::fd::AsRawFd) {
    let time = [libc::timespec {
        tv_sec: 1,
        tv_nsec: 0,
    }; 2];
    assert_eq!(
        unsafe { libc::futimens(file.as_raw_fd(), time.as_ptr()) },
        0
    );
}
#[tokio::test]
async fn orphan_gc_keeps_live_and_in_progress_files_then_recovers_after_crash() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let store = xelab_kernel::storage::blob::Store::open(f.root.path()).unwrap();
    let id = Uuid::new_v4();
    let mut upload = store.stage(0).unwrap();
    upload.publish(id).await.unwrap();
    age(upload.file.as_ref().unwrap());
    xelab_kernel::blobs::collect_orphans(&f.app, &mut None)
        .await
        .unwrap();
    assert!(
        f.root.path().join("blobs").join(id.to_string()).exists(),
        "active upload lock fences orphan collection"
    );
    let mut pending = f.app.db.connect().await.unwrap();
    let tx = pending.transaction().await.unwrap();
    tx.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended($1,$2))",
        &[&id.to_string(), &0x58454c424c4f4246_i64],
    )
    .await
    .unwrap();
    drop(upload);
    xelab_kernel::blobs::collect_orphans(&f.app, &mut None)
        .await
        .unwrap();
    assert!(
        f.root.path().join("blobs").join(id.to_string()).exists(),
        "an unresolved database commit remains fenced after its filesystem handle closes"
    );
    tx.rollback().await.unwrap();
    xelab_kernel::blobs::collect_orphans(&f.app, &mut None)
        .await
        .unwrap();
    assert!(
        !f.root.path().join("blobs").join(id.to_string()).exists(),
        "uncommitted published object is collected after its uploader exits"
    );
    let blob = f.upload(b"retained", "retain").await;
    let id = contracts::id(&blob, "id").unwrap();
    let file = store.open_object(id).unwrap();
    age(&file);
    drop(file);
    xelab_kernel::blobs::collect_orphans(&f.app, &mut None)
        .await
        .unwrap();
    assert!(
        f.root.path().join("blobs").join(id.to_string()).exists(),
        "live metadata protects old files"
    );
    let pending = f
        .request(
            "DELETE",
            &Fixture::path(&blob),
            Body::empty(),
            None,
            None,
            "delete",
            Some(1),
            None,
        )
        .await;
    assert_eq!(pending.0, StatusCode::NO_CONTENT);
    store.remove(id).unwrap(); // crash after unlink, before marking the durable intent complete
    xelab_kernel::blobs::collect(&f.app).await.unwrap();
    let c = f.app.db.connect().await.unwrap();
    assert_eq!(
        c.query_one("SELECT count(*) FROM blob_deletions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
}
#[tokio::test]
async fn upload_accepts_zero_and_64_mib_and_rejects_larger_declared_lengths() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let zero = f.upload(&[], "zero").await;
    assert_eq!(zero["size_bytes"], 0);
    let data = vec![71u8; 67108864];
    let maximum = f.upload(&data, "maximum").await;
    assert_eq!(maximum["size_bytes"], 67108864);
    let rejected = f
        .request(
            "POST",
            "/api/platform/v1/blobs?content_type=application%2Foctet-stream",
            Body::empty(),
            Some(67108865),
            Some(&contracts::digest(&[])),
            "over-limit",
            None,
            None,
        )
        .await;
    assert_eq!(rejected.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn registered_root_refuses_missing_or_replaced_identity_without_adopting_it() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    f.upload(b"existing", "existing").await;
    let marker = f.root.path().join("blobs/.identity");
    let original = fs::read(&marker).unwrap();
    fs::remove_file(&marker).unwrap();
    let upload = || {
        f.request(
            "POST",
            "/api/platform/v1/blobs?content_type=application%2Foctet-stream",
            Body::from("new"),
            Some(3),
            Some("sha256:11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437"),
            "new",
            None,
            None,
        )
    };
    assert_eq!(upload().await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!marker.exists());
    assert!(xelab_kernel::blobs::collect(&f.app).await.is_err());
    fs::write(&marker, format!("{}\n", Uuid::new_v4())).unwrap();
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(upload().await.0, StatusCode::SERVICE_UNAVAILABLE);
    fs::write(&marker, original).unwrap();
    assert_eq!(upload().await.0, StatusCode::CREATED);
}
#[tokio::test]
async fn durable_root_registration_intent_recovers_both_sides_of_marker_publication() {
    let Some(f) = Fixture::new().await else {
        return;
    };
    let c = f.app.db.connect().await.unwrap();
    let identity = Uuid::new_v4();
    c.execute(
        "INSERT INTO blob_storage_root(id,state) VALUES($1,'preparing')",
        &[&identity],
    )
    .await
    .unwrap();
    f.upload(b"initial", "initial").await;
    assert_eq!(
        fs::read(f.root.path().join("blobs/.identity")).unwrap(),
        format!("{identity}\n").as_bytes()
    );
    c.execute("UPDATE blob_storage_root SET state='preparing'", &[])
        .await
        .unwrap();
    f.upload(b"after restart", "restart").await;
    assert_eq!(
        c.query_one("SELECT state FROM blob_storage_root", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "ready"
    );
    assert_eq!(
        c.query_one("SELECT count(*) FROM blobs", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );
}

#[tokio::test]
async fn streaming_download_rechecks_revocation_before_delivering_more_bytes() {
    use futures_util::StreamExt;
    let Some(f) = Fixture::new().await else {
        return;
    };
    let blob = f.upload(&vec![91; 200000], "stream").await;
    let mut request = Request::builder()
        .header(
            "host",
            http_host::authority(Some(f.app.config.text("server", "public_origin"))),
        )
        .uri(format!("{}/content", Fixture::path(&blob)))
        .header("authorization", format!("Bearer {}", f.token))
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:30000".parse::<SocketAddr>().unwrap(),
    ));
    let response = f.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap().len(), 65536);
    let c = f.app.db.connect().await.unwrap();
    c.execute(
        "UPDATE credentials SET status='revoked' WHERE owner_id=$1",
        &[&f.owner],
    )
    .await
    .unwrap();
    assert!(body.next().await.unwrap().is_err());
}
