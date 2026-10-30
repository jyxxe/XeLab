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
    error::Result,
    runtime::Runtime,
    tls,
};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "/etc/xelab/kernel.toml")]
    config: PathBuf,
}
#[tokio::main]
async fn main() {
    xelab_kernel::init_crypto();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if let Err(e) = start(Args::parse()).await {
        eprintln!("{e}");
        std::process::exit(7);
    }
}
async fn start(args: Args) -> Result<()> {
    require_host_admin()?;
    let mut config = Config::load(&args.config)?;
    let _host_lease = Runtime::new(config.clone()).host_lease()?;
    let (dsn, key) = config.secrets()?;
    let db = Db::new(dsn)?;
    db.check().await?;
    let leader = Arc::new(db.leader().await?);
    let (cpu, memory) = admission::host_capacity()?;
    let mut c = db.connect().await?;
    let tx = c.transaction().await?;
    admission::initialize(&tx, &config.value["resources"], cpu, memory).await?;
    tx.commit().await?;
    config.value["resources"] = c.query_one("SELECT config FROM node_budget", &[]).await?.get(0);
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
    });
    let worker = tokio::spawn(controller::run(app.clone()));
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(
        config.text("server", "tls_cert_file"),
        config.text("server", "tls_key_file"),
    )
    .await?;
    let handle = axum_server::Handle::new();
    let monitor_app=app.clone();
    let monitor_handle=handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if monitor_app.leader.is_closed() {
                monitor_app.ready.store(false,Ordering::SeqCst);
                monitor_app.shutdown.store(true,Ordering::SeqCst);
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
        stop.graceful_shutdown(Some(std::time::Duration::from_millis(signal_app.config.value["resources"]["shutdown_grace_ms"].as_u64().unwrap())));
    });
    axum_server::bind_rustls(config.listen()?, tls)
        .handle(handle)
        .serve(router(app.clone()).into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await?;
    app.shutdown.store(true, Ordering::SeqCst);
    worker.abort();
    if let Ok(c) = app.db.connect().await {
        let _ = c
            .execute("UPDATE node_budget SET admission_enabled=false", &[])
            .await;
    }
    Ok(())
}
