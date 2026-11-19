//! Reconcile due UTC occurrences through a durable fire ledger.
use crate::{
    api::App,
    auth::{Auth, Resource},
    contracts::string,
    db::{self, Db},
    error::{Error, Result},
    schedule_cron::{CronSchedule, MisfirePolicy, OverlapPolicy, RecoveryDecision, RecoveryInputs},
    tasks,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration as Timeout,
};
use tokio_postgres::{Row, Transaction};
use uuid::Uuid;

#[cfg(test)]
mod tests;

const BATCH_SIZE: usize = 16;
const BATCH_TIMEOUT: Timeout = Timeout::from_secs(10);

/// Match automatic firing's Schedule → Task lock order before an explicit
/// retry can move a settled scheduled Task back into the active set.
pub(crate) async fn lock_retry_origin(tx: &Transaction<'_>, task: Uuid) -> Result<Option<Uuid>> {
    let schedules = tx
        .query(
            "SELECT s.id FROM schedules s WHERE s.admission IS NOT NULL AND EXISTS (
             SELECT FROM schedule_fires f WHERE f.schedule_id=s.id AND f.task_id=$1
         ) ORDER BY s.id LIMIT 2 FOR UPDATE",
            &[&task],
        )
        .await?;
    match schedules.as_slice() {
        [] => Ok(None),
        [row] => Ok(Some(row.try_get(0)?)),
        _ => Err(Error::conflict("Task has ambiguous schedule history")),
    }
}

pub(crate) async fn ensure_retry_exclusive(
    tx: &Transaction<'_>,
    schedule: Uuid,
    task: Uuid,
) -> Result<()> {
    let another_active: bool = tx
        .query_one(
            "SELECT EXISTS(SELECT FROM schedule_fires f JOIN tasks t ON t.id=f.task_id
                        WHERE f.schedule_id=$1 AND t.id<>$2
                            AND t.state NOT IN ('succeeded','failed','canceled'))",
            &[&schedule, &task],
        )
        .await?
        .get(0);
    if another_active {
        return Err(Error::conflict(
            "Another run of the schedule is still active",
        ));
    }
    Ok(())
}

// A single pending occurrence whose previous Task settled is eligible even
// when the next cron time is in the future. Task links count conservatively
// irrespective of the recorded outcome on an older fire.
const CANDIDATE: &str = "
    SELECT s.id,s.owner_id,s.spec,s.admission,s.next_fire_at,
           e.subject_id,e.issuer_credential_id,e.maximum_rules,e.expires_at,
           (e.status='active' AND e.run_as='user' AND e.service_account_id IS NULL
              AND e.subject_id=s.owner_id AND e.issuer_credential_id IS NOT NULL
              AND e.expires_at>clock_timestamp()
              AND NOT EXISTS(SELECT FROM calls q WHERE q.delegation_id=e.id)
              AND NOT EXISTS(SELECT FROM tasks t WHERE t.delegation_id=e.id)) AS delegation_live
      FROM schedules s JOIN execution_delegations e ON e.id=s.delegation_id
     WHERE s.admission IS NOT NULL AND s.spec->'enabled'='true'::jsonb
       AND NOT(s.id=ANY($2::uuid[]))
       AND NOT EXISTS (SELECT FROM platform_recovery_state WHERE phase<>'ready')
       AND (CASE WHEN $3::boolean THEN s.next_fire_at<=$1 ELSE
           EXISTS (SELECT FROM schedule_fires f WHERE f.schedule_id=s.id
                   AND f.outcome='pending' AND f.task_id IS NULL)
           AND NOT EXISTS (SELECT FROM schedule_fires f JOIN tasks t ON t.id=f.task_id
                 WHERE f.schedule_id=s.id AND t.state NOT IN ('succeeded','failed','canceled')) END)
     ORDER BY s.next_fire_at NULLS LAST,s.id LIMIT 1
       FOR UPDATE OF s SKIP LOCKED FOR SHARE OF e SKIP LOCKED";

/// Run Schedule reconciliation independently of provider health and runtime
/// reconciliation. The daemon owns this future and its shutdown deadline.
pub async fn run(app: Arc<App>) {
    let stopping = || app.shutdown.load(Ordering::SeqCst) || app.leader.is_closed();
    let blocked = || stopping() || !app.ready.load(Ordering::SeqCst);
    let mut startup_at = None;
    while !stopping() {
        if !blocked() {
            let result = tokio::time::timeout(
                BATCH_TIMEOUT,
                tick(&app.db, &app.auth, &mut startup_at, &blocked),
            )
            .await
            .unwrap_or_else(|_| Err(Error::unavailable("Schedule reconciliation timed out")));
            if let Err(error) = result {
                if !blocked() {
                    tracing::warn!(reason_code = error.code, "Schedule batch deferred");
                }
            }
        }
        tokio::time::sleep(Timeout::from_millis(250)).await;
    }
}

