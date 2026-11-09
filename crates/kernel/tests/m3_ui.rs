//! Real signed OCI WebUI + contribution + OAuth + Chromium on a disposable host.
//! The hosted test uses the existing synthetic metadata budget; actual plugin
//! processes, mTLS, cgroups, routes, authority and browser requests are real.
//! This does not certify production daemon capacity, RUN-03 or AWS recovery.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::Request,
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use futures_util::FutureExt;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpListener},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tower::ServiceExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};
use uuid::Uuid;
use xelab_kernel::{
    admission,
    api::{self, App},
    auth::{self, Auth},
    config::Config,
    contracts, controller,
    db::Db,
    runtime::Runtime,
    tls,
};

const BASE_DOMAIN: &str = "plugins.m3.localhost";
const NAMESPACE: &str = "xelab-m3-ui";
const CORE_DOMAIN: &str = "core.m3.localhost";

/// Chrome's resolver flags do not affect xelabctl. This disposable-host alias
/// keeps the real CLI's URL/Host/SAN/CA checks intact and removes only its own
/// unique line. Existing explicit mappings to another address are never edited.
struct LoopbackHost {
    path: PathBuf,
    line: Option<String>,
    original: Vec<u8>,
    installed: Vec<u8>,
}
impl LoopbackHost {
    fn locked_file(path: &Path) -> std::io::Result<fs::File> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() > 262144 {
            return Err(std::io::Error::other("Invalid disposable host alias file"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(file)
    }
    async fn install(port: u16) -> Self {
        let resolved = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((CORE_DOMAIN, port)),
        )
        .await;
        if let Ok(Ok(addresses)) = resolved {
            assert!(
                addresses
                    .into_iter()
                    .all(|address| address.ip().is_loopback()),
                "Preexisting core test DNS must not point to a remote host"
            );
        }
        let guard = Self::install_file(Path::new("/etc/hosts"))
            .expect("Disposable host must permit an isolated CLI alias");
        let addresses = tokio::time::timeout(
            Duration::from_secs(3),
            tokio::net::lookup_host((CORE_DOMAIN, port)),
        )
        .await
        .expect("CLI alias resolution deadline")
        .expect("CLI alias must resolve")
        .collect::<Vec<_>>();
        assert!(
            addresses
                .iter()
                .any(|address| address.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
                && addresses.iter().all(|address| address.ip().is_loopback()),
            "CLI alias must resolve only to loopback, including the actual IPv4 listener"
        );
        guard
    }
    fn install_file(path: &Path) -> std::io::Result<Self> {
        let mut file = Self::locked_file(path)?;
        let mut original = Vec::new();
        file.read_to_end(&mut original)?;
        let contents = std::str::from_utf8(&original)
            .map_err(|_| std::io::Error::other("Host aliases must be UTF-8"))?;
        let mut already = false;
        for line in contents.lines() {
            let fields = line
                .split('#')
                .next()
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>();
            if fields.get(1..).is_some_and(|names| {
                names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(CORE_DOMAIN))
            }) {
                if fields[0] != "127.0.0.1" {
                    return Err(std::io::Error::other(
                        "Preexisting core test hosts mapping differs; refusing to replace it",
                    ));
                }
                already = true;
            }
        }
        let (line, installed) = if already {
            (None, original.clone())
        } else {
            let line = format!("127.0.0.1 {CORE_DOMAIN} # xelab-m3-ui-{}", Uuid::new_v4());
            let mut installed = original.clone();
            if !installed.is_empty() && !installed.ends_with(b"\n") {
                installed.push(b'\n');
            }
            installed.extend_from_slice(line.as_bytes());
            installed.push(b'\n');
            // Append only our suffix, preserving all preexisting bytes even
            // during a partial write. Roll back a failed append under the lock.
            file.seek(SeekFrom::End(0))?;
            if let Err(error) = file
                .write_all(&installed[original.len()..])
                .and_then(|()| file.sync_all())
            {
                file.set_len(original.len() as u64)?;
                file.sync_all()?;
                return Err(error);
            }
            (Some(line), installed)
        };
        drop(file); // Releases only this short exclusive file lock.
        Ok(Self {
            path: path.to_owned(),
            line,
            original,
            installed,
        })
    }
    fn remove(&mut self) -> std::io::Result<()> {
        let Some(line) = self.line.as_ref() else {
            return Ok(());
        };
        let mut file = Self::locked_file(&self.path)?;
        let mut current = Vec::new();
        file.read_to_end(&mut current)?;
        let restored = if current == self.installed {
            self.original.clone()
        } else {
            let contents = std::str::from_utf8(&current).map_err(|_| {
                std::io::Error::other("Host aliases changed to unsupported encoding")
            })?;
            contents
                .split_inclusive('\n')
                .filter(|segment| segment.trim_end_matches(['\r', '\n']) != line)
                .collect::<String>()
                .into_bytes()
        };
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&restored)?;
        file.set_len(restored.len() as u64)?;
        file.sync_all()?;
        self.line = None;
        Ok(())
    }
}
impl Drop for LoopbackHost {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

#[test]
fn cli_loopback_alias_preserves_preexisting_and_concurrent_hosts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hosts");
    let original = b"127.0.0.1 localhost\n::1 localhost";
    fs::write(&path, original).unwrap();
    let mut alias = LoopbackHost::install_file(&path).unwrap();
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("127.0.0.1 core.m3.localhost # xelab-m3-ui-"));
    alias.remove().unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
    {
        let _alias = LoopbackHost::install_file(&path).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"127.0.0.2 another-test.localhost # concurrent-owner\n")
            .unwrap();
    }
    let remaining = fs::read_to_string(&path).unwrap();
    assert!(remaining.starts_with(std::str::from_utf8(original).unwrap()));
    assert!(remaining.contains("127.0.0.2 another-test.localhost # concurrent-owner\n"));
    assert!(!remaining.contains("xelab-m3-ui-"));
}

#[test]
fn cli_loopback_alias_rejects_conflicts_without_editing_them() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("hosts");
    let conflicting = b"127.0.0.1 localhost\n192.0.2.5 core.m3.localhost # existing-owner\n";
    fs::write(&path, conflicting).unwrap();
    assert!(LoopbackHost::install_file(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), conflicting);
    let preexisting = b"127.0.0.1 localhost core.m3.localhost # existing-loopback-owner\n";
    fs::write(&path, preexisting).unwrap();
    {
        let _alias = LoopbackHost::install_file(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), preexisting);
    }
    assert_eq!(fs::read(&path).unwrap(), preexisting);
}

fn private_write(path: &Path, bytes: &[u8]) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn public_host(app: &App) -> String {
    let url = url::Url::parse(app.config.text("server", "public_origin")).unwrap();
    format!(
        "{}:{}",
        url.host_str().unwrap(),
        url.port_or_known_default().unwrap()
    )
}

#[allow(clippy::too_many_arguments)]
async fn api(
    router: &Router,
    host: &str,
    name: &str,
    params: &[(&str, String)],
    token: Option<&str>,
    body: Value,
    key: &str,
    revision: Option<i64>,
) -> (u16, Value) {
    let (mut path, method) = contracts::openapi()["paths"]
        .as_object()
        .unwrap()
        .iter()
        .find_map(|(path, methods)| {
            methods
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, operation)| operation["operationId"] == name)
                .map(|(method, _)| (path.clone(), method.to_ascii_uppercase()))
        })
        .expect("Known public operation");
    for (name, value) in params {
        path = path.replace(&format!("{{{name}}}"), value);
    }
    let mut request = Request::builder()
        .method(method.as_str())
        .uri(path)
        .header("host", host)
        .header("idempotency-key", key);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if let Some(revision) = revision {
        request = request.header("if-match", format!("\"rev-{revision}\""));
    }
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        request = request.header("content-type", "application/json");
        serde_json::to_vec(&body).unwrap()
    };
    let mut request = request.body(Body::from(bytes)).unwrap();
    request.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:50000".parse::<SocketAddr>().unwrap(),
    ));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024 + 65536)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("Public JSON response")
    };
    (status, value)
}

fn status(response: (u16, Value), expected: u16, operation: &str) -> Value {
    // A login/activation failure must never print a credential-bearing body.
    assert_eq!(
        response.0,
        expected,
        "public operation {operation}: error_code={}",
        response.1["error"]["code"]
            .as_str()
            .unwrap_or("no-safe-code")
    );
    response.1
}

