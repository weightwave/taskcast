use crate::PostgresLongTermStore;
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};
use taskcast_core::{
    cleanup_deadline, BoxError, CleanupBatchResult, CleanupClaim, CleanupTarget,
    DurableWriteContext, StorageFenceConflictError, StoragePreconditionError, Task,
};

const NOW: &str = "FLOOR(EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::BIGINT";
fn bound(value: u64, zero: bool) -> Result<i64, BoxError> {
    if (!zero && value == 0) || value > i32::MAX as u64 {
        return Err(Box::new(StoragePreconditionError::new(
            "Invalid cleanup batch or lease bound",
        )));
    }
    Ok(value as i64)
}
fn target_name(target: &CleanupTarget) -> Result<&'static str, BoxError> {
    match target {
        CleanupTarget::Events => Ok("events"),
        CleanupTarget::All => Ok("all"),
        _ => Err(Box::new(StoragePreconditionError::new(
            "Invalid cleanup target",
        ))),
    }
}
pub(crate) fn next_deadline(task: &Task) -> Option<i64> {
    let events = if task.history_expired_at.is_none() {
        cleanup_deadline(task, CleanupTarget::Events)
    } else {
        None
    };
    [events, cleanup_deadline(task, CleanupTarget::All)]
        .into_iter()
        .flatten()
        .min_by(f64::total_cmp)
        .map(|v| v as i64)
}
pub(crate) async fn guard_write(
    tx: &mut Transaction<'_, Postgres>,
    task_id: &str,
    context: Option<&DurableWriteContext>,
    enrolled: bool,
) -> Result<(), BoxError> {
    let row = sqlx::query("SELECT creation_token, cleanup_policy_version, history_expired_at FROM taskcast_tasks WHERE id = $1 FOR UPDATE").bind(task_id).fetch_optional(&mut **tx).await?;
    let expired = row
        .as_ref()
        .is_some_and(|r| r.get::<Option<i64>, _>("history_expired_at").is_some());
    let requires_context = enrolled
        || row
            .as_ref()
            .is_some_and(|r| r.get::<Option<i32>, _>("cleanup_policy_version") == Some(1));
    let token = row
        .as_ref()
        .and_then(|r| r.get::<Option<String>, _>("creation_token"));
    if expired
        || context.is_some_and(|ctx| token.as_deref() != Some(ctx.creation_token.as_str()))
        || (context.is_none() && requires_context)
    {
        return Err(Box::new(StorageFenceConflictError::new(
            "Durable write belongs to missing, expired, or replaced task generation",
        )));
    }
    Ok(())
}

