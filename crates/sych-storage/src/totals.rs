//! Period totals and usage behind the dashboard overview, over the repositories a caller may
//! read. Money is micro-USD; an unknown charge is counted, never added as zero.

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::api::{Count, SubscriptionFee, Summary, UsageDay};

use crate::{Storage, StorageError, Visibility};

// Every statement binds `$1` tenant, `$2` readable set, `$3` repository and, over a period,
// `$4` its start.

const RUNS_BY_STATE: &str = concat!(
    "SELECT u.state, count(*) FROM runs u JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 GROUP BY u.state ORDER BY u.state"
);

const RUNS_BY_COVERAGE: &str = concat!(
    "SELECT u.coverage, count(*) FROM runs u JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1 AND u.coverage IS NOT NULL",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 GROUP BY u.coverage ORDER BY u.coverage"
);

const FINDING_EVENTS: &str = concat!(
    "SELECT e.to_state, count(*) FROM finding_events e
     JOIN findings f ON f.id = e.finding_id JOIN changes c ON c.id = f.change_id
     WHERE c.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND e.at >= $4 GROUP BY e.to_state ORDER BY e.to_state"
);

const UNKNOWN_OBLIGATIONS: &str = concat!(
    "SELECT CAST(coalesce(sum(u.unknown_obligations), 0) AS BIGINT)
     FROM runs u JOIN changes c ON c.id = u.change_id WHERE u.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4"
);

// Unknown cost as the result's usage counts it: neither a settled charge, a covered
// subscription call, nor a released reservation; in-flight reservations are apart.
const COST: &str = concat!(
    "SELECT CAST(coalesce(sum(CASE WHEN a.state = 'SETTLED'
         THEN coalesce(a.charge_micro_usd, 0) ELSE 0 END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN a.state = 'UNKNOWN' OR (a.state = 'SETTLED'
             AND a.charge_micro_usd IS NULL AND a.billing_kind <> 'subscription_quota')
             THEN 1 ELSE 0 END), 0) AS BIGINT)
     FROM model_attempts a JOIN runs u ON u.id = a.run_id
     JOIN changes c ON c.id = u.change_id WHERE a.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND a.created_at >= $4"
);

const CURRENT: &str = concat!(
    "SELECT
         (SELECT CAST(coalesce(sum(a.reserved_micro_usd), 0) AS BIGINT)
          FROM model_attempts a JOIN runs u ON u.id = a.run_id JOIN changes c ON c.id = u.change_id
          WHERE a.tenant_id = $1 AND a.state = 'RESERVED'",
    scoped!("c.repository_id"),
    "),
         (SELECT count(*) FROM jobs j JOIN runs u ON u.id = j.run_id
          JOIN changes c ON c.id = u.change_id WHERE u.tenant_id = $1 AND j.state = 'QUEUED'",
    scoped!("c.repository_id"),
    "),
         (SELECT min(j.next_attempt_at) FROM jobs j JOIN runs u ON u.id = j.run_id
          JOIN changes c ON c.id = u.change_id WHERE u.tenant_id = $1 AND j.state = 'QUEUED'",
    scoped!("c.repository_id"),
    "),
         (SELECT count(*) FROM publication_operations o JOIN runs u ON u.id = o.run_id
          JOIN changes c ON c.id = u.change_id WHERE u.tenant_id = $1 AND o.state = 'UNCERTAIN'",
    scoped!("c.repository_id"),
    ")"
);

impl Storage {
    /// Totals since `since` over readable repositories; the queue is current, not historical.
    pub async fn summary(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
        now: i64,
    ) -> Result<Summary, StorageError> {
        let counts = |sql| self.counts(sql, v, repository_id, since);
        let period = |sql| self.row(sql, v, repository_id, Some(since));
        let cost = period(COST).await?;
        let current = self.row(CURRENT, v, repository_id, None).await?;
        let oldest: Option<i64> = current.get(2);
        Ok(Summary {
            since,
            runs_by_state: counts(RUNS_BY_STATE).await?,
            runs_by_coverage: counts(RUNS_BY_COVERAGE).await?,
            unknown_obligations: period(UNKNOWN_OBLIGATIONS).await?.get(0),
            finding_events: counts(FINDING_EVENTS).await?,
            cost_known_micro_usd: cost.get(0),
            unknown_cost_attempts: cost.get(1),
            reserved_micro_usd: current.get(0),
            queued_jobs: current.get(1),
            oldest_queued_age_seconds: oldest.map_or(0, |t| (now - t).max(0)),
            uncertain_publications: current.get(3),
        })
    }

    async fn counts(
        &self,
        sql: &'static str,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Vec<Count>, StorageError> {
        let rows = sqlx::query(sql)
            .bind(v.tenant_id)
            .bind(v.set())
            .bind(repository_id)
            .bind(since)
            .fetch_all(self.pool())
            .await?;
        Ok(rows
            .iter()
            .map(|r| Count {
                key: r.get(0),
                count: r.get(1),
            })
            .collect())
    }

    async fn row(
        &self,
        sql: &'static str,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: Option<i64>,
    ) -> Result<AnyRow, StorageError> {
        let q = sqlx::query(sql)
            .bind(v.tenant_id)
            .bind(v.set())
            .bind(repository_id);
        let q = match since {
            Some(since) => q.bind(since),
            None => q,
        };
        Ok(q.fetch_one(self.pool()).await?)
    }

    /// Model usage by UTC day, `from` to `to` inclusive (`YYYY-MM-DD`).
    pub async fn usage_days(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        from: &str,
        to: &str,
    ) -> Result<Vec<UsageDay>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT a.day, count(*),
                 CAST(coalesce(sum(CASE WHEN a.state = 'SETTLED'
                     THEN coalesce(a.charge_micro_usd, 0) ELSE 0 END), 0) AS BIGINT),
                 CAST(coalesce(sum(CASE WHEN a.state = 'UNKNOWN' OR (a.state = 'SETTLED'
                     AND a.charge_micro_usd IS NULL AND a.billing_kind <> 'subscription_quota')
                     THEN 1 ELSE 0 END), 0) AS BIGINT),
                 CAST(coalesce(sum(a.input_tokens), 0) AS BIGINT),
                 CAST(coalesce(sum(a.output_tokens), 0) AS BIGINT)
             FROM model_attempts a JOIN runs u ON u.id = a.run_id
             JOIN changes c ON c.id = u.change_id WHERE a.tenant_id = $1",
            scoped!("c.repository_id"),
            " AND a.day >= $4 AND a.day <= $5 GROUP BY a.day ORDER BY a.day"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(repository_id)
        .bind(from)
        .bind(to)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| UsageDay {
                day: r.get(0),
                attempts: r.get(1),
                cost_known_micro_usd: r.get(2),
                unknown_cost_attempts: r.get(3),
                input_tokens: r.get(4),
                output_tokens: r.get(5),
            })
            .collect())
    }

    /// The tenant's subscription fees for months `from` to `to` inclusive (`YYYY-MM`).
    pub async fn subscription_fees(
        &self,
        tenant_id: &str,
        from: &str,
        to: &str,
    ) -> Result<Vec<SubscriptionFee>, StorageError> {
        let rows = sqlx::query(
            "SELECT provider, month, amount_micro_usd FROM subscription_fees
             WHERE tenant_id = $1 AND month >= $2 AND month <= $3 ORDER BY month, provider",
        )
        .bind(tenant_id)
        .bind(from)
        .bind(to)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| SubscriptionFee {
                provider: r.get(0),
                month: r.get(1),
                amount_micro_usd: r.get(2),
            })
            .collect())
    }
}
