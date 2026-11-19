//! Dispatch a committed Broker admission for a trusted, fenced TaskEngine worker.
use super::{
    admission::{self, Call, TaskAdmission},
    race_live_call, rpc_contract, settle_business, unary_capability_error, unary_failure,
    CallReply, CallSettlement, ReplyBinding,
};
use crate::{
    api::App,
    contracts::digest,
    controller, db,
    error::{Error, Result},
    rpc::{self, capability_service_client::CapabilityServiceClient},
    skill_streams::ensure_active,
    task_engine::{self, AttemptLease},
    tls,
};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::{timeout, timeout_at, Instant};
use uuid::Uuid;

const METADATA_TIMEOUT: Duration = Duration::from_secs(10);
const MONITOR_INTERVAL: Duration = Duration::from_millis(100);

/// Run one already-claimed fixed signed unary Capability attempt. A successful
/// return is the fenced TaskStep, including a confirmed business failure if any.
/// An unconfirmed transport outcome is reported as an error with its Call ID.
/// Reentry consults the associated Call and can acknowledge an existing result.
pub async fn dispatch_task(app: &App, lease: &AttemptLease) -> Result<Value> {
    ensure_active(app)?;
    match admission::admit_task(app, lease).await? {
        TaskAdmission::Existing(call) => acknowledge(app, lease, call).await,
        TaskAdmission::Dispatch(admitted) => dispatch_new(app, lease, *admitted).await,
    }
}

/// Claim and dispatch the next Broker-supported step. Admission failures leave
/// the step ready when the Claim and Call transaction did not commit.
pub async fn dispatch_next_task(app: &App, worker_id: Uuid) -> Result<Option<Value>> {
    dispatch_next_task_after(app, worker_id, &mut None).await
}

pub(crate) async fn dispatch_next_task_after(
    app: &App,
    worker_id: Uuid,
    after: &mut Option<(DateTime<Utc>, Uuid)>,
) -> Result<Option<Value>> {
    ensure_active(app)?;
    let Some((lease, admitted)) = admission::admit_next_task(app, worker_id, after).await? else {
        return Ok(None);
    };
    dispatch_new(app, &lease, *admitted).await.map(Some)
}

async fn acknowledge(app: &App, lease: &AttemptLease, call: Uuid) -> Result<Value> {
    // A durable admission has no second transport token. Only a known Call
    // result may advance the Attempt, and the usual fenced commit checks it.
    let row = timeout(METADATA_TIMEOUT, async {
        let c = app.db.connect_scoped(METADATA_TIMEOUT).await?;
        c.query_one("SELECT state,result,error FROM calls WHERE id=$1", &[&call])
            .await
            .map_err(Error::from)
    })
    .await
    .map_err(|_| Error::unavailable("Call acknowledgement timed out").with_resource(call))??;
    match row.try_get::<_, &str>("state")? {
        "succeeded" => {
            let result: Value = row.try_get("result")?;
            let output = result.get("output").ok_or_else(|| unconfirmed(call))?;
            commit_attempt(app, lease, call, "succeeded", output, &Value::Null).await
        }
        "failed" => {
            let error: Value = row.try_get("error")?;
            commit_attempt(app, lease, call, "failed", &Value::Null, &error).await
        }
        _ => Err(unconfirmed(call)),
    }
}

