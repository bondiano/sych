//! Durable model ledger (spec §17.2): reservation and settlement are single transactions, so
//! spent + held reservations + unknown charges never exceed a cap.

use serde::Serialize;
use sqlx::Row;
use sych_core::config::BillingKind;
use sych_core::model::{
    Committed, Denied, Ledger, LedgerError, Limits, Reservation, Settlement, admit,
};
use sych_core::{AttemptId, MicroUsd, Timestamp};

use crate::{Storage, StorageError, int};

const RESERVATION_TOTALS: &str =
    "SELECT
         CAST(coalesce(sum(CASE WHEN run_id = $1 AND purpose = $2
             AND scope IS NOT DISTINCT FROM CAST($8 AS TEXT) THEN 1 ELSE 0 END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN run_id = $1 AND purpose = $2
             AND scope IS NOT DISTINCT FROM CAST($8 AS TEXT)
             THEN CASE state WHEN 'SETTLED' THEN coalesce(charge_micro_usd, 0)
                 WHEN 'RELEASED' THEN 0 ELSE reserved_micro_usd END END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN run_id = $1 AND purpose = $2
             AND scope IS NOT DISTINCT FROM CAST($8 AS TEXT)
             THEN CASE state WHEN 'SETTLED' THEN coalesce(input_tokens, input_tokens_bound)
                 WHEN 'RELEASED' THEN 0 ELSE input_tokens_bound END END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN run_id = $1 AND purpose = $2
             AND scope IS NOT DISTINCT FROM CAST($8 AS TEXT)
             THEN CASE state WHEN 'SETTLED' THEN coalesce(output_tokens, output_tokens_bound, 0)
                 WHEN 'RELEASED' THEN 0 ELSE coalesce(output_tokens_bound, 0) END END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN day = $3 THEN CASE state
             WHEN 'SETTLED' THEN coalesce(charge_micro_usd, 0)
             WHEN 'RELEASED' THEN 0 ELSE reserved_micro_usd END END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN month = $5 THEN CASE state
             WHEN 'SETTLED' THEN coalesce(charge_micro_usd, 0)
             WHEN 'RELEASED' THEN 0 ELSE reserved_micro_usd END END), 0) AS BIGINT),
         CAST(coalesce(max(CASE WHEN month = $5 AND provider = $6 AND billing_kind = $7
             AND billing_mismatch = 1 THEN 1 ELSE 0 END), 0) AS BIGINT)
     FROM model_attempts
     WHERE tenant_id = $4 AND (month = $5 OR (run_id = $1 AND purpose = $2
         AND scope IS NOT DISTINCT FROM CAST($8 AS TEXT)))";

