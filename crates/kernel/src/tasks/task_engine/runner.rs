//! Bounded in-process workers for broker-supported workflow steps.
use crate::{api::App, broker::dispatch_next_task_after};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::task::JoinSet;
use uuid::Uuid;

const IDLE_DELAY: Duration = Duration::from_millis(250);

/// Run alongside the controller. The durable Call ledger owns recovery for
/// work interrupted by shutdown or an uncertain worker exit.
pub async fn run(app: Arc<App>) {
    let mut workers = JoinSet::new();
    let slots = if app.config.compact() { 1 } else { 2 };
    for _ in 0..slots {
        workers.spawn(worker(app.clone()));
    }
    while let Some(result) = workers.join_next().await {
        if result.is_err() && !stopping(&app) {
            tracing::error!(reason_code = "TASK_WORKER_EXITED");
            // Do not reuse an interrupted worker's identity or claim. The
            // controller reconciles its durable Attempt and Call as before.
            let app = app.clone();
            workers.spawn(async move {
                tokio::time::sleep(IDLE_DELAY).await;
                worker(app).await;
            });
        }
    }
}

async fn worker(app: Arc<App>) {
    let worker_id = Uuid::new_v4();
    let mut after = None;
    while !stopping(&app) {
        if app.ready.load(Ordering::SeqCst) {
            match dispatch_next_task_after(&app, worker_id, &mut after).await {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => tracing::debug!(code = error.code, "Task dispatch did not complete"),
            }
        }
        tokio::time::sleep(IDLE_DELAY).await;
    }
}

fn stopping(app: &App) -> bool {
    app.shutdown.load(Ordering::SeqCst) || app.leader.is_closed()
}
