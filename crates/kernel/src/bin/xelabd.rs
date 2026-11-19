use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::time::Instant;
use xelab_kernel::{
    admission,
    api::{router, App},
    auth::Auth,
    config::{require_host_admin, Config},
    controller,
    db::Db,
    error::{Error, Result},
    runtime::Runtime,
    tls,
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "/etc/xelab/kernel.toml")]
    config: PathBuf,
    #[arg(long, hide = true)]
    inherited_host_lease: Option<InheritedHostLease>,
}

#[derive(Clone, Debug)]
struct InheritedHostLease {
    fd: i32,
    cpu: i64,
    memory: i64,
}

impl std::str::FromStr for InheritedHostLease {
    type Err = &'static str;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let invalid = "Invalid inherited host lease";
        let mut fields = value.split(':');
        let fd = fields.next().ok_or(invalid)?.parse().map_err(|_| invalid)?;
        let cpu = fields.next().ok_or(invalid)?.parse().map_err(|_| invalid)?;
        let memory = fields.next().ok_or(invalid)?.parse().map_err(|_| invalid)?;
        if fd < 3 || cpu <= 0 || memory <= 0 || fields.next().is_some() {
            return Err(invalid);
        }
        Ok(Self { fd, cpu, memory })
    }
}
fn main() {
    xelab_kernel::init_crypto();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if let Err(e) = run(Args::parse()) {
        eprintln!("{e}");
        std::process::exit(7);
    }
}
fn run(args: Args) -> Result<()> {
    require_host_admin()?;
    // This descriptor has no Rust owner in the new process image. Adopt it
    // before reading configuration, and restore CLOEXEC before any adapters run.
    let inherited_lease = args
        .inherited_host_lease
        .as_ref()
        .map(|inherited| unsafe { Runtime::inherited_host_lease(inherited.fd) })
        .transpose()?;
    let config = Config::load(&args.config)?;
    let runtime = Runtime::new(config.clone());
    let lease = match inherited_lease {
        Some(lease) => lease,
        None => runtime.host_lease()?,
    };
    let capacity = if let Some(inherited) = args.inherited_host_lease {
        if !config.compact() {
            return Err(Error::invalid(
                "Inherited startup requires the compact profile",
            ));
        }
        let capacity = admission::compact_host_capacity((inherited.cpu, inherited.memory))?;
        // Validate and reapply the newly loaded file's limits in case it changed
        // across exec. Persisted resource budgets are applied by preflight later.
        runtime.prepare_kernel_cgroup(capacity.0, capacity.1)?;
        capacity
    } else {
        let capacity = admission::host_capacity()?;
        if config.compact() {
            runtime.prepare_kernel_cgroup(capacity.0, capacity.1)?;
            return Runtime::reexec_compact(&lease, &args.config, capacity);
        }
        capacity
    };
    Runtime::retain_host_lease(lease)?;
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if config.compact() {
        // Archive/encryption pipelines need several simultaneous blocking jobs.
        builder.worker_threads(1).max_blocking_threads(8);
    }
    builder.build()?.block_on(start(config, capacity))
}
async fn start(mut config: Config, (cpu, memory): (i64, i64)) -> Result<()> {
    let (dsn, key) = config.secrets()?;
    let mut db = Db::new(dsn)?;
    if config.compact() {
        db = db.with_connection_limit(16);
    }
    db.check().await?;
    let leader = Arc::new(db.leader().await?);
    let mut c = db.connect().await?;
    let tx = c.transaction().await?;
    xelab_kernel::auth::seed_roles(&tx).await?;
    admission::initialize(&tx, &config.value["resources"], cpu, memory).await?;
    tx.commit().await?;
    let persisted: serde_json::Value = c
        .query_one("SELECT config FROM node_budget", &[])
        .await?
        .get(0);
    if (persisted["profile"] == "compact") != config.compact() {
        return Err(xelab_kernel::error::Error::conflict(
            "Resource profile differs from persisted node budget; use the existing profile",
        ));
    }
    config.value["resources"] = persisted;
    drop(c);
    let runtime = Runtime::new(config.clone());
    runtime.preflight(cpu, memory).await?;
    tls::issue(&config, None, &config.root().join("kernel-identity"))?;
    let auth = Auth::new(key)?;
    let app = Arc::new(App {
        config: config.clone(),
        db,
        auth,
        runtime,
        leader,
        ready: AtomicBool::new(false),
        shutdown: AtomicBool::new(false),
        blob_uploads: tokio::sync::Semaphore::new(if config.compact() { 1 } else { 4 }),
    });
    let listen = config.listen()?;
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        config.text("server", "tls_cert_file"),
        config.text("server", "tls_key_file"),
    )
    .await?;
    let shutdown_grace = Duration::from_millis(
        app.config.value["resources"]["shutdown_grace_ms"]
            .as_u64()
            .unwrap(),
    );
    let failure_grace = shutdown_grace.min(Duration::from_secs(1));
    let shutdown_deadline = Arc::new(OnceLock::new());
    let worker = tokio::spawn(controller::run(app.clone()));
    let mut task_worker = tokio::spawn(xelab_kernel::task_engine::run(app.clone()));
    let volume_worker = tokio::spawn(xelab_kernel::volumes::run(app.clone()));
    let blob_worker = tokio::spawn(xelab_kernel::blobs::run(app.clone()));
    let backup_worker = tokio::spawn(xelab_kernel::backups::run(app.clone()));
    let upgrade_worker = tokio::spawn(xelab_kernel::upgrades::run(app.clone()));
    let schema_worker = tokio::spawn(xelab_kernel::schema_jobs::run(app.clone()));
    let host_app = app.clone();
    let host_deadline = shutdown_deadline.clone();
    let host_worker = tokio::spawn(async move {
        if xelab_kernel::host::serve(host_app.clone()).await.is_err() {
            tracing::error!(reason_code = "HOST_SERVICE_STOPPED");
            begin_shutdown(&host_app, &host_deadline, failure_grace);
        }
    });
    let handle = axum_server::Handle::new();
    let monitor_app = app.clone();
    let monitor_deadline = shutdown_deadline.clone();
    let monitor_handle = handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if monitor_app.leader.is_closed() || monitor_app.shutdown.load(Ordering::SeqCst) {
                let deadline = begin_shutdown(&monitor_app, &monitor_deadline, failure_grace);
                monitor_handle
                    .graceful_shutdown(Some(deadline.saturating_duration_since(Instant::now())));
                break;
            }
        }
    });
    let stop = handle.clone();
    let signal_app = app.clone();
    let signal_deadline = shutdown_deadline.clone();
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal registration");
        tokio::select! {_=tokio::signal::ctrl_c()=>(),_=term.recv()=>()};
        let deadline = begin_shutdown(&signal_app, &signal_deadline, shutdown_grace);
        stop.graceful_shutdown(Some(deadline.saturating_duration_since(Instant::now())));
    });
    let served = axum_server::bind_rustls(listen, tls)
        .handle(handle)
        .serve(router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await;
    let deadline = begin_shutdown(&app, &shutdown_deadline, shutdown_grace);
    worker.abort();
    host_worker.abort();
    blob_worker.abort();
    backup_worker.abort();
    upgrade_worker.abort();
    schema_worker.abort();
    volume_worker.abort();
    if tokio::time::timeout_at(deadline, &mut task_worker)
        .await
        .is_err()
    {
        task_worker.abort();
        let _ = task_worker.await;
    }
    if let Ok(c) = app.db.connect().await {
        let _ = c
            .execute("UPDATE node_budget SET admission_enabled=false", &[])
            .await;
    }
    served.map_err(Error::from)
}