async fn tick(
    db: &Db,
    auth: &Auth,
    startup_at: &mut Option<DateTime<Utc>>,
    blocked: &(impl Fn() -> bool + Sync),
) -> Result<()> {
    let mut client = db.connect_scoped(BATCH_TIMEOUT).await?;
    let through = client
        .query_one("SELECT clock_timestamp()", &[])
        .await?
        .get(0);
    if blocked() {
        return Ok(());
    }
    // The cursor separates a startup outage from normal ticks and remains
    // fixed while startup recovery work drains over many bounded batches.
    let startup_at = *startup_at.get_or_insert(through);
    run_batch(&mut client, auth, startup_at, through, blocked).await
}

async fn run_batch(
    client: &mut tokio_postgres::Client,
    auth: &Auth,
    startup_at: DateTime<Utc>,
    through: DateTime<Utc>,
    stopping: &(impl Fn() -> bool + Sync),
) -> Result<()> {
    let mut considered = Vec::new();
    for round in 0..BATCH_SIZE {
        if stopping() {
            return Err(Error::unavailable("Controller is stopping"));
        }
        let tx = client.transaction().await?;
        if !db::try_auth_lock_shared(&tx).await? {
            break;
        }
        // Give pending work a lane of its own, even when due plans have built
        // up a backlog. A lane with no eligible row lends its turn.
        let due = round % 2 == 0;
        let row = match tx
            .query_opt(CANDIDATE, &[&through, &considered, &due])
            .await?
        {
            Some(row) => Some(row),
            None => {
                tx.query_opt(CANDIDATE, &[&through, &considered, &!due])
                    .await?
            }
        };
        let Some(row) = row else {
            break;
        };
        let schedule = row.try_get("id")?;
        considered.push(schedule);
        match reconcile(&tx, auth, &row, schedule, startup_at.min(through), through).await {
            Ok(()) if !stopping() => tx.commit().await?,
            Ok(()) => {
                tx.rollback().await?;
                return Err(Error::unavailable("Controller is stopping"));
            }
            Err(error) => {
                tx.rollback().await?;
                tracing::warn!(schedule_id=%schedule, reason_code=error.code, "Schedule reconciliation deferred");
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Fire {
    Create,
    Pending,
    Skip,
}

struct Plan {
    next: Option<DateTime<Utc>>,
    fire: Option<(DateTime<Utc>, Fire)>,
}

fn plan(
    spec: &Value,
    cursor: Option<DateTime<Utc>>,
    pending: Option<DateTime<Utc>>,
    active: bool,
    startup_at: DateTime<Utc>,
    through: DateTime<Utc>,
) -> Result<Plan> {
    let cron = CronSchedule::parse(string(spec, "cron")?, string(spec, "timezone")?)?;
    let misfire: MisfirePolicy = string(spec, "misfire")?.parse()?;
    let overlap: OverlapPolicy = string(spec, "overlap")?.parse()?;
    if let Some(next) = cursor.filter(|next| *next <= startup_at) {
        // Recovery uses an exclusive lower bound. next_fire_at is the first
        // unprocessed occurrence and has whole-second cron precision.
        let recovery = cron.plan_recovery(RecoveryInputs {
            after: next - Duration::seconds(1),
            through: startup_at,
            misfire,
            overlap,
            has_active_task: active,
            pending_at: pending,
        })?;
        let fire = match recovery.decision {
            RecoveryDecision::Ready { scheduled_at } => Some((scheduled_at, Fire::Create)),
            RecoveryDecision::Queued { scheduled_at } => Some((scheduled_at, Fire::Pending)),
            RecoveryDecision::Skipped { .. } => {
                recovery.latest_missed_at.map(|at| (at, Fire::Skip))
            }
            RecoveryDecision::Idle => None,
        };
        return Ok(Plan {
            next: recovery.next_fire_at,
            fire,
        });
    }
    if let Some(due) = cursor.filter(|at| *at <= through) {
        let fire = match (active, overlap) {
            (true, OverlapPolicy::Skip) => Fire::Skip,
            (true, OverlapPolicy::QueueOne) => Fire::Pending,
            (false, _) => Fire::Create,
        };
        let next = cron.preview(due, 1)?.fire_at.into_iter().next();
        return Ok(Plan {
            next,
            fire: Some((due, fire)),
        });
    }
    Ok(Plan {
        next: cursor,
        fire: pending.filter(|_| !active).map(|at| (at, Fire::Create)),
    })
}

async fn reconcile(
    tx: &Transaction<'_>,
    auth: &Auth,
    row: &Row,
    schedule: Uuid,
    startup_at: DateTime<Utc>,
    through: DateTime<Utc>,
) -> Result<()> {
    let spec: Value = row.try_get("spec")?;
    let cursor: Option<DateTime<Utc>> = row.try_get("next_fire_at")?;
    let facts = tx.query_one(
        "SELECT (SELECT scheduled_at FROM schedule_fires WHERE schedule_id=$1
                  AND outcome='pending' AND task_id IS NULL ORDER BY scheduled_at DESC LIMIT 1),
                EXISTS(SELECT FROM schedule_fires f JOIN tasks t ON t.id=f.task_id
                        WHERE f.schedule_id=$1 AND t.state NOT IN ('succeeded','failed','canceled'))",
        &[&schedule],
    ).await?;
    let planned = plan(
        &spec,
        cursor,
        facts.try_get(0)?,
        facts.try_get(1)?,
        startup_at,
        through,
    )?;
    if planned.next != cursor {
        tx.execute(
            "UPDATE schedules SET next_fire_at=$2,revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
            &[&schedule, &planned.next],
        ).await?;
    }
    let Some((at, fire)) = planned.fire else {
        return Ok(());
    };
    if matches!(fire, Fire::Skip) {
        tx.execute(
            "INSERT INTO schedule_fires(schedule_id,scheduled_at,outcome)
             VALUES($1,$2,'skipped') ON CONFLICT DO NOTHING",
            &[&schedule, &at],
        )
        .await?;
        return Ok(());
    }

    // Reserve the unique fire identity before creating any Task. A previously
    // created, skipped or paused fire is durable reconciliation evidence.
    tx.execute(
        "INSERT INTO schedule_fires(schedule_id,scheduled_at,outcome)
         VALUES($1,$2,'pending') ON CONFLICT DO NOTHING",
        &[&schedule, &at],
    )
    .await?;
    let pending: bool = tx
        .query_one(
            "SELECT outcome='pending' AND task_id IS NULL FROM schedule_fires
          WHERE schedule_id=$1 AND scheduled_at=$2 FOR UPDATE",
            &[&schedule, &at],
        )
        .await?
        .get(0);
    if !pending {
        return Ok(());
    }
    tx.execute(
        "UPDATE schedule_fires SET outcome='skipped'
          WHERE schedule_id=$1 AND scheduled_at<$2 AND task_id IS NULL AND outcome='pending'",
        &[&schedule, &at],
    )
    .await?;
    if matches!(fire, Fire::Pending) {
        return Ok(());
    }

    tx.batch_execute("SAVEPOINT before_scheduled_task").await?;
    match tasks::create_scheduled(tx, auth, row, &spec).await {
        Ok((task, until, ctx)) => {
            tx.execute(
                "UPDATE schedule_fires SET outcome='created',task_id=$3
                  WHERE schedule_id=$1 AND scheduled_at=$2",
                &[&schedule, &at, &task],
            )
            .await?;
            db::audit(
                tx,
                ctx.subject,
                Some(ctx.credential),
                ctx.request,
                "schedule.fire",
                &Resource::new("schedule", schedule, Some(ctx.subject)).json(),
                "succeeded",
                "TASK_SUBMITTED",
            )
            .await?;
            tx.batch_execute("SET CONSTRAINTS ALL IMMEDIATE").await?;
            let live: bool = tx
                .query_one(
                    "SELECT clock_timestamp()<$1 AND NOT EXISTS
                       (SELECT FROM platform_recovery_state WHERE phase<>'ready')",
                    &[&until],
                )
                .await?
                .get(0);
            if live {
                return Ok(());
            }
        }
        Err(error)
            if matches!(
                error.code,
                "INVALID_CREDENTIAL"
                    | "PERMISSION_DENIED"
                    | "SCHEDULE_WORKFLOW_ARCHIVED"
                    | "TASK_PROVIDER_UNAVAILABLE"
                    | "INVALID_ARGUMENT"
            ) => {}
        Err(error) => return Err(error),
    }
    tx.batch_execute("ROLLBACK TO SAVEPOINT before_scheduled_task")
        .await?;
    tx.execute(
        "UPDATE schedule_fires SET outcome='paused' WHERE schedule_id=$1 AND scheduled_at=$2",
        &[&schedule, &at],
    )
    .await?;
    Ok(())
}
