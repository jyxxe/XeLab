//! Validate an active worker lease for a heartbeat or for input preparation.
use super::{
    active::{live, LockedAttempt, Operation},
    DispatchInput, ENGINE_TIMEOUT,
};
use crate::{
    auth::Auth,
    contracts::unsigned,
    db::Db,
    error::{Error, Result},
    plugin_catalog::attempt_value,
    tasks::dispatch_input,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;
use std::time::Instant;
use uuid::Uuid;

/// Identity of a claim held by a trusted in-process worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttemptLease {
    pub worker_id: Uuid,
    pub attempt_id: Uuid,
    pub execution_id: Uuid,
    pub fence: i64,
}

/// Renew an active lease within the original step timeout, task deadline and
/// revalidated delegation and authority limits. Returns the public Attempt.
pub async fn renew(db: &Db, auth: &Auth, lease: &AttemptLease) -> Result<Value> {
    tokio::time::timeout(
        ENGINE_TIMEOUT,
        with_active_lease(db, auth, lease, Operation::Renew, extend),
    )
    .await
    .map_err(|_| Error::unavailable("Task lease renewal timed out"))?
}

/// Build the input for a currently authorized, leased step without consuming
/// the execution or extending its lease. The request has a stable timeout
/// for Broker idempotency. [`DispatchInput::deadline_at`] is the absolute
/// step, task and authority cutoff, independent of heartbeats.
///
/// Prepared data conveys no authorization. At dispatch the Broker must
/// revalidate the current lease, fence, original authority and binding.
/// Admission and transport must cap the static timeout by the remaining
/// hard deadline, including time spent queued.
pub async fn prepare_dispatch(db: &Db, auth: &Auth, lease: &AttemptLease) -> Result<DispatchInput> {
    tokio::time::timeout(
        ENGINE_TIMEOUT,
        with_active_lease(
            db,
            auth,
            lease,
            Operation::PrepareDispatch,
            async |attempt, deadline_at| {
                let input =
                    dispatch_input::build(&attempt.tx, attempt.row.get("step_id"), deadline_at)
                        .await?;
                Ok((input, attempt.lease_end.min(deadline_at)))
            },
        ),
    )
    .await
    .map_err(|_| Error::unavailable("Task dispatch preparation timed out"))?
}

async fn extend(
    attempt: &LockedAttempt<'_>,
    limit: DateTime<Utc>,
) -> Result<(Value, DateTime<Utc>)> {
    // Run deferred attempt triggers during UPDATE so they are included in
    // the final check against the previous lease expiry.
    attempt
        .tx
        .batch_execute("SET CONSTRAINTS ALL IMMEDIATE")
        .await?;
    let row = attempt
        .tx
        .query_opt(
            "WITH renewed AS MATERIALIZED (SELECT clock_timestamp() AS at)
         UPDATE attempts a SET lease_expires_at=least($2,at+interval '30 seconds'),
                               revision=a.revision+1,updated_at=at
           FROM renewed WHERE id=$1 AND at<least(a.lease_expires_at,$2)
         RETURNING to_jsonb(a) AS attempt,a.created_at,a.updated_at,a.lease_expires_at",
            &[&attempt.lease.attempt_id, &limit],
        )
        .await?
        .ok_or_else(|| Error::conflict("Attempt lease expired during renewal"))?;
    Ok((attempt_value(&row)?, row.get("lease_expires_at")))
}

async fn with_active_lease<T>(
    db: &Db,
    auth: &Auth,
    lease: &AttemptLease,
    operation: Operation,
    work: impl AsyncFnOnce(&LockedAttempt<'_>, DateTime<Utc>) -> Result<(T, DateTime<Utc>)>,
) -> Result<T> {
    let mut client = db.connect_scoped(ENGINE_TIMEOUT).await?;
    let attempt = LockedAttempt::lock(client.transaction().await?, lease, operation).await?;
    if !attempt.active() {
        return attempt.reject().await;
    }
    let (attempt, authorized_until) = attempt.authorize(auth, false).await?;
    let timeout_ms = i32::try_from(unsigned(&attempt.step["timeout_ms"])?)
        .map_err(|_| Error::invalid("Step timeout is out of range"))?;
    let step_end = attempt
        .row
        .get::<_, DateTime<Utc>>("created_at")
        .checked_add_signed(TimeDelta::milliseconds(i64::from(timeout_ms)))
        .ok_or_else(|| Error::invalid("Step deadline is out of range"))?;
    let limit = authorized_until.min(step_end);
    let old_end = limit.min(attempt.lease_end);
    if !live(&attempt.tx, old_end).await? {
        return attempt.reject().await;
    }
    attempt
        .tx
        .batch_execute("SAVEPOINT before_worker_operation")
        .await?;
    let result = work(&attempt, limit).await;
    if result.is_err() {
        // A failed SQL statement aborts its subtransaction. Recover before
        // checking whether the worker lost its lease while the statement ran.
        attempt
            .tx
            .batch_execute("ROLLBACK TO SAVEPOINT before_worker_operation")
            .await?;
    }
    // Sample after mapping, locking, triggers and public projection. Check
    // the *old* lease even if the UPDATE just produced a later expiry.
    let sampled = Instant::now();
    let check = attempt
        .tx
        .query_one(
            "SELECT clock_timestamp(), NOT EXISTS (
                          SELECT FROM platform_recovery_state WHERE phase<>'ready')",
            &[],
        )
        .await?;
    let now: DateTime<Utc> = check.get(0);
    let before =
        |until: DateTime<Utc>| sampled.elapsed() < (until - now).to_std().unwrap_or_default();
    if !before(old_end) || !check.get::<_, bool>(1) {
        if result.is_ok() && matches!(operation, Operation::Renew) {
            attempt
                .tx
                .batch_execute("ROLLBACK TO SAVEPOINT before_worker_operation")
                .await?;
        }
        return attempt.reject().await;
    }
    let (value, expires) = result?;
    let row = attempt.finish().await?;
    if !before(expires) {
        // As in claim(), a delayed COMMIT acknowledgement must never hand
        // an expired lease or its prepared request to the worker.
        return operation
            .reject(client.transaction().await?, lease, Some(&row))
            .await;
    }
    Ok(value)
}
