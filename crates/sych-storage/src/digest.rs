//! Weekly digest deliveries: a row claimed before each send makes one worker of many send a
//! digest once per slot and channel. A send that never reported back stays `SENDING`, an unknown
//! outcome that is not sent again; a failed one is retried after a pause.

use sqlx::Row;
use sych_core::api::DigestDelivery;

use crate::{Storage, StorageError};

/// Sends of one slot and channel at most.
pub const MAX_DIGEST_ATTEMPTS: i64 = 3;
/// Pause before a failed send is tried again.
pub const DIGEST_RETRY_SECONDS: i64 = 300;
/// A send still `SENDING` after this long is reported as undelivered.
const SENDING_STALE_SECONDS: i64 = 3600;

impl Storage {
    /// Whether this worker sends the digest of `week_start` to `channel` now: the slot is new,
    /// or its last send failed a while ago with attempts left.
    pub async fn claim_digest(
        &self,
        tenant_id: &str,
        week_start: i64,
        channel: &str,
        now: i64,
    ) -> Result<bool, StorageError> {
        let mut tx = self.write().await?;
        let inserted = sqlx::query(
            "INSERT INTO digest_deliveries
                 (tenant_id, week_start, channel, state, attempts, error, updated_at)
             VALUES ($1, $2, $3, 'SENDING', 1, NULL, $4)
             ON CONFLICT (tenant_id, week_start, channel) DO NOTHING",
        )
        .bind(tenant_id)
        .bind(week_start)
        .bind(channel)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let claimed = inserted == 1
            || sqlx::query(
                "UPDATE digest_deliveries SET state = 'SENDING', attempts = attempts + 1,
                     updated_at = $4
                 WHERE tenant_id = $1 AND week_start = $2 AND channel = $3 AND state = 'FAILED'
                   AND attempts < $5 AND updated_at <= $6",
            )
            .bind(tenant_id)
            .bind(week_start)
            .bind(channel)
            .bind(now)
            .bind(MAX_DIGEST_ATTEMPTS)
            .bind(now - DIGEST_RETRY_SECONDS)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
        tx.commit().await?;
        Ok(claimed)
    }

    /// The outcome of a claimed send; `error` is redacted text or `None` when delivered.
    pub async fn finish_digest(
        &self,
        tenant_id: &str,
        week_start: i64,
        channel: &str,
        error: Option<&str>,
        now: i64,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE digest_deliveries SET state = $4, error = $5, updated_at = $6
             WHERE tenant_id = $1 AND week_start = $2 AND channel = $3 AND state = 'SENDING'",
        )
        .bind(tenant_id)
        .bind(week_start)
        .bind(channel)
        .bind(if error.is_some() {
            "FAILED"
        } else {
            "DELIVERED"
        })
        .bind(error)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The tenant's deliveries, newest slot first.
    pub async fn digest_deliveries(
        &self,
        tenant_id: &str,
        limit: i64,
    ) -> Result<Vec<DigestDelivery>, StorageError> {
        let rows = sqlx::query(
            "SELECT week_start, channel, state, attempts, error, updated_at
             FROM digest_deliveries WHERE tenant_id = $1
             ORDER BY week_start DESC, channel LIMIT $2",
        )
        .bind(tenant_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| DigestDelivery {
                week_start: r.get("week_start"),
                channel: r.get("channel"),
                state: r.get("state"),
                attempts: r.get("attempts"),
                error: r.get("error"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// Sends of the last eight days that failed for good or never reported back.
    pub(crate) async fn undelivered_digests(&self, now: i64) -> Result<i64, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM digest_deliveries WHERE week_start >= $1
               AND ((state = 'FAILED' AND attempts >= $2) OR (state = 'SENDING' AND updated_at <= $3))",
        )
        .bind(now - 8 * 86_400)
        .bind(MAX_DIGEST_ATTEMPTS)
        .bind(now - SENDING_STALE_SECONDS)
        .fetch_one(self.pool())
        .await?)
    }
}