async fn dispatch_new(app: &App, lease: &AttemptLease, mut admitted: Call) -> Result<Value> {
    let call = admitted.id;
    let end = Instant::now()
        + (admitted.deadline - Utc::now())
            .to_std()
            .unwrap_or_default();
    let wire = rpc_contract(&admitted.contract)?;
    let monitor = async {
        loop {
            tokio::time::sleep(MONITOR_INTERVAL).await;
            // A cancel request forbids a new send. Once sent, retain any
            // known result that can still be committed with the current fence.
            check_live(app, lease, call, end, true).await?;
        }
    };
    let bytes = std::mem::take(&mut admitted.bytes);
    let delegation = std::mem::take(&mut admitted.delegation);
    let execution = execute(app, lease, &admitted, &wire, end, bytes, delegation);
    let outcome = timeout_at(
        end,
        race_live_call(execution, monitor, std::future::pending()),
    )
    .await
    .unwrap_or_else(|_| Err(expired(call)));
    let outcome = timeout(
        METADATA_TIMEOUT,
        settle(app, lease, &admitted, &wire, outcome),
    )
    .await
    .map_err(|_| Error::unavailable("Call settlement timed out").with_resource(call))?
    .map_err(|error| error.with_resource(call))?;

    match outcome {
        Ok(CallReply::Completed(result)) => {
            commit_attempt(
                app,
                lease,
                call,
                "succeeded",
                &result["output"],
                &Value::Null,
            )
            .await
        }
        Ok(CallReply::Rejected(error)) => {
            commit_attempt(app, lease, call, "failed", &Value::Null, &error.detail()).await
        }
        Err(error) => {
            // If the lease or original rights have ended, the controller
            // projects UNKNOWN when it reconciles the expired attempt.
            if let Err(failure) =
                commit_attempt(app, lease, call, "unknown", &Value::Null, &error.detail()).await
            {
                tracing::debug!(%call, code = failure.code, "Task outcome pending reconciliation");
            }
            Err(error.with_resource(call))
        }
    }
}

async fn execute(
    app: &App,
    lease: &AttemptLease,
    admitted: &Call,
    wire: &rpc::ContractRef,
    end: Instant,
    bytes: Vec<u8>,
    delegation: String,
) -> Result<CallReply> {
    let call = admitted.id;
    let instance = admitted.instance;
    let address = app.runtime.address(instance.id)?;
    let remaining = || end.saturating_duration_since(Instant::now());
    let channel = tls::channel_with_timeout(
        &app.config,
        instance.id,
        instance.generation,
        &address,
        remaining(),
    )
    .await?;
    let mut client = CapabilityServiceClient::new(channel)
        .max_decoding_message_size(app.config.rpc_limit(16_778_240));
    let (deadline, guarded_end) = check_live(app, lease, call, end, false).await?;
    let bound = guarded_end.saturating_duration_since(Instant::now());
    if bound.is_zero() {
        return Err(expired(call));
    }
    let mut context = controller::context(Some(admitted.ctx.subject));
    context.request_id = admitted.ctx.request.to_string();
    context.delegation_token = delegation;
    context.deadline_unix_ms = deadline.min(admitted.deadline).timestamp_millis();
    context.idempotency_key = admitted.business_key.clone();
    let mut request = tonic::Request::new(rpc::InvokeRequest {
        context: Some(context),
        call_id: call.to_string(),
        contract: Some(wire.clone()),
        input_digest: digest(&bytes),
        input_json: bytes,
        binding_id: String::new(),
        terminal_failure_version: unary_failure::VERSION,
    });
    request.set_timeout(bound);
    let response = client
        .invoke(request)
        .await
        .map_err(unary_capability_error)?
        .into_inner();
    ReplyBinding {
        call,
        instance: instance.id,
        contract: &admitted.contract,
        wire_contract: wire,
        definition: &admitted.definition,
        skill: None,
        terminal_version: unary_failure::VERSION,
    }
    .decode(app, response)
}

async fn check_live(
    app: &App,
    lease: &AttemptLease,
    call: Uuid,
    end: Instant,
    allow_cancel: bool,
) -> Result<(DateTime<Utc>, Instant)> {
    ensure_active(app)?;
    let mut c = app
        .db
        .connect_scoped(end.saturating_duration_since(Instant::now()))
        .await?;
    let tx = c.transaction().await?;
    db::auth_lock(&tx, false).await?;
    let deadline = admission::validate_task_call(&tx, app, lease, call, allow_cancel).await?;
    let sampled = Instant::now();
    let now: DateTime<Utc> = tx
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .try_get(0)?;
    let end = end.min(sampled + (deadline - now).to_std().unwrap_or_default());
    tx.commit().await?;
    ensure_active(app)?;
    if Instant::now() >= end {
        return Err(expired(call));
    }
    Ok((deadline, end))
}

