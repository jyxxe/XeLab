use clap::Parser;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
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
    let worker = tokio::spawn(controller::run(app.clone()));
    let volume_worker = tokio::spawn(xelab_kernel::volumes::run(app.clone()));
    let blob_worker = tokio::spawn(xelab_kernel::blobs::run(app.clone()));
    let backup_worker = tokio::spawn(xelab_kernel::backups::run(app.clone()));
    let upgrade_worker = tokio::spawn(xelab_kernel::upgrades::run(app.clone()));
    let schema_worker = tokio::spawn(xelab_kernel::schema_jobs::run(app.clone()));
    let host_app = app.clone();
    let host_worker = tokio::spawn(async move {
        if xelab_kernel::host::serve(host_app.clone()).await.is_err() {
            tracing::error!(reason_code = "HOST_SERVICE_STOPPED");
            host_app.ready.store(false, Ordering::SeqCst);
            host_app.shutdown.store(true, Ordering::SeqCst);
        }
    });
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        config.text("server", "tls_cert_file"),
        config.text("server", "tls_key_file"),
    )
    .await?;
    let handle = axum_server::Handle::new();
    let monitor_app = app.clone();
    let monitor_handle = handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if monitor_app.leader.is_closed() || monitor_app.shutdown.load(Ordering::SeqCst) {
                monitor_app.ready.store(false, Ordering::SeqCst);
                monitor_app.shutdown.store(true, Ordering::SeqCst);
                monitor_handle.graceful_shutdown(Some(std::time::Duration::from_secs(1)));
                break;
            }
        }
    });
    let stop = handle.clone();
    let signal_app = app.clone();
    tokio::spawn(async move {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal registration");
        tokio::select! {_=tokio::signal::ctrl_c()=>(),_=term.recv()=>()};
        signal_app.ready.store(false, Ordering::SeqCst);
        signal_app.shutdown.store(true, Ordering::SeqCst);
        stop.graceful_shutdown(Some(std::time::Duration::from_millis(
            signal_app.config.value["resources"]["shutdown_grace_ms"]
                .as_u64()
                .unwrap(),
        )));
    });
    axum_server::bind_rustls(config.listen()?, tls)
        .handle(handle)
        .serve(router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await?;
    app.shutdown.store(true, Ordering::SeqCst);
    worker.abort();
    host_worker.abort();
    blob_worker.abort();
    backup_worker.abort();
    upgrade_worker.abort();
    schema_worker.abort();
    volume_worker.abort();
    if let Ok(c) = app.db.connect().await {
        let _ = c
            .execute("UPDATE node_budget SET admission_enabled=false", &[])
            .await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