async fn operation(router: &Router, host: &str, id: &str, token: &str) -> Value {
    for _ in 0..480 {
        let value = status(
            api(
                router,
                host,
                "getOperation",
                &[("operation_id", id.into())],
                Some(token),
                Value::Null,
                "poll",
                None,
            )
            .await,
            200,
            "getOperation",
        );
        match value["state"].as_str() {
            Some("succeeded") => return value["result"].clone(),
            Some("failed" | "canceled") => panic!(
                "Public operation failed: {}",
                value["error"]["code"].as_str().unwrap_or("no-safe-code")
            ),
            _ => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    panic!("Public operation exceeded the integration deadline");
}

async fn read_deployment(router: &Router, host: &str, token: &str, deployment: &str) -> Value {
    status(
        api(
            router,
            host,
            "getDeployment",
            &[("deployment_id", deployment.into())],
            Some(token),
            Value::Null,
            "read-deployment",
            None,
        )
        .await,
        200,
        "getDeployment",
    )
}

async fn enabled(
    router: &Router,
    host: &str,
    token: &str,
    deployment: &str,
    enabled: bool,
    intent: &str,
) {
    for attempt in 0..12 {
        let current = read_deployment(router, host, token, deployment).await;
        let changed = api(
            router,
            host,
            "setDeploymentEnabled",
            &[("deployment_id", deployment.into())],
            Some(token),
            json!({"enabled":enabled}),
            &format!("{intent}-{attempt}"),
            current["revision"].as_i64(),
        )
        .await;
        if changed.0 == 412 {
            continue;
        } // Confirmed rejection only; no unknown retry.
        let changed = status(changed, 202, "setDeploymentEnabled");
        operation(router, host, changed["id"].as_str().unwrap(), token).await;
        return;
    }
    panic!("Deployment observations did not settle for enable/disable");
}

async fn wait_ready(app: &App, deployment: &str) {
    let c = app.db.connect().await.unwrap();
    let id = Uuid::parse_str(deployment).unwrap();
    for _ in 0..480 {
        let ready:bool=c.query_one("SELECT EXISTS(SELECT 1 FROM deployments d JOIN instances i ON i.deployment_id=d.id WHERE d.id=$1 AND d.phase='ready' AND i.phase='ready' AND i.health='alive' AND i.generation=d.observed_generation)",&[&id]).await.unwrap().get(0);
        if ready && app.ready.load(Ordering::SeqCst) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("Actual OCI deployment did not become ready");
}

/// Independently check the kernel files for the actual admitted Ready instance,
/// including pids.max (in addition to Runtime's CPU/memory startup checks).
/// The returned evidence contains finite limit numbers, never host paths.
async fn instance_cgroup_limits(app: &App, deployment: &str, approved: &Value) -> Value {
    let c = app.db.connect().await.unwrap();
    let deployment = Uuid::parse_str(deployment).unwrap();
    let instances=c.query("SELECT i.id,i.limits FROM deployments d JOIN instances i ON i.deployment_id=d.id WHERE d.id=$1 AND d.spec->'enabled'='true'::jsonb AND d.approval<>'{}'::jsonb AND d.phase='ready' AND i.phase='ready' AND i.health='alive' AND i.generation=d.observed_generation",&[&deployment]).await.unwrap();
    assert_eq!(
        instances.len(),
        1,
        "One approved Ready instance is required"
    );
    assert_eq!(
        instances[0].get::<_, Value>("limits"),
        *approved,
        "The actual instance must retain the approved deployment limits"
    );
    let instance: Uuid = instances[0].get("id");
    let cgroup = Path::new("/sys/fs/cgroup/xelab/plugins").join(instance.to_string());
    let cpu = fs::read_to_string(cgroup.join("cpu.max")).expect("Read actual cgroup CPU limit");
    let cpu: Vec<u64> = cpu
        .split_whitespace()
        .map(|value| {
            value
                .parse()
                .expect("Actual cgroup CPU limit must be finite")
        })
        .collect();
    assert_eq!(cpu.len(), 2, "Actual cgroup CPU quota and period required");
    let quota = approved["cpu_millis"]
        .as_u64()
        .unwrap()
        .checked_mul(100)
        .unwrap();
    assert_eq!(cpu, [quota, 100000], "Actual approved cgroup CPU limit");
    let memory: u64 = fs::read_to_string(cgroup.join("memory.max"))
        .expect("Read actual cgroup memory limit")
        .trim()
        .parse()
        .expect("Actual cgroup memory limit must be finite");
    assert_eq!(
        memory,
        approved["memory_bytes"].as_u64().unwrap(),
        "Actual approved cgroup memory limit"
    );
    let pids: u64 = fs::read_to_string(cgroup.join("pids.max"))
        .expect("Read actual cgroup process limit")
        .trim()
        .parse()
        .expect("Actual cgroup process limit must be finite");
    assert_eq!(
        pids,
        approved["pids"].as_u64().unwrap(),
        "Actual approved cgroup process limit"
    );
    json!({"cpu_quota_us":cpu[0],"cpu_period_us":cpu[1],"memory_bytes":memory,"pids":pids})
}

// Diagnostic identities stay in memory. Evidence contains fixed roles, enums
// and numbers only, and is captured before any shutdown or runtime cleanup.
struct DiagnosticTarget {
    role: &'static str,
    deployment: Uuid,
    instance: Option<Uuid>,
}

// This subscriber intentionally has no fmt layer. Only fixed controller fields
// survive; instance/deployment identities are private role-mapping inputs.
#[derive(Clone)]
struct ControllerEvents {
    events: Arc<Mutex<VecDeque<ControllerEvent>>>,
    dropped: Arc<AtomicU64>,
    installed: Arc<AtomicBool>,
    started: Instant,
}
struct ControllerEvent {
    fields: ControllerFields,
    relative_ms: u64,
}
#[derive(Default)]
struct ControllerFields {
    deployment: Option<Uuid>,
    instance: Option<Uuid>,
    safe: serde_json::Map<String, Value>,
}
impl tracing::field::Visit for ControllerFields {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        let allowed: &[&str] = match field.name() {
            "code" => &[
                "UNCERTAIN_WORK_FENCE",
                "HEALTH_REJECTED",
                "HEALTH_THRESHOLD_STOP",
                "STATIC_UNSETTLED",
            ],
            "stage" => &[
                "recovery",
                "channel",
                "describe",
                "initialize",
                "get_health",
                "http_probe",
                "transport_setup",
                "fetch",
                "fetch_timeout",
                "fetch_error",
                "authority_monitor",
                "lease_complete",
            ],
            _ => return,
        };
        self.safe
            .insert(field.name().into(), json!(diagnostic_enum(value, allowed)));
    }
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if [
            "call_unknown",
            "call_expired",
            "static_unknown",
            "static_expired",
            "failures",
            "threshold",
            "elapsed_ms",
            "phase_ms",
        ]
        .contains(&field.name())
        {
            self.safe.insert(field.name().into(), json!(value));
        }
    }
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        if let Ok(value) = value.try_into() {
            self.record_u64(field, value);
        }
    }
    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        if field.name() == "was_ready" {
            self.safe.insert("was_ready".into(), json!(value));
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let slot = match field.name() {
            "deployment_id" => &mut self.deployment,
            "instance_id" => &mut self.instance,
            _ => return,
        };
        *slot = Uuid::parse_str(&format!("{value:?}")).ok();
    }
}
impl<S: tracing::Subscriber> Layer<S> for ControllerEvents {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if !matches!(
            event.metadata().target(),
            "xelab.controller_transition" | "xelab.static_transition"
        ) {
            return;
        }
        let mut fields = ControllerFields::default();
        event.record(&mut fields);
        let Ok(mut events) = self.events.lock() else {
            self.dropped.fetch_add(1, Ordering::SeqCst);
            return;
        };
        if events.len() >= 32 {
            events.pop_front();
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
        events.push_back(ControllerEvent {
            fields,
            relative_ms: self.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        });
    }
}
impl ControllerEvents {
    fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(VecDeque::new())),
            dropped: Arc::new(AtomicU64::new(0)),
            installed: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        }
    }
    fn install() -> Self {
        let collector = Self::new();
        let installed = tracing_subscriber::registry()
            .with(collector.clone())
            .try_init()
            .is_ok();
        collector.installed.store(installed, Ordering::SeqCst);
        collector
    }
    fn snapshot(&self, targets: &[DiagnosticTarget]) -> Value {
        let installed = self.installed.load(Ordering::SeqCst);
        let Ok(events) = self.events.lock() else {
            return json!({"status":"read_failed","init_failures":u64::from(!installed)});
        };
        let safe: Vec<_> = events
            .iter()
            .map(|event| {
                let role = targets
                    .iter()
                    .find(|target| event.fields.deployment == Some(target.deployment))
                    .or_else(|| {
                        targets.iter().find(|target| {
                            target.instance.is_some() && target.instance == event.fields.instance
                        })
                    })
                    .map(|target| target.role)
                    .unwrap_or("unknown");
                let mut safe = event.fields.safe.clone();
                safe.entry("code").or_insert(json!("missing"));
                safe.entry("stage").or_insert(json!("missing"));
                safe.insert("role".into(), json!(role));
                safe.insert("relative_ms".into(), json!(event.relative_ms));
                Value::Object(safe)
            })
            .collect();
        json!({"status":if installed {"installed"} else {"init_failed"},"init_failures":u64::from(!installed),"dropped":self.dropped.load(Ordering::SeqCst),"events":safe})
    }
}