async fn settle(
    app: &App,
    lease: &AttemptLease,
    admitted: &Call,
    wire: &rpc::ContractRef,
    mut outcome: Result<CallReply>,
) -> Result<Result<CallReply>> {
    ensure_active(app)?;
    let call = admitted.id;
    let mut c = app.db.connect_scoped(METADATA_TIMEOUT).await?;
    let tx = c.transaction().await?;
    db::call_lock(&tx).await?;
    tx.batch_execute("SAVEPOINT result_evidence; SET CONSTRAINTS ALL IMMEDIATE")
        .await?;
    if let Ok(reply) = &outcome {
        let checked: Result<DateTime<Utc>> = async {
            let limit = admission::validate_task_call(&tx, app, lease, call, true).await?;
            let signed = admitted.terminal.as_ref().ok_or_else(Error::denied)?;
            signed
                .verify(
                    &tx,
                    app,
                    admitted.instance,
                    admitted.deployment,
                    wire,
                    &admitted.definition,
                )
                .await?;
            Ok(limit.min(admitted.deadline))
        }
        .await;
        match checked {
            Ok(limit) => {
                let confirmed = match reply {
                    CallReply::Completed(out) => CallSettlement::Completed(out),
                    CallReply::Rejected(error) => CallSettlement::Failed(error),
                };
                settle_business(
                    &tx,
                    &admitted.ctx,
                    call,
                    &admitted.accounts,
                    "capability.invoke",
                    confirmed,
                )
                .await?;
                let allowed: bool = tx
                    .query_one(
                        "SELECT clock_timestamp()<$1 AND NOT EXISTS (
                          SELECT FROM platform_recovery_state WHERE phase<>'ready')",
                        &[&limit],
                    )
                    .await?
                    .try_get(0)?;
                if allowed {
                    ensure_active(app)?;
                    tx.commit().await?;
                    app.runtime.business_queue.wake_head(admitted.instance);
                    return Ok(outcome);
                }
                outcome = Err(expired(call));
            }
            Err(error) => outcome = Err(error),
        }
        // A failed verification or late result must not release the physical
        // reservation or leave a confirmed Call in the ledger.
        tx.batch_execute("ROLLBACK TO SAVEPOINT result_evidence")
            .await?;
    }
    let Err(ref error) = outcome else {
        unreachable!("confirmed results commit above")
    };
    settle_business(
        &tx,
        &admitted.ctx,
        call,
        &admitted.accounts,
        "capability.invoke",
        CallSettlement::Unknown(error),
    )
    .await?;
    ensure_active(app)?;
    tx.commit().await?;
    Ok(outcome)
}

async fn commit_attempt(
    app: &App,
    lease: &AttemptLease,
    call: Uuid,
    state: &str,
    output: &Value,
    error: &Value,
) -> Result<Value> {
    ensure_active(app)?;
    let input = json!({"attempt_id":lease.attempt_id,"execution_id":lease.execution_id,
        "fence":lease.fence,"state":state,"output":output,"error":error});
    task_engine::commit(&app.db, &app.auth, lease.worker_id, &input)
        .await
        .map_err(|error| error.with_resource(call))
}

fn expired(call: Uuid) -> Error {
    Error::new(
        "DEADLINE_EXCEEDED",
        "Task Call deadline elapsed; inspect the admitted Call",
        StatusCode::GATEWAY_TIMEOUT,
    )
    .with_resource(call)
}

fn unconfirmed(call: Uuid) -> Error {
    Error::new(
        "OUTCOME_UNKNOWN",
        "Existing Task Call has no confirmed outcome; inspect the admitted Call",
        StatusCode::CONFLICT,
    )
    .with_resource(call)
}