pub(crate) async fn claim(
    pool: &PgPool,
    limit: u64,
    ttl: u64,
) -> Result<Vec<CleanupClaim>, BoxError> {
    let (limit, ttl) = (bound(limit, false)?, bound(ttl, false)?);
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(&format!("SELECT *, {NOW} AS cleanup_now FROM taskcast_tasks WHERE cleanup_policy_version = 1 AND creation_token IS NOT NULL AND creation_completed_at IS NOT NULL AND status IN ('completed', 'failed', 'cancelled', 'timeout') AND completed_at IS NOT NULL AND cleanup_due_at <= {NOW} AND (cleanup_next_attempt_at IS NULL OR cleanup_next_attempt_at <= {NOW}) AND (cleanup_claim_until IS NULL OR cleanup_claim_until <= {NOW}) ORDER BY cleanup_due_at, id LIMIT $1 FOR UPDATE SKIP LOCKED")).bind(limit).fetch_all(&mut *tx).await?;
    let mut claims = vec![];
    for row in rows {
        let task = PostgresLongTermStore::row_to_task(&row);
        let target = if row.get::<bool, _>("cleanup_in_progress") {
            match row.get::<Option<String>, _>("cleanup_target").as_deref() {
                Some("all") => CleanupTarget::All,
                _ => CleanupTarget::Events,
            }
        } else if cleanup_deadline(&task, CleanupTarget::All)
            .is_some_and(|due| due <= row.get::<i64, _>("cleanup_now") as f64)
        {
            CleanupTarget::All
        } else {
            CleanupTarget::Events
        };
        let token: String = sqlx::query_scalar(&format!("UPDATE taskcast_tasks SET cleanup_claim_token = MD5(id || ':' || clock_timestamp()::TEXT || ':' || random()::TEXT || ':' || txid_current()::TEXT), cleanup_claim_until = {NOW} + $2, cleanup_target = $3 WHERE id = $1 RETURNING cleanup_claim_token")).bind(&task.id).bind(ttl).bind(target_name(&target)?).fetch_one(&mut *tx).await?;
        claims.push(CleanupClaim {
            task_id: task.id,
            creation_token: row.get("creation_token"),
            claim_token: token,
            target,
            completed_at: task.completed_at.unwrap(),
            task_version: row.get::<i64, _>("task_version") as u64,
        });
    }
    tx.commit().await?;
    Ok(claims)
}
pub(crate) async fn renew(pool: &PgPool, claim: &CleanupClaim, ttl: u64) -> Result<bool, BoxError> {
    let ttl = bound(ttl, false)?;
    Ok(sqlx::query(&format!("UPDATE taskcast_tasks SET cleanup_claim_until = {NOW} + $4 WHERE id = $1 AND creation_token = $2 AND cleanup_claim_token = $3 AND cleanup_claim_until > {NOW}")).bind(&claim.task_id).bind(&claim.creation_token).bind(&claim.claim_token).bind(ttl).execute(pool).await?.rows_affected() == 1)
}
pub(crate) async fn defer(pool: &PgPool, claim: &CleanupClaim, delay: u64) -> Result<(), BoxError> {
    let delay = bound(delay, true)?;
    sqlx::query(&format!("UPDATE taskcast_tasks SET cleanup_claim_token = NULL, cleanup_claim_until = NULL, cleanup_next_attempt_at = {NOW} + $4 WHERE id = $1 AND creation_token = $2 AND cleanup_claim_token = $3")).bind(&claim.task_id).bind(&claim.creation_token).bind(&claim.claim_token).bind(delay).execute(pool).await?;
    Ok(())
}
async fn locked(
    tx: &mut Transaction<'_, Postgres>,
    claim: &CleanupClaim,
) -> Result<Option<PgRow>, BoxError> {
    Ok(sqlx::query(&format!("SELECT * FROM taskcast_tasks WHERE id = $1 AND creation_token = $2 AND cleanup_policy_version = 1 AND cleanup_claim_token = $3 AND cleanup_claim_until > {NOW} AND cleanup_target = $4 AND completed_at = $5 AND task_version = $6 AND status IN ('completed','failed','cancelled','timeout') FOR UPDATE")).bind(&claim.task_id).bind(&claim.creation_token).bind(&claim.claim_token).bind(target_name(&claim.target)?).bind(claim.completed_at as i64).bind(claim.task_version as i64).fetch_optional(&mut **tx).await?)
}
async fn settled(tx: &mut Transaction<'_, Postgres>, id: &str) -> Result<bool, BoxError> {
    Ok(sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM taskcast_durable_assignments WHERE task_id = $1) AND NOT EXISTS (SELECT 1 FROM taskcast_terminal_outbox WHERE task_id = $1 AND projected_at IS NULL) AND NOT EXISTS (SELECT 1 FROM taskcast_archive_generations WHERE task_id = $1 AND status = 'uploading')").bind(id).fetch_one(&mut **tx).await?)
}
pub(crate) async fn begin(
    pool: &PgPool,
    claim: &CleanupClaim,
    epoch: u64,
    through: i64,
) -> Result<bool, BoxError> {
    let epoch = bound(epoch, false)?;
    if through < -1 {
        return Err(Box::new(StoragePreconditionError::new(
            "Invalid cleanup watermark",
        )));
    }
    let mut tx = pool.begin().await?;
    let Some(row) = locked(&mut tx, claim).await? else {
        return Ok(false);
    };
    if row.get::<String, _>("storage_state") != "cold"
        || row.get::<i64, _>("storage_epoch") != epoch
        || row
            .get::<Option<String>, _>("active_release_generation")
            .is_some()
        || row.get::<i64, _>("archive_watermark") != through
    {
        return Ok(false);
    }
    let now: i64 = sqlx::query_scalar(&format!("SELECT {NOW}"))
        .fetch_one(&mut *tx)
        .await?;
    if !cleanup_deadline(
        &PostgresLongTermStore::row_to_task(&row),
        claim.target.clone(),
    )
    .is_some_and(|due| due <= now as f64)
        || !settled(&mut tx, &claim.task_id).await?
    {
        return Ok(false);
    }
    let beyond: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM taskcast_events WHERE task_id = $1 AND idx > $2) OR EXISTS (SELECT 1 FROM taskcast_series_state WHERE task_id = $1 AND through_index > $2)").bind(&claim.task_id).bind(through).fetch_one(&mut *tx).await?;
    if beyond {
        return Ok(false);
    }
    sqlx::query(&format!("UPDATE taskcast_tasks SET cleanup_in_progress = true, history_expired_at = COALESCE(history_expired_at, {NOW}), history_expired_through_index = GREATEST(COALESCE(history_expired_through_index,-1), $2) WHERE id = $1")).bind(&claim.task_id).bind(through).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}