#[test]
fn controller_diagnostics_are_bounded_and_role_mapped_without_raw_fields() {
    let collector = ControllerEvents::new();
    let deployment = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let subscriber = tracing_subscriber::registry().with(collector.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "ignored", code="HEALTH_REJECTED", secret="never-print");
        tracing::info!(target: "xelab.controller_transition", code="HEALTH_REJECTED", stage="channel", deployment_id=%deployment, instance_id=%instance, failures=1u64, threshold=3u64, was_ready=true, secret="never-print");
        for _ in 0..32 {
            tracing::info!(target: "xelab.controller_transition", code="untrusted-value", stage="untrusted-value");
        }
        tracing::info!(target: "xelab.controller_transition", code="UNCERTAIN_WORK_FENCE", stage="recovery", deployment_id=%deployment, instance_id=%instance, call_unknown=0i64, static_unknown=1i64);
        tracing::info!(target: "xelab.static_transition", code="STATIC_UNSETTLED", stage="authority_monitor", deployment_id=%deployment, elapsed_ms=10001u64, phase_ms=3000u64, secret="never-print");
        tracing::info!(target: "xelab.static_transition", code="STATIC_UNSETTLED", stage="fetch_timeout", deployment_id=%deployment, secret="never-print");
        tracing::info!(target: "xelab.static_transition", code="STATIC_UNSETTLED", stage="fetch_error", deployment_id=%deployment, secret="never-print");
    });
    let safe = collector.snapshot(&[DiagnosticTarget {
        role: "ui_reference",
        deployment,
        instance: None,
    }]);
    assert_eq!(safe["status"], "init_failed");
    assert_eq!(safe["init_failures"], 1);
    assert_eq!(safe["events"].as_array().unwrap().len(), 32);
    assert_eq!(safe["dropped"], 5);
    assert_eq!(safe["events"][0]["role"], "unknown");
    assert_eq!(safe["events"][0]["code"], "other");
    assert_eq!(safe["events"][31]["role"], "ui_reference");
    assert_eq!(safe["events"][28]["code"], "UNCERTAIN_WORK_FENCE");
    assert_eq!(safe["events"][28]["static_unknown"], 1);
    assert_eq!(safe["events"][29]["code"], "STATIC_UNSETTLED");
    assert_eq!(safe["events"][29]["stage"], "authority_monitor");
    assert_eq!(safe["events"][29]["elapsed_ms"], 10001);
    assert_eq!(safe["events"][29]["phase_ms"], 3000);
    assert_eq!(safe["events"][30]["stage"], "fetch_timeout");
    assert_eq!(safe["events"][31]["stage"], "fetch_error");
    let encoded = serde_json::to_string(&safe).unwrap();
    for forbidden in [
        "never-print".to_owned(),
        "untrusted-value".to_owned(),
        deployment.to_string(),
        instance.to_string(),
    ] {
        assert!(!encoded.contains(&forbidden));
    }
}

fn diagnostic_enum(value: &str, allowed: &[&'static str]) -> &'static str {
    allowed
        .iter()
        .copied()
        .find(|item| *item == value)
        .unwrap_or("other")
}

fn diagnostic_effect(value: &str) -> &'static str {
    diagnostic_enum(
        value,
        &[
            "none",
            "idempotent",
            "queryable",
            "non_repeatable",
            "not_recorded",
        ],
    )
}

fn diagnostic_counters(bytes: &[u8], keys: &[&str]) -> Value {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return json!({"status":"parse_error"});
    };
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2 {
            return json!({"status":"parse_error"});
        }
        let Ok(value) = fields[1].parse::<u64>() else {
            return json!({"status":"parse_error"});
        };
        if keys.contains(&fields[0]) && values.insert(fields[0], value).is_some() {
            return json!({"status":"parse_error"});
        }
    }
    let mut result = json!({"status":"ok","counters":{}});
    for key in keys {
        result["counters"][*key] = match values.get(key) {
            Some(value) => json!({"status":"ok","value":value}),
            None => json!({"status":"missing_key"}),
        };
    }
    result
}

fn diagnostic_file(path: &Path, keys: &[&str]) -> Value {
    let read = (|| -> std::io::Result<Vec<u8>> {
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let mut bytes = Vec::new();
        file.take(4097).read_to_end(&mut bytes)?;
        Ok(bytes)
    })();
    match read {
        Ok(bytes) if bytes.len() > 4096 => json!({"status":"oversized"}),
        Ok(bytes) if !keys.is_empty() => diagnostic_counters(&bytes, keys),
        Ok(bytes) => match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
        {
            Some(value) => json!({"status":"ok","value":value}),
            None => json!({"status":"parse_error"}),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"status":"missing"}),
        Err(_) => json!({"status":"read_error"}),
    }
}

fn diagnostic_cgroup(path: &Path) -> Value {
    json!({
        "memory_current":diagnostic_file(&path.join("memory.current"), &[]),
        "pids_current":diagnostic_file(&path.join("pids.current"), &[]),
        "memory_events":diagnostic_file(&path.join("memory.events"), &["low","high","max","oom","oom_kill","oom_group_kill"]),
        "pids_events":diagnostic_file(&path.join("pids.events"), &["max"])
    })
}

async fn diagnostic_cgroup_bounded(path: PathBuf) -> Value {
    match tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || diagnostic_cgroup(&path)),
    )
    .await
    {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => json!({"status":"read_error"}),
        Err(_) => json!({"status":"timeout"}),
    }
}

fn diagnostic_tasks(bytes: &[u8]) -> Result<BTreeMap<Uuid, &'static str>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header: Vec<_> = lines.next().ok_or(())?.split_whitespace().collect();
    if header != ["TASK", "PID", "STATUS"] {
        return Err(());
    }
    let mut tasks = BTreeMap::new();
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 3 || fields[1].parse::<u32>().is_err() {
            return Err(());
        }
        let id = fields[0].parse::<Uuid>().map_err(|_| ())?;
        let status = diagnostic_enum(
            fields[2],
            &[
                "RUNNING", "CREATED", "STOPPED", "PAUSED", "PAUSING", "UNKNOWN",
            ],
        );
        if tasks.insert(id, status).is_some() {
            return Err(());
        }
    }
    Ok(tasks)
}

