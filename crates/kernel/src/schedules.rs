//! Stop future occurrences of a persisted schedule.
use crate::{
    api::etag,
    auth::{self, Context, Resource},
    error::{Error, Result},
};
use axum::http::HeaderMap;
use tokio_postgres::{GenericClient, Transaction};
use uuid::Uuid;

pub(crate) async fn authorize_delete(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    schedule: Uuid,
) -> Result<()> {
    let owner = c
        .query_opt("SELECT owner_id FROM schedules WHERE id=$1", &[&schedule])
        .await?
        .ok_or_else(Error::not_found)?
        .get(0);
    authorize_owner(c, ctx, schedule, owner).await
}

async fn authorize_owner(
    c: &(impl GenericClient + Sync),
    ctx: &Context,
    schedule: Uuid,
    owner: Uuid,
) -> Result<()> {
    let resource = Resource::new("schedule", schedule, Some(owner));
    let result = auth::authorize(c, ctx, "schedule.delete", &resource).await;
    if result
        .as_ref()
        .is_err_and(|error| error.code == "PERMISSION_DENIED")
        && !crate::policy::allows(c, ctx, "schedule.read", &resource).await?
    {
        return Err(Error::not_found());
    }
    result
}

pub(crate) async fn stop(
    tx: &Transaction<'_>,
    ctx: &Context,
    schedule: Uuid,
    headers: &HeaderMap,
) -> Result<()> {
    let row = tx
        .query_opt(
            "SELECT owner_id,revision FROM schedules WHERE id=$1 FOR UPDATE",
            &[&schedule],
        )
        .await?
        .ok_or_else(Error::not_found)?;
    authorize_owner(tx, ctx, schedule, row.get("owner_id")).await?;
    etag(headers, row.get("revision"))?;

    // Serialize plan updates and fire creation on the same parent row. Task
    // links are retained, including legacy rows with a shared delegation.
    let skipped = tx
        .execute(
            "UPDATE schedule_fires SET outcome='skipped'
             WHERE schedule_id=$1 AND task_id IS NULL AND outcome IN ('pending','paused')",
            &[&schedule],
        )
        .await?;
    tx.execute(
        "UPDATE schedules SET spec=jsonb_set(spec,'{enabled}','false'),next_fire_at=NULL,
            revision=revision+1,updated_at=clock_timestamp()
         WHERE id=$1 AND ($2 OR next_fire_at IS NOT NULL
                          OR spec->'enabled' IS DISTINCT FROM 'false'::jsonb)",
        &[&schedule, &(skipped != 0)],
    )
    .await?;
    Ok(())
}