pub(crate) async fn delete_batch(
    pool: &PgPool,
    claim: &CleanupClaim,
    limit: u64,
) -> Result<CleanupBatchResult, BoxError> {
    let limit = bound(limit, false)?;
    let mut tx = pool.begin().await?;
    let row = locked(&mut tx, claim).await?;
    if !row.as_ref().is_some_and(|r| {
        r.get::<bool, _>("cleanup_in_progress")
            && r.get::<Option<i64>, _>("history_expired_at").is_some()
            && r.get::<String, _>("storage_state") == "cold"
    }) || !settled(&mut tx, &claim.task_id).await?
    {
        return Err(Box::new(StorageFenceConflictError::new(
            "Cleanup claim or cold-state precondition was lost",
        )));
    }
    let count = sqlx::query("DELETE FROM taskcast_events WHERE id IN (SELECT id FROM taskcast_events WHERE task_id = $1 ORDER BY idx, id LIMIT $2)").bind(&claim.task_id).bind(limit).execute(&mut *tx).await?.rows_affected();
    let remaining: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM taskcast_events WHERE task_id = $1)")
            .bind(&claim.task_id)
            .fetch_one(&mut *tx)
            .await?;
    if remaining {
        tx.commit().await?;
        return Ok(CleanupBatchResult {
            deleted_events: count,
            complete: false,
        });
    }
    for sql in [
        "DELETE FROM taskcast_series_state WHERE (task_id,series_id) IN (SELECT task_id,series_id FROM taskcast_series_state WHERE task_id = $1 LIMIT $2)",
        "DELETE FROM taskcast_archive_batches WHERE (task_id,generation,ordinal) IN (SELECT task_id,generation,ordinal FROM taskcast_archive_batches WHERE task_id = $1 LIMIT $2)",
        "DELETE FROM taskcast_archive_generations g WHERE (task_id,generation) IN (SELECT task_id,generation FROM taskcast_archive_generations WHERE task_id = $1 LIMIT $2) AND NOT EXISTS (SELECT 1 FROM taskcast_archive_batches b WHERE b.task_id = g.task_id AND b.generation = g.generation)",
        "DELETE FROM taskcast_terminal_outbox WHERE projection_id IN (SELECT projection_id FROM taskcast_terminal_outbox WHERE task_id = $1 AND projected_at IS NOT NULL LIMIT $2)",
    ] { sqlx::query(sql).bind(&claim.task_id).bind(limit).execute(&mut *tx).await?; }
    let related: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM taskcast_series_state WHERE task_id = $1) OR EXISTS (SELECT 1 FROM taskcast_archive_generations WHERE task_id = $1) OR EXISTS (SELECT 1 FROM taskcast_terminal_outbox WHERE task_id = $1)").bind(&claim.task_id).fetch_one(&mut *tx).await?;
    if !related {
        if claim.target == CleanupTarget::All {
            sqlx::query("DELETE FROM taskcast_tasks WHERE id = $1")
                .bind(&claim.task_id)
                .execute(&mut *tx)
                .await?;
        } else {
            let next = cleanup_deadline(
                &PostgresLongTermStore::row_to_task(&row.unwrap()),
                CleanupTarget::All,
            )
            .map(|v| v as i64);
            sqlx::query("UPDATE taskcast_tasks SET cleanup_in_progress = false, cleanup_target = NULL, cleanup_due_at = $2, cleanup_next_attempt_at = NULL, cleanup_claim_token = NULL, cleanup_claim_until = NULL WHERE id = $1").bind(&claim.task_id).bind(next).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(CleanupBatchResult {
        deleted_events: count,
        complete: !related,
    })
}

pub(crate) async fn ready(pool: &PgPool, claim: &CleanupClaim) -> Result<bool, BoxError> {
    let mut tx = pool.begin().await?;
    let ready = locked(&mut tx, claim).await?.is_some() && settled(&mut tx, &claim.task_id).await?;
    tx.commit().await?;
    Ok(ready)
}