async fn diagnostic_database(
    app: &App,
    targets: &mut [DiagnosticTarget],
    evidence: &mut Value,
) -> Result<(), ()> {
    let c = app.db.connect().await.map_err(|_| ())?;
    // HTTP Calls do not persist a signed side_effect classification. Report
    // that absence separately from Call state=unknown; never invent a claim.
    let calls=c.query("SELECT c.state,CASE WHEN c.http_binding IS NOT NULL THEN 'not_recorded' ELSE COALESCE(p.spec->>'side_effect','not_recorded') END AS effect,count(*) AS count FROM calls c LEFT JOIN capability_contracts p ON p.name=c.contract->>'name' AND p.version=c.contract->>'version' AND p.schema_digest=c.contract->>'schema_digest' GROUP BY c.state,effect",&[]).await.map_err(|_| ())?;
    let mut counts: BTreeMap<(&str, &str), i64> = BTreeMap::new();
    for row in calls {
        let state = diagnostic_enum(
            &row.get::<_, String>("state"),
            &[
                "accepted",
                "running",
                "succeeded",
                "failed",
                "canceled",
                "unknown",
            ],
        );
        let effect = diagnostic_effect(&row.get::<_, String>("effect"));
        *counts.entry((state, effect)).or_default() += row.get::<_, i64>("count");
    }
    evidence["calls"] = json!(counts
        .into_iter()
        .map(|((state, effect), count)| json!({"state":state,"effect":effect,"count":count}))
        .collect::<Vec<_>>());
    for target in targets {
        let deployment = c
            .query_opt(
                "SELECT phase,observed_generation FROM deployments WHERE id=$1",
                &[&target.deployment],
            )
            .await
            .map_err(|_| ())?;
        let Some(row) = deployment else {
            evidence["deployments"][target.role] = json!({"status":"missing"});
            continue;
        };
        let phase = diagnostic_enum(
            &row.get::<_, String>("phase"),
            &[
                "disabled",
                "waiting_resources",
                "reconciling",
                "ready",
                "degraded",
                "failed",
                "draining",
            ],
        );
        let mut entry = json!({"status":"ok","phase":phase,"observed_generation":row.get::<_, i64>("observed_generation"),"instance_status":"missing"});
        // Do not restrict this query to Ready: failed/draining instances are
        // exactly the evidence needed. Preserve only the latest trusted ID.
        let latest=c.query_opt("SELECT id,generation,phase,health,restart_count,(SELECT count(*) FROM instances WHERE deployment_id=$1) AS total FROM instances WHERE deployment_id=$1 ORDER BY generation DESC,ordinal LIMIT 1",&[&target.deployment]).await.map_err(|_| ())?;
        if let Some(row) = latest {
            target.instance = Some(row.get("id"));
            entry["instance_status"] = json!("ok");
            entry["instance_count"] = json!(row.get::<_, i64>("total"));
            entry["generation"] = json!(row.get::<_, i64>("generation"));
            entry["restart_count"] = json!(row.get::<_, i32>("restart_count"));
            entry["instance_phase"] = json!(diagnostic_enum(
                &row.get::<_, String>("phase"),
                &["pending", "starting", "ready", "degraded", "draining", "stopped", "failed"]
            ));
            entry["health"] = json!(diagnostic_enum(
                &row.get::<_, String>("health"),
                &["alive", "unhealthy", "unknown"]
            ));
        }
        evidence["deployments"][target.role] = entry;
    }
    Ok(())
}

async fn diagnostic_snapshot(
    app: &App,
    targets: &mut [DiagnosticTarget],
    controller: &ControllerEvents,
) -> Value {
    let mut evidence = json!({"status":"collecting","database_status":"not_observed","tasks_status":"not_observed","exit_status":"unsupported","calls":null,"deployments":{},"runtime":{},"kernel_ready":app.ready.load(Ordering::SeqCst),"kernel_shutdown":app.shutdown.load(Ordering::SeqCst),"consistency":"best_effort_non_atomic","counter_comparison":"absolute_only_lifetime_not_proven"});
    let collected=std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(8),async {
        evidence["database_status"]=json!(match tokio::time::timeout(Duration::from_secs(2),diagnostic_database(app,targets,&mut evidence)).await {
            Ok(Ok(()))=>"ok",Ok(Err(()))=>"read_error",Err(_)=>"timeout"
        });
        let tasks=tokio::time::timeout(Duration::from_secs(2),app.runtime.ctr(&["tasks".into(),"list".into()])).await;
        let tasks=match tasks {
            Ok(Ok(bytes))=>match diagnostic_tasks(&bytes) {Ok(tasks)=>{evidence["tasks_status"]=json!("ok");Some(tasks)},Err(())=>{evidence["tasks_status"]=json!("parse_error");None}},
            Ok(Err(_))=>{evidence["tasks_status"]=json!("read_error");None},
            Err(_)=>{evidence["tasks_status"]=json!("timeout");None}
        };
        if let Some(tasks)=tasks.as_ref() {
            let mut counts=BTreeMap::new();
            for status in tasks.values() {*counts.entry(*status).or_insert(0u64)+=1;}
            evidence["namespace_task_status_counts"]=json!(counts);
        }
        for target in targets.iter().take(2) {
            evidence["runtime"][target.role]=match target.instance {
                Some(id)=>json!({"identity_source":if evidence["database_status"]=="ok" && evidence["deployments"][target.role]["instance_status"]=="ok" {"current_database"} else {"last_observed_database"},"task_status":tasks.as_ref().map(|tasks|tasks.get(&id).copied().unwrap_or("MISSING")).unwrap_or("not_observed"),"cgroup":diagnostic_cgroup_bounded(Path::new("/sys/fs/cgroup/xelab/plugins").join(id.to_string())).await}),
                None=>json!({"identity_source":"not_observed","task_status":"not_observed","cgroup_status":"not_observed"})
            };
        }
        evidence["plugin_pool"]=diagnostic_cgroup_bounded(PathBuf::from("/sys/fs/cgroup/xelab/plugins")).await;
    })).catch_unwind().await;
    evidence["status"] = json!(match collected {
        Ok(Ok(())) => "complete",
        Ok(Err(_)) => "timeout",
        Err(_) => "collector_failed",
    });
    evidence["controller"] = controller.snapshot(targets);
    evidence
}

#[test]
fn diagnostic_parsers_reject_ambiguous_or_missing_evidence() {
    assert_eq!(diagnostic_effect("not_recorded"), "not_recorded");
    assert_eq!(diagnostic_effect("untrusted-effect"), "other");
    assert_eq!(
        diagnostic_counters(b"oom 0\noom_kill 2\n", &["oom", "oom_kill"])["counters"]["oom_kill"]
            ["value"],
        2
    );
    assert_eq!(
        diagnostic_counters(b"oom 0\n", &["oom_kill"])["counters"]["oom_kill"]["status"],
        "missing_key"
    );
    for bytes in [
        b"oom 1\noom 2\n".as_slice(),
        b"oom -1\n",
        b"oom 1 extra\n",
        b"\xff",
    ] {
        assert_eq!(
            diagnostic_counters(bytes, &["oom"])["status"],
            "parse_error"
        );
    }
    assert!(diagnostic_tasks(b"TASK PID STATUS\n").unwrap().is_empty());
    assert!(diagnostic_tasks(b"TASK PID STATUS\ninvalid 1 RUNNING\n").is_err());
    let row = "00000000-0000-4000-8000-000000000001 1 RUNNING\n";
    assert!(diagnostic_tasks(format!("TASK PID STATUS\n{row}{row}").as_bytes()).is_err());
    let tasks = diagnostic_tasks(
        b"TASK PID STATUS\n00000000-0000-4000-8000-000000000001 1 untrusted-status\n",
    )
    .unwrap();
    assert_eq!(tasks.values().copied().collect::<Vec<_>>(), ["other"]);
    let root = tempfile::tempdir().unwrap();
    assert_eq!(
        diagnostic_file(&root.path().join("missing"), &[])["status"],
        "missing"
    );
    let file = root.path().join("oversized");
    fs::write(&file, vec![b'1'; 4097]).unwrap();
    assert_eq!(diagnostic_file(&file, &[])["status"], "oversized");
}

async fn installed(
    router: &Router,
    host: &str,
    token: &str,
    kind: &str,
    image_variable: &str,
) -> (Value, Value) {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../examples/{kind}/generated-m3-ui"));
    let manifest: Value =
        serde_json::from_slice(&fs::read(directory.join("xelab/plugin.json")).unwrap()).unwrap();
    let install: Value =
        serde_json::from_slice(&fs::read(directory.join("install.json")).unwrap()).unwrap();
    assert_eq!(
        install["image"].as_str().unwrap(),
        std::env::var(image_variable).unwrap()
    );
    let value = status(
        api(
            router,
            host,
            "installPlugin",
            &[],
            Some(token),
            install,
            &format!("install-{kind}"),
            None,
        )
        .await,
        202,
        "installPlugin",
    );
    let result = operation(router, host, value["id"].as_str().unwrap(), token).await;
    (manifest, result)
}