fn drain_cutoff(deadline: &OnceLock<Instant>, grace: Duration) -> Instant {
    *deadline.get_or_init(|| Instant::now() + grace)
}

fn begin_shutdown(app: &App, deadline: &OnceLock<Instant>, grace: Duration) -> Instant {
    let cutoff = drain_cutoff(deadline, grace);
    app.ready.store(false, Ordering::SeqCst);
    app.shutdown.store(true, Ordering::SeqCst);
    cutoff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_and_task_shutdown_share_the_first_deadline() {
        let deadline = OnceLock::new();
        let first = drain_cutoff(&deadline, Duration::from_secs(1));
        assert_eq!(drain_cutoff(&deadline, Duration::from_secs(30)), first);
        assert_eq!(drain_cutoff(&deadline, Duration::ZERO), first);
    }

    #[test]
    fn compact_startup_descriptor_requires_complete_positive_capacity() {
        let value: InheritedHostLease = "3:1000:536870912".parse().unwrap();
        assert_eq!((value.fd, value.cpu, value.memory), (3, 1000, 536870912));
        for invalid in [
            "",
            "3",
            "3:1000",
            "2:1000:536870912",
            "-1:1000:536870912",
            "3:0:536870912",
            "3:1000:0",
            "3:1000:-1",
            "3:1000:536870912:extra",
            "9999999999999999999:1000:536870912",
            "3:invalid:536870912",
        ] {
            assert!(invalid.parse::<InheritedHostLease>().is_err(), "{invalid}");
        }
    }
}