/// The serde name of a config enum, as stored.
fn tag<T: Serialize>(value: &T) -> Result<String, StorageError> {
    Ok(serde_json::to_value(value)?
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn utc(at: Timestamp, format: &str) -> String {
    at.strftime(format).to_string()
}

fn micro(value: i64) -> MicroUsd {
    MicroUsd(u64::try_from(value).unwrap_or_default())
}

fn opt(value: Option<u64>) -> Option<i64> {
    value.map(int)
}

impl Storage {
    pub async fn reserve_attempt(
        &self,
        r: &Reservation,
        limits: &Limits,
    ) -> Result<Result<(), Denied>, StorageError> {
        self.reserve_scoped(r, limits, None).await
    }

    /// [`Self::reserve_attempt`] whose run caps count only the attempts of `scope`, e.g. one
    /// follow-up session; the tenant day and month pools stay shared.
    pub async fn reserve_scoped(
        &self,
        r: &Reservation,
        limits: &Limits,
        scope: Option<&str>,
    ) -> Result<Result<(), Denied>, StorageError> {
        let day = utc(r.at, "%Y-%m-%d");
        let month = utc(r.at, "%Y-%m");
        let billing = tag(&r.route.billing)?;
        let mut tx = self.write().await?;
        // Run caps are lifetime: a released attempt still counts, and a run spans months.
        let row = sqlx::query(RESERVATION_TOTALS)
            .bind(r.run_id.to_string())
            .bind(r.purpose.as_str())
            .bind(&day)
            .bind(&r.tenant_id)
            .bind(&month)
            .bind(&r.route.provider)
            .bind(tag(&BillingKind::SubscriptionQuota)?)
            .bind(scope)
            .fetch_one(&mut *tx)
            .await?;
        let fees = sqlx::query(
            "SELECT CAST(coalesce(sum(allocated_micro_usd), 0) AS BIGINT),
                 CAST(coalesce(max(CASE WHEN provider = $3 THEN 1 ELSE 0 END), 0) AS BIGINT)
             FROM subscription_fees WHERE tenant_id = $1 AND month = $2",
        )
        .bind(&r.tenant_id)
        .bind(&month)
        .bind(&r.route.provider)
        .fetch_one(&mut *tx)
        .await?;
        let mismatch = row.get::<i64, _>(6) != 0;
        if mismatch && r.route.billing == BillingKind::SubscriptionQuota {
            return Ok(Err(Denied::BillingMismatch));
        }

        let fee_recorded = fees.get::<i64, _>(1) != 0;
        let fee_due = r.fixed_fee.filter(|_| !fee_recorded);
        let committed = Committed {
            run_attempts: u32::try_from(row.get::<i64, _>(0)).unwrap_or(u32::MAX),
            run_money: micro(row.get(1)),
            run_input_tokens: u64::try_from(row.get::<i64, _>(2)).unwrap_or_default(),
            run_output_tokens: u64::try_from(row.get::<i64, _>(3)).unwrap_or_default(),
            day_money: micro(row.get(4)),
            month_money: MicroUsd(micro(row.get(5)).0 + micro(fees.get(0)).0),
            fee_due: fee_due.map_or(MicroUsd(0), |f| f.allocated_micro_usd),
        };
        if let Err(denied) = admit(limits, &committed, r) {
            return Ok(Err(denied));
        }

        if let Some(fee) = fee_due {
            sqlx::query(
                "INSERT INTO subscription_fees (tenant_id, provider, month, amount_micro_usd,
                     allocated_micro_usd, recorded_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(&r.tenant_id)
            .bind(&r.route.provider)
            .bind(&month)
            .bind(int(fee.amount_micro_usd.0))
            .bind(int(fee.allocated_micro_usd.0))
            .bind(r.at.as_second())
            .execute(&mut *tx)
            .await?;
        }

        sqlx::query(
            "INSERT INTO model_attempts (id, tenant_id, run_id, purpose, kind, template, profile,
                 provider, requested_model, billing_kind, enforcement, day, month,
                 reserved_micro_usd, input_tokens_bound, output_tokens_bound, state, created_at,
                 scope)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                 'RESERVED', $17, $18)",
        )
        .bind(r.attempt_id.to_string())
        .bind(&r.tenant_id)
        .bind(r.run_id.to_string())
        .bind(r.purpose.as_str())
        .bind(r.kind.as_str())
        .bind(&r.template)
        .bind(&r.route.profile)
        .bind(&r.route.provider)
        .bind(&r.route.model)
        .bind(&billing)
        .bind(tag(&r.route.enforcement)?)
        .bind(&day)
        .bind(&month)
        .bind(int(r.money.0))
        .bind(int(r.input_tokens))
        .bind(opt(r.output_tokens))
        .bind(r.at.as_second())
        .bind(scope)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Ok(()))
    }

    /// Applies a settlement once. A known charge may still reconcile an UNKNOWN attempt; a
    /// charge above the reservation is kept and flagged as a billing mismatch.
    pub async fn settle_attempt(
        &self,
        attempt: AttemptId,
        settlement: &Settlement,
        at: Timestamp,
    ) -> Result<(), StorageError> {
        let id = attempt.to_string();
        let mut tx = self.write().await?;
        let done = match settlement {
            Settlement::Released => {
                sqlx::query(
                    "UPDATE model_attempts SET state = 'RELEASED', settled_at = $1
                     WHERE id = $2 AND state = 'RESERVED'",
                )
                .bind(at.as_second())
                .bind(&id)
                .execute(&mut *tx)
                .await?
            }
            Settlement::Unknown { usage } => {
                sqlx::query(
                    "UPDATE model_attempts SET state = 'UNKNOWN', input_tokens = $1,
                         output_tokens = $2, reasoning_tokens = $3, settled_at = $4
                     WHERE id = $5 AND state = 'RESERVED'",
                )
                .bind(opt(usage.input_tokens))
                .bind(opt(usage.output_tokens))
                .bind(opt(usage.reasoning_tokens))
                .bind(at.as_second())
                .bind(&id)
                .execute(&mut *tx)
                .await?
            }
            Settlement::Settled {
                usage,
                charge,
                reference,
                resolved_model,
                generation_id,
            } => {
                sqlx::query(
                    "UPDATE model_attempts SET state = 'SETTLED', charge_micro_usd = $1,
                         reference_micro_usd = $2, input_tokens = $3, output_tokens = $4,
                         reasoning_tokens = $5, resolved_model = $6, generation_id = $7,
                         billing_mismatch = CASE WHEN coalesce(CAST($1 AS BIGINT), 0) > reserved_micro_usd
                             THEN 1 ELSE 0 END,
                         settled_at = $8
                     WHERE id = $9 AND state IN ('RESERVED', 'UNKNOWN')",
                )
                .bind(opt(charge.map(|c| c.0)))
                .bind(opt(reference.map(|c| c.0)))
                .bind(opt(usage.input_tokens))
                .bind(opt(usage.output_tokens))
                .bind(opt(usage.reasoning_tokens))
                .bind(resolved_model)
                .bind(generation_id)
                .bind(at.as_second())
                .bind(&id)
                .execute(&mut *tx)
                .await?
            }
        };
        if done.rows_affected() == 0 {
            let exists = sqlx::query("SELECT 1 FROM model_attempts WHERE id = $1")
                .bind(&id)
                .fetch_optional(&mut *tx)
                .await?;
            if exists.is_none() {
                return Err(StorageError::Conflict(format!(
                    "unknown model attempt {id}"
                )));
            }
        }

        tx.commit().await?;
        Ok(())
    }
}

impl Ledger for Storage {
    async fn reserve(
        &self,
        reservation: &Reservation,
        limits: &Limits,
    ) -> Result<Result<(), Denied>, LedgerError> {
        self.reserve_attempt(reservation, limits)
            .await
            .map_err(|e| LedgerError(e.to_string()))
    }

    async fn settle(
        &self,
        attempt: AttemptId,
        settlement: &Settlement,
        at: Timestamp,
    ) -> Result<(), LedgerError> {
        self.settle_attempt(attempt, settlement, at)
            .await
            .map_err(|e| LedgerError(e.to_string()))
    }
}