async fn create_deployment(
    router: &Router,
    host: &str,
    token: &str,
    installation: &Value,
    name: &str,
    limits: Value,
    config: Value,
) -> String {
    let body = json!({"installation_id":installation["installation_id"],"display_name":name,"enabled":false,"replicas":1,"limits":limits,"config":config,"volume_ids":[]});
    let created = status(
        api(
            router,
            host,
            "createDeployment",
            &[],
            Some(token),
            body,
            &format!("create-{name}"),
            None,
        )
        .await,
        202,
        "createDeployment",
    );
    operation(router, host, created["id"].as_str().unwrap(), token).await["id"]
        .as_str()
        .unwrap()
        .into()
}

async fn approve(router: &Router, host: &str, token: &str, deployment: &str, key: &str) {
    for attempt in 0..12 {
        let current = read_deployment(router, host, token, deployment).await;
        let value = api(
            router,
            host,
            "approvePlugin",
            &[("deployment_id", deployment.into())],
            Some(token),
            json!({"rules":[],"egress":[]}),
            &format!("{key}-{attempt}"),
            current["revision"].as_i64(),
        )
        .await;
        if value.0 == 412 {
            continue;
        }
        status(value, 200, "approvePlugin");
        return;
    }
    panic!("Approval observation did not settle");
}

fn listener(address: SocketAddr) -> TcpListener {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.set_reuseaddr(true).unwrap();
    socket.bind(address).unwrap();
    socket.listen(128).unwrap().into_std().unwrap()
}

struct Running {
    app: Arc<App>,
    handle: axum_server::Handle,
    api: tokio::task::JoinHandle<std::io::Result<()>>,
    controller: tokio::task::JoinHandle<()>,
    host: tokio::task::JoinHandle<xelab_kernel::error::Result<()>>,
}
impl Running {
    async fn start(
        config: Config,
        db: Db,
        hmac: Vec<u8>,
        public_tls: axum_server::tls_rustls::RustlsConfig,
        socket: TcpListener,
    ) -> Self {
        db.check().await.unwrap();
        let leader = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match db.leader().await {
                    Ok(leader) => break leader,
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await
        .expect("The previous kernel component must release its leader lease");
        let app = Arc::new(App {
            runtime: Runtime::new(config.clone()),
            config,
            db,
            auth: Auth::new(hmac).unwrap(),
            leader: Arc::new(leader),
            ready: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            blob_uploads: tokio::sync::Semaphore::new(4),
        });
        tls::issue(
            &app.config,
            None,
            &app.config.root().join("kernel-identity"),
        )
        .unwrap();
        let controller = tokio::spawn(controller::run(app.clone()));
        let host = tokio::spawn(xelab_kernel::host::serve(app.clone()));
        let handle = axum_server::Handle::new();
        let api = tokio::spawn(
            axum_server::from_tcp_rustls(socket, public_tls)
                .handle(handle.clone())
                .serve(
                    api::router(app.clone()).into_make_service_with_connect_info::<SocketAddr>(),
                ),
        );
        Self {
            app,
            handle,
            api,
            controller,
            host,
        }
    }
    async fn shutdown(mut self) {
        self.app.ready.store(false, Ordering::SeqCst);
        self.app.shutdown.store(true, Ordering::SeqCst);
        self.controller.abort();
        let _ = self.controller.await;
        self.host.abort();
        let _ = self.host.await;
        self.handle.graceful_shutdown(Some(Duration::from_secs(1)));
        if tokio::time::timeout(Duration::from_secs(3), &mut self.api)
            .await
            .is_err()
        {
            self.api.abort();
            let _ = self.api.await;
        }
    }
}

async fn business_usage(c: &tokio_postgres::Client, subject: Uuid) -> (i64, i64, i64) {
    let row=c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) AS calls,daily_calls_used,concurrent_calls_used FROM quotas WHERE account_id=$1",&[&subject]).await.unwrap();
    (
        row.get("calls"),
        row.get("daily_calls_used"),
        row.get("concurrent_calls_used"),
    )
}

async fn set_inheritance(
    router: &Router,
    host: &str,
    token: &str,
    child: &str,
    rules: &Value,
    key: &str,
) {
    let account = status(
        api(
            router,
            host,
            "getAccount",
            &[("account_id", child.into())],
            Some(token),
            Value::Null,
            "read-child",
            None,
        )
        .await,
        200,
        "getAccount",
    );
    status(
        api(
            router,
            host,
            "updateInheritance",
            &[("account_id", child.into())],
            Some(token),
            json!({"mode":"selected","rules":rules}),
            key,
            account["revision"].as_i64(),
        )
        .await,
        200,
        "updateInheritance",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable root/containerd/cgroups/nftables/PostgreSQL/Chromium host and tools/prepare-m3-ui-images.sh"]
async fn signed_oci_webui_oauth_iframe_skill_revocation_and_kernel_restart() {
    assert_eq!(std::env::var("XELAB_M3_UI_TEST").as_deref(), Ok("1"));
    assert_eq!(unsafe { libc::geteuid() }, 0);
    xelab_kernel::init_crypto();
    let controller_events = ControllerEvents::install();
    let directory = tempfile::Builder::new()
        .prefix("xelab-m3-ui-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/mnt")
        .unwrap();
    let root = directory.path();
    let public_socket = listener("127.0.0.1:0".parse().unwrap());
    let address = public_socket.local_addr().unwrap();
    let mut loopback_host = LoopbackHost::install(address.port()).await;
    let core = format!("https://{CORE_DOMAIN}:{}", address.port());
    let public_key = KeyPair::generate().unwrap();
    let public_ca_key = KeyPair::generate().unwrap();
    let mut public_ca_params =
        CertificateParams::new(vec!["M3 disposable public TLS CA".into()]).unwrap();
    public_ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let public_ca = public_ca_params.self_signed(&public_ca_key).unwrap();
    let certificate =
        CertificateParams::new(vec!["core.m3.localhost".into(), format!("*.{BASE_DOMAIN}")])
            .unwrap()
            .signed_by(&public_key, &public_ca, &public_ca_key)
            .unwrap();
    private_write(&root.join("public-ca.crt"), public_ca.pem().as_bytes());
    let spki = STANDARD.encode(Sha256::digest(public_key.public_key_der()));
    let public_tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        format!("{}{}", certificate.pem(), public_ca.pem()).into_bytes(),
        public_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(vec!["M3 disposable workload CA".into()]).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    private_write(
        &root.join("ca.crt"),
        ca.self_signed(&ca_key).unwrap().pem().as_bytes(),
    );
    private_write(&root.join("ca.key"), ca_key.serialize_pem().as_bytes());
    let hmac = rand::random::<[u8; 32]>().to_vec();
    private_write(&root.join("master.key"), &rand::random::<[u8; 32]>());
    let mut value = serde_json::to_value(
        toml::from_str::<toml::Value>(include_str!("../../../docs/contracts/examples/kernel.toml"))
            .unwrap(),
    )
    .unwrap();
    value["storage"]["root_path"] = json!(root.join("state"));
    value["security"]["master_key_file"] = json!(root.join("master.key"));
    value["security"]["workload_ca_cert_file"] = json!(root.join("ca.crt"));
    value["security"]["workload_ca_key_file"] = json!(root.join("ca.key"));
    let namespace = std::env::var("XELAB_M3_UI_NAMESPACE").unwrap_or_else(|_| NAMESPACE.into());
    assert!(
        namespace == NAMESPACE || namespace.starts_with("xelab-m3-ui-"),
        "Only the isolated test namespace is permitted"
    );
    value["runtime"]["namespace"] = json!(namespace);
    // The host-wide managed xh-rpc interface is immutable across sequential
    // OCI test processes. Reuse its approved CIDR; isolation is provided by
    // this test's separate namespace/root/DB and real per-instance netns.
    value["runtime"]["network_cidr"] = json!("172.29.240.0/24");
    value["server"]["host_rpc_listen"] = json!("172.29.240.1:7443");
    value["server"]["listen"] = json!(address.to_string());
    value["server"]["public_origin"] = json!(core);
    value["ui_gateway"] =
        json!({"base_domain":BASE_DOMAIN,"public_port":address.port(),"shell_origins":[]});
    let config = Config::from_value(value).unwrap();
    assert!(
        admission::pool(&config.value["resources"], 4000, 17179869184).is_err(),
        "production kernel reservation stays fail closed"
    );
    fs::create_dir_all("/sys/fs/cgroup/xelab/plugins").unwrap();
    for path in [
        "/sys/fs/cgroup/cgroup.subtree_control",
        "/sys/fs/cgroup/xelab/cgroup.subtree_control",
        "/sys/fs/cgroup/xelab/plugins/cgroup.subtree_control",
    ] {
        fs::write(path, "+cpu +memory +pids").unwrap();
    }
    fs::write("/sys/fs/cgroup/xelab/plugins/memory.max", "1073741824").unwrap();
    fs::write("/sys/fs/cgroup/xelab/plugins/cpu.max", "200000 100000").unwrap();
    fs::create_dir_all(config.root()).unwrap();
    fs::set_permissions(config.root(), fs::Permissions::from_mode(0o700)).unwrap();
    Runtime::new(config.clone())
        .prepare_host_service_network()
        .await
        .unwrap();
    let base = std::env::var("XELAB_TEST_DATABASE_URL").unwrap();
    let admin = Db::new(base.clone()).unwrap().connect().await.unwrap();
    let name = format!("xelab_m3_ui_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();
    let db = Db::new(format!("{base} dbname={name}")).unwrap();
    db.migrate().await.unwrap();
    let password = format!("M3-test-{}", Uuid::new_v4());
    let child_password = format!("M3-child-{}", Uuid::new_v4());
    let mut c = db.connect().await.unwrap();
    let tx = c.transaction().await.unwrap();
    let owner = auth::bootstrap(
        &tx,
        "Actual M3 owner",
        &password,
        &config.value["quota_defaults"],
    )
    .await
    .unwrap();
    // Existing hosted OCI component technique: metadata only. No daemon
    // preflight, kernel resource setting or real cgroup limit is weakened.
    admission::initialize(&tx, &config.value["resources"], 8000, 34359738368)
        .await
        .unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let pem = signing
        .verifying_key()
        .to_public_key_pem(Default::default())
        .unwrap();
    let publisher: Uuid = "00000000-0000-4000-8000-000000000001".parse().unwrap();
    // The prebuilt image embeds this fixed fixture UUID. Trust is explicitly
    // enabled by the public API below; signature admission still runs normally.
    tx.execute("INSERT INTO publishers(id,name,key_id,public_key_pem,status,created_at,updated_at) VALUES($1,'M3 disposable publisher','m3-ui-component',$2,'revoked',now(),now())",&[&publisher,&pem]).await.unwrap();
    tx.commit().await.unwrap();
    let mut running = Some(
        Running::start(
            config.clone(),
            db.clone(),
            hmac.clone(),
            public_tls.clone(),
            public_socket,
        )
        .await,
    );
    let mut report = json!({"test":"real_core_oci_oauth_webui_iframe","status":"running","production_capacity":"DEFERRED","metadata_budget":"synthetic_component_only_real_cgroups","aws":"DEFERRED","checks":{}});
    let mut diagnostic_targets = Vec::new();
    let outcome=std::panic::AssertUnwindSafe(async {
        let app=&running.as_ref().unwrap().app;let router=api::router(app.clone());let host=public_host(app);
        let login=status(api(&router,&host,"login",&[],None,json!({"account_id":owner,"password":password}),"owner-login",None).await,200,"login");
        let token=login["access_token"].as_str().unwrap().to_owned();
        status(api(&router,&host,"trustPublisher",&[],Some(&token),json!({"name":"M3 disposable publisher","key_id":"m3-ui-component","public_key_pem":pem}),"trust-publisher",None).await,200,"trustPublisher");
        let (web_manifest,web_installation)=installed(&router,&host,&token,"webui","XELAB_TEST_WEBUI_IMAGE").await;
        let (reference_manifest,reference_installation)=installed(&router,&host,&token,"ui-reference","XELAB_TEST_UI_REFERENCE_IMAGE").await;
        let mut web_config=json!({"core_origin":core,"client_id":null,"management_api_major":1,"default_page":"desktop","default_theme":"system","max_open_windows":16,"max_active_widgets":8,"widget_min_refresh_seconds":30,"ui_protocol_major":1});
        let web_deployment=create_deployment(&router,&host,&token,&web_installation,"M3 official WebUI",web_manifest["minimum_resources"].clone(),web_config.clone()).await;
        diagnostic_targets.push(DiagnosticTarget {role:"official_webui",deployment:web_deployment.parse().unwrap(),instance:None});
        let reference_deployment=create_deployment(&router,&host,&token,&reference_installation,"M3 independent UI reference",reference_manifest["minimum_resources"].clone(),json!({})).await;
        diagnostic_targets.push(DiagnosticTarget {role:"ui_reference",deployment:reference_deployment.parse().unwrap(),instance:None});
        let ui=format!("https://d-{}.{BASE_DOMAIN}:{}",web_deployment.replace('-',""),address.port());
        let reference_origin=format!("https://d-{}.{BASE_DOMAIN}:{}",reference_deployment.replace('-',""),address.port());
        let effective=status(api(&router,&host,"getEffectivePermissions",&[("account_id",owner.to_string())],Some(&token),Value::Null,"owner-scope",None).await,200,"getEffectivePermissions");
        let actions=["session.read","account.read","account.permissions.read","plugin.read","skill.read","skill.invoke","plugin.http.invoke","plugin.ui.open","platform.read","client.read","ui.preferences.read","ui.preferences.update","capability.call.read"];
        let rules:Vec<Value>=effective["rules"].as_array().unwrap().iter().filter(|rule|actions.contains(&rule["action"].as_str().unwrap())).cloned().collect();
        let client=status(api(&router,&host,"createClient",&[],Some(&token),json!({"name":"Actual OCI desktop","plugin_id":"org.xelab.webui","allowed_origins":[ui],"redirect_uris":[format!("{ui}/ui/org.xelab.webui/callback")],"maximum_rules":rules}),"create-client",None).await,201,"createClient");
        web_config["client_id"]=client["client_id"].clone();
        let web=read_deployment(&router,&host,&token,&web_deployment).await;let mut web_spec=web["spec"].clone();web_spec["config"]=web_config;
        let configured=status(api(&router,&host,"updateDeployment",&[("deployment_id",web_deployment.clone())],Some(&token),json!({"spec":web_spec}),"configure-real-client",web["revision"].as_i64()).await,202,"updateDeployment");
        let configured=operation(&router,&host,configured["id"].as_str().unwrap(),&token).await;
        contracts::validate("Deployment",&configured).unwrap();
        assert_eq!(configured["id"],web_deployment);assert_eq!(configured["spec"],web_spec);assert_eq!(configured["spec"]["enabled"],false);
        report["checks"]["registered_client_config_operation_completed"]=json!(true);
        approve(&router,&host,&token,&web_deployment,"approve-webui").await;approve(&router,&host,&token,&reference_deployment,"approve-reference").await;
        enabled(&router,&host,&token,&web_deployment,true,"enable-webui").await;enabled(&router,&host,&token,&reference_deployment,true,"enable-reference").await;
        wait_ready(app,&web_deployment).await;wait_ready(app,&reference_deployment).await;
        let approved_web=read_deployment(&router,&host,&token,&web_deployment).await;
        let approved_reference=read_deployment(&router,&host,&token,&reference_deployment).await;
        report["cgroup_limits"]=json!({"official_webui":instance_cgroup_limits(app,&web_deployment,&approved_web["spec"]["limits"]).await,"ui_reference":instance_cgroup_limits(app,&reference_deployment,&approved_reference["spec"]["limits"]).await});
        report["checks"]["both_oci_images_enforce_approved_cpu_memory_and_pids_cgroups"]=json!(true);
        report["diagnostics"]["baseline"]=diagnostic_snapshot(app,&mut diagnostic_targets,&controller_events).await;
        let skill=status(api(&router,&host,"registerSkill",&[],Some(&token),json!({"installation_id":reference_installation["installation_id"],"definition":reference_manifest["skills"][0],"enabled":false}),"register-fixed-skill",None).await,201,"registerSkill");
        let skill_id=skill["id"].as_str().unwrap().to_owned();
        status(api(&router,&host,"setSkillEnabled",&[("skill_id",skill_id.clone())],Some(&token),json!({"enabled":true}),"enable-fixed-skill",skill["revision"].as_i64()).await,200,"setSkillEnabled");
        let default=status(api(&router,&host,"getDefaultUi",&[],Some(&token),Value::Null,"read-default",None).await,200,"getDefaultUi");
        status(api(&router,&host,"setDefaultUi",&[],Some(&token),json!({"deployment_id":web_deployment}),"set-default",default["revision"].as_i64()).await,200,"setDefaultUi");
        report["checks"]["signed_images_public_install_approve_ready"]=json!(true);
        let child=status(api(&router,&host,"createChildAccount",&[("account_id",owner.to_string())],Some(&token),json!({"display_name":"Actual M3 limited caller","kind":"user","inheritance":{"mode":"follow_parent"}}),"create-limited-child",None).await,201,"createChildAccount");
        let child_id=child["account"]["id"].as_str().unwrap().to_owned();
        let child_rules=json!([
            {"action":"session.read","resource":{"kind":"account","match":"ids","ids":[child_id]}},
            {"action":"ui.preferences.read","resource":{"kind":"account","match":"ids","ids":[child_id]}},
            {"action":"ui.preferences.update","resource":{"kind":"account","match":"ids","ids":[child_id]}},
            {"action":"plugin.ui.open","resource":{"kind":"plugin","match":"ids","ids":["org.xelab.ui-reference"]}},
            {"action":"plugin.http.invoke","resource":{"kind":"plugin","match":"ids","ids":["org.xelab.ui-reference"]}},
            {"action":"skill.read","resource":{"kind":"skill","match":"ids","ids":[skill_id]}},
            {"action":"skill.invoke","resource":{"kind":"skill","match":"ids","ids":[skill_id]}},
            {"action":"capability.call.read","resource":{"kind":"call","match":"owned_by","owner_id":child_id}}
        ]);
        set_inheritance(&router,&host,&token,&child_id,&child_rules,"restrict-child-before-activation").await;
        status(api(&router,&host,"activateAccount",&[],None,json!({"activation_token":child["activation_token"],"password":child_password}),"activate-child",None).await,200,"activateAccount");
        let child_login=status(api(&router,&host,"login",&[],None,json!({"account_id":child_id,"password":child_password}),"child-cli-login",None).await,200,"login");
        let child_token=child_login["access_token"].as_str().unwrap().to_owned();
        let fixture=root.join("browser-private.json");let evidence=root.join("browser-safe.json");let progress=root.join("browser-progress.json");let control=root.join("browser-control.json");
        private_write(&fixture,&serde_json::to_vec(&json!({"core_origin":core,"ui_origin":ui,"reference_origin":reference_origin,"client_id":client["client_id"],"owner_id":owner,"owner_password":password,"child_id":child_id,"child_password":child_password,"skill_id":skill_id,"reference_deployment_id":reference_deployment,"tls_spki_sha256":spki,"connect_ip":"127.0.0.1","chromium_path":std::env::var("XELAB_OAUTH_BROWSER_CHROMIUM").unwrap_or_else(|_|"/usr/bin/chromium".into()),"progress_file":progress,"control_file":control})).unwrap());
        let driver=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/m3-ui-browser.py");
        let mut command=tokio::process::Command::new(std::env::var("XELAB_OAUTH_BROWSER_PYTHON").unwrap_or_else(|_|"python3".into()));
        command.arg(driver).arg("--fixture").arg(&fixture).arg("--output").arg(&evidence).kill_on_drop(true);
        let output=command.output();tokio::pin!(output);
        let mut revoked=false;
        let mut attack_usage:Option<(i64,i64,i64)>=None;
        let mut attack_checked=false;
        let mut withdrawn_usage:Option<(i64,i64)>=None;
        let mut owner_default_cleared=false;
        let mut child_default_restored=false;
        let output=tokio::time::timeout(Duration::from_secs(300),async {
            loop {
                tokio::select! {
                    result=&mut output=>break result.unwrap(),
                    _=tokio::time::sleep(Duration::from_millis(100))=>{
                        let state=fs::read(&progress).ok().and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok());
                        if !owner_default_cleared && state.as_ref().is_some_and(|state|state["phase"]=="await_owner_nondefault_ui") {
                            assert!(!child_default_restored && !attack_checked && !revoked,"The nondefault shell phase must precede all contribution actions");
                            let current=status(api(&router,&host,"getDefaultUi",&[],Some(&token),Value::Null,"read-owner-default",None).await,200,"getDefaultUi");
                            assert!(current["deployment_id"]==web_deployment,"The known WebUI must initially be default");
                            let cleared=status(api(&router,&host,"setDefaultUi",&[],Some(&token),json!({"deployment_id":null}),"clear-default-for-owner",current["revision"].as_i64()).await,200,"setDefaultUi");
                            assert!(cleared["deployment_id"].is_null(),"The default must be cleared through the public API");
                            let confirmed=status(api(&router,&host,"getDefaultUi",&[],Some(&token),Value::Null,"confirm-owner-nondefault",None).await,200,"getDefaultUi");
                            assert!(confirmed["deployment_id"].is_null(),"The owner shell must run while no default is selected");
                            status(api(&router,&host,"redirectDefaultUi",&[],None,Value::Null,"nondefault-root-unavailable",None).await,503,"redirectDefaultUi");
                            owner_default_cleared=true;
                            report["checks"]["owner_shell_has_no_default_ui_during_contribution_actions"]=json!(true);
                            tls::atomic_file(&control,br#"{"phase":"owner_nondefault_ui_ready","cleared":true}"#,0o600).unwrap();
                        }
                        if attack_usage.is_none() && state.as_ref().is_some_and(|state|state["phase"]=="await_attack_snapshot") {
                            assert!(owner_default_cleared && !child_default_restored,"Owner attacks must execute in the nondefault shell phase");
                            let usage=business_usage(&c,owner).await;assert_eq!(usage.2,0,"Attack snapshot requires no pending legitimate call");attack_usage=Some(usage);
                            tls::atomic_file(&control,br#"{"phase":"attack_snapshot_ready"}"#,0o600).unwrap();
                        }
                        if !attack_checked && state.as_ref().is_some_and(|state|state["phase"]=="await_attack_verification") {
                            assert!(owner_default_cleared && !child_default_restored,"Owner channel verification must remain nondefault");
                            let before=attack_usage.expect("The browser must synchronize before channel attacks");let after=business_usage(&c,owner).await;
                            assert_eq!(after.0-before.0,1,"Channel attacks must not allocate extra Call rows");assert_eq!(after.1-before.1,1,"Channel attacks must not consume extra daily quota");assert_eq!(after.2,0,"Channel attacks must not leak concurrent quota");
                            tls::atomic_file(&control,&serde_json::to_vec(&json!({"phase":"attack_verified","calls_delta":after.0-before.0,"daily_delta":after.1-before.1,"concurrent":after.2})).unwrap(),0o600).unwrap();attack_checked=true;
                            report["checks"]["channel_attacks_allocate_only_the_legitimate_call_and_quota"]=json!(true);
                        }
                        if !child_default_restored && state.as_ref().is_some_and(|state|state["phase"]=="await_child_default_ui") {
                            assert!(owner_default_cleared && attack_checked && !revoked,"Restore default only after the owner contribution proof and before child login");
                            let current=status(api(&router,&host,"getDefaultUi",&[],Some(&token),Value::Null,"read-child-default",None).await,200,"getDefaultUi");
                            assert!(current["deployment_id"].is_null(),"The owner contribution phase must not silently restore default UI");
                            status(api(&router,&host,"setDefaultUi",&[],Some(&token),json!({"deployment_id":web_deployment}),"restore-default-for-child",current["revision"].as_i64()).await,200,"setDefaultUi");
                            let confirmed=status(api(&router,&host,"getDefaultUi",&[],Some(&token),Value::Null,"confirm-child-default",None).await,200,"getDefaultUi");
                            assert!(confirmed["deployment_id"]==web_deployment,"Child root navigation must use the original WebUI");
                            status(api(&router,&host,"redirectDefaultUi",&[],None,Value::Null,"child-root-redirect-ready",None).await,302,"redirectDefaultUi");
                            child_default_restored=true;
                            report["checks"]["default_ui_restored_before_child_root_login"]=json!(true);
                            tls::atomic_file(&control,br#"{"phase":"child_default_ui_ready","restored":true}"#,0o600).unwrap();
                        }
                        if !revoked && state.as_ref().is_some_and(|state|state["phase"]=="await_revoke") {
                            assert!(attack_checked,"The owner channel attack evidence must be verified first");
                            assert!(owner_default_cleared && child_default_restored,"Child revocation follows real nondefault and restored root login phases");
                            let remaining=Value::Array(child_rules.as_array().unwrap().iter().filter(|rule|rule["action"]!="skill.invoke").cloned().collect());
                            let usage=c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) AS calls,daily_calls_used FROM quotas WHERE account_id=$1",&[&Uuid::parse_str(&child_id).unwrap()]).await.unwrap();
                            withdrawn_usage=Some((usage.get("calls"),usage.get("daily_calls_used")));
                            set_inheritance(&router,&host,&token,&child_id,&remaining,"withdraw-child-skill-authority").await;
                            tls::atomic_file(&control,br#"{"phase":"revoked","revoked":true}"#,0o600).unwrap();revoked=true;
                        }
                    }
                }
            }
        }).await.expect("M3 browser deadline exceeded");
        let browser_report:Value=fs::read(&evidence).ok().and_then(|bytes|serde_json::from_slice(&bytes).ok()).unwrap_or_else(||json!({"status":"failed","step":"report_missing"}));
        report["browser"]=browser_report.clone();
        // Never print child stderr/exception text: it can include callback URLs
        // or filled form data. Only the structured safe report enters evidence.
        assert!(output.status.success() && browser_report["status"]=="passed","M3 browser failed at safe step={}",browser_report["step"].as_str().unwrap_or("unknown"));
        assert!(revoked,"The actual inheritance withdrawal must have executed");
        assert!(attack_checked,"Actual channel attack admission/accounting evidence is required");
        assert!(owner_default_cleared && child_default_restored,"Both public default UI transitions are required");
        let usage=c.query_one("SELECT (SELECT count(*) FROM calls WHERE owner_id=$1) AS calls,daily_calls_used,concurrent_calls_used FROM quotas WHERE account_id=$1",&[&Uuid::parse_str(&child_id).unwrap()]).await.unwrap();
        assert_eq!(withdrawn_usage,Some((usage.get::<_,i64>("calls"),usage.get::<_,i64>("daily_calls_used"))));assert_eq!(usage.get::<_,i64>("concurrent_calls_used"),0);
        report["checks"]["withdrawn_calls_do_not_allocate_or_charge"]=json!(true);
        report["checks"]["real_browser_oauth_desktop_iframe_skill_and_revoke"]=json!(true);
        let counts=c.query_one("SELECT (SELECT count(*) FROM calls WHERE state='succeeded' AND skill_id=$1) AS successes,(SELECT count(*) FROM reservations WHERE kind='call' AND status<>'released') AS outstanding",&[&Uuid::parse_str(&skill_id).unwrap()]).await.unwrap();
        assert!(counts.get::<_,i64>("successes")>=3);assert_eq!(counts.get::<_,i64>("outstanding"),0);
        enabled(&router,&host,&token,&web_deployment,false,"disable-webui-keep-core").await;
        status(api(&router,&host,"getNode",&[],Some(&token),Value::Null,"core-without-webui",None).await,200,"getNode");
        assert_eq!(api(&router,&host,"redirectDefaultUi",&[],None,Value::Null,"disabled-root",None).await.0,503);
        private_write(&root.join("owner.token"),token.as_bytes());
        let cli=tokio::time::timeout(Duration::from_secs(30),tokio::process::Command::new(std::env::var("XELAB_M3_UI_CTL").unwrap_or_else(|_|env!("CARGO_BIN_EXE_xelabctl").into())).arg("api").arg("getNode").arg("--endpoint").arg(&core).arg("--ca-file").arg(root.join("public-ca.crt")).arg("--token-file").arg(root.join("owner.token")).env("NO_PROXY","core.m3.localhost,localhost,127.0.0.1").env("no_proxy","core.m3.localhost,localhost,127.0.0.1").kill_on_drop(true).output()).await.expect("Independent CLI deadline").unwrap();
        assert!(cli.status.success(),"Actual xelabctl must operate while WebUI is stopped");
        let cli_node:Value=serde_json::from_slice(&cli.stdout).expect("Actual CLI JSON response");contracts::validate("NodeStatus",&cli_node).unwrap();
        report["checks"]["actual_cli_after_webui_stop"]=json!(true);
        let before=status(api(&router,&host,"getNode",&[],Some(&token),Value::Null,"before-restart",None).await,200,"getNode");
        drop(router);
        running.take().unwrap().shutdown().await;
        running=Some(Running::start(config.clone(),db.clone(),hmac.clone(),public_tls.clone(),listener(address)).await);
        let app=&running.as_ref().unwrap().app;wait_ready(app,&reference_deployment).await;
        let router=api::router(app.clone());let host=public_host(app);
        let approved_reference=read_deployment(&router,&host,&token,&reference_deployment).await;
        let reference_limits=instance_cgroup_limits(app,&reference_deployment,&approved_reference["spec"]["limits"]).await;
        assert_eq!(reference_limits,report["cgroup_limits"]["ui_reference"]);
        report["checks"]["reference_cgroup_limits_persist_across_kernel_restart"]=json!(true);
        let after=status(api(&router,&host,"getNode",&[],Some(&token),Value::Null,"after-restart",None).await,200,"getNode");
        assert_eq!(before["reserved_cpu_millis"],after["reserved_cpu_millis"]);assert_eq!(before["reserved_memory_bytes"],after["reserved_memory_bytes"]);
        let fixed=json!({"input":{"message":"actual Skill after kernel component restart"},"provider_deployment_id":reference_deployment,"idempotency_key":"post-restart-intent","timeout_ms":10000});
        let called=status(api(&router,&host,"invokeSkill",&[("skill_id",skill_id.clone())],Some(&token),fixed.clone(),"post-restart-intent",None).await,200,"invokeSkill");assert_eq!(called["call"]["output"]["message"],fixed["input"]["message"]);
        let denied=api(&router,&host,"invokeSkill",&[("skill_id",skill_id.clone())],Some(&child_token),fixed,"withdrawn-child-after-restart",None).await;assert_eq!(denied.0,403);
        let web=read_deployment(&router,&host,&token,&web_deployment).await;assert_eq!(web["spec"]["enabled"],false);
        report["checks"]["disabled_webui_cli_core_and_persistent_restart"]=json!(true);
        report["checks"]["revoked_child_remains_denied_after_restart"]=json!(true);
        report["status"]=json!("passed");
    }).catch_unwind().await;
    if outcome.is_err() {
        report["status"] = json!("failed");
        // The overall browser timeout can unwind before its normal safe-report
        // import. Read only the bounded, structured evidence file, never stderr.
        report["browser_report_read_status"] = json!("missing_or_read_error");
        if let Ok(file) = fs::File::open(root.join("browser-safe.json")) {
            let mut bytes = Vec::new();
            if file.take(1_048_577).read_to_end(&mut bytes).is_ok() && bytes.len() <= 1_048_576 {
                if let Ok(safe) = serde_json::from_slice::<Value>(&bytes) {
                    report["browser"] = safe;
                    report["browser_report_read_status"] = json!("ok");
                } else {
                    report["browser_report_read_status"] = json!("parse_error");
                }
            } else {
                report["browser_report_read_status"] = json!("oversized_or_read_error");
            }
        }
        if let Some(active) = running.as_ref() {
            report["diagnostics"]["failure_before_shutdown"] =
                diagnostic_snapshot(&active.app, &mut diagnostic_targets, &controller_events).await;
        } else {
            report["diagnostics"]["failure_before_shutdown"] =
                json!({"status":"kernel_not_running"});
        }
        // Cleanup can itself fail. Preserve the original pre-cleanup state first
        // without leaking task IDs, paths, credentials, bodies or raw errors.
        eprintln!(
            "M3_UI_FAILURE_EVIDENCE {}",
            serde_json::to_string(&report).unwrap()
        );
    }
    if let Some(active) = running.take() {
        active.shutdown().await;
    }
    let cleanup = Runtime::new(config);
    for row in c.query("SELECT id FROM instances", &[]).await.unwrap() {
        cleanup.stop(row.get("id")).await.unwrap();
    }
    assert!(cleanup.inventory().await.unwrap().is_empty());
    loopback_host
        .remove()
        .expect("The test must remove its own loopback alias line");
    report["checks"]["isolated_cli_alias_restored"] = json!(true);
    eprintln!("M3_UI_EVIDENCE {}", serde_json::to_string(&report).unwrap());
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
    assert!(
        controller_events.installed.load(Ordering::SeqCst),
        "Controller diagnostic subscriber initialization failed"
    );
}
