//! Cost and latency behind the dashboard analytics over the repositories a caller may read, and
//! the tenant's spend against its budgets. Money is micro-USD; an unknown charge is counted,
//! never added as zero.

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::api::{ChangeCost, CostRow, Spread};

use crate::{Storage, StorageError, Visibility};

// Statements bind `$1` tenant, `$2` readable set, `$3` repository and `$4` the period's start.

/// Attempts, known cost and attempts of unknown cost of the rows grouped, as `usage_days`.
macro_rules! charges {
    () => {
        "count(*),
         CAST(coalesce(sum(CASE WHEN a.state = 'SETTLED'
             THEN coalesce(a.charge_micro_usd, 0) ELSE 0 END), 0) AS BIGINT),
         CAST(coalesce(sum(CASE WHEN a.state = 'UNKNOWN' OR (a.state = 'SETTLED'
             AND a.charge_micro_usd IS NULL AND a.billing_kind <> 'subscription_quota')
             THEN 1 ELSE 0 END), 0) AS BIGINT)"
    };
}

/// Model attempts of the period joined to their run, pull request and repository.
macro_rules! attempts {
    () => {
        concat!(
            " FROM model_attempts a JOIN runs u ON u.id = a.run_id
             JOIN changes c ON c.id = u.change_id JOIN repositories r ON r.id = c.repository_id
             WHERE a.tenant_id = $1",
            scoped!("c.repository_id"),
            " AND a.created_at >= $4"
        )
    };
}

const BY_REPOSITORY: &str = concat!(
    "SELECT r.full_name, ",
    charges!(),
    attempts!(),
    " GROUP BY r.full_name ORDER BY r.full_name"
);

const BY_MODEL: &str = concat!(
    "SELECT coalesce(a.resolved_model, a.requested_model), ",
    charges!(),
    attempts!(),
    " GROUP BY coalesce(a.resolved_model, a.requested_model)
     ORDER BY coalesce(a.resolved_model, a.requested_model)"
);

const BY_PURPOSE: &str = concat!(
    "SELECT a.purpose, ",
    charges!(),
    attempts!(),
    " GROUP BY a.purpose ORDER BY a.purpose"
);

const TOP_CHANGES: &str = concat!(
    "SELECT c.id, r.full_name, c.external_number, ",
    charges!(),
    attempts!(),
    " GROUP BY c.id, r.full_name, c.external_number ORDER BY 5 DESC, c.id DESC LIMIT 10"
);

const SUPERSEDED: &str = concat!(
    "SELECT CAST(coalesce(sum(CASE WHEN u.state = 'SUPERSEDED' AND a.state = 'SETTLED'
         THEN coalesce(a.charge_micro_usd, 0) ELSE 0 END), 0) AS BIGINT)",
    attempts!()
);

const USEFUL_FINDINGS: &str = concat!(
    "SELECT count(DISTINCT b.finding_id) FROM feedback b JOIN changes c ON c.id = b.change_id
     WHERE b.tenant_id = $1 AND b.judgement = 'USEFUL'",
    scoped!("c.repository_id"),
    " AND b.at >= $4"
);

const TO_DELIVERY: &str = concat!(
    "SELECT min(o.finished_at) - u.created_at FROM runs u JOIN changes c ON c.id = u.change_id
     JOIN publication_operations o ON o.run_id = u.id
         AND o.state = 'DELIVERED' AND o.finished_at IS NOT NULL
     WHERE u.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 GROUP BY u.id, u.created_at"
);

const RUN_DURATION: &str = concat!(
    "SELECT u.finished_at - u.created_at FROM runs u JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1 AND u.state = 'COMPLETED' AND u.finished_at IS NOT NULL",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4"
);

const QUEUE_WAIT: &str = concat!(
    "SELECT j.claimed_at - j.created_at FROM jobs j JOIN runs u ON u.id = j.run_id
     JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1 AND j.claimed_at IS NOT NULL",
    scoped!("c.repository_id"),
    " AND j.created_at >= $4"
);

/// One attempt's spend as the ledger counts it before a call: settled charges and held
/// reservations.
macro_rules! spent {
    () => {
        "CASE state WHEN 'SETTLED' THEN coalesce(charge_micro_usd, 0)
             WHEN 'RELEASED' THEN 0 ELSE reserved_micro_usd END"
    };
}

const SPENT: &str = concat!(
    "SELECT CAST(coalesce(sum(CASE WHEN day = $2 THEN ",
    spent!(),
    " END), 0) AS BIGINT), CAST(coalesce(sum(",
    spent!(),
    "), 0) AS BIGINT) FROM model_attempts WHERE tenant_id = $1 AND month = $3"
);

const BUSIEST: &str = concat!(
    "SELECT CAST(coalesce(max(t.today), 0) AS BIGINT), CAST(coalesce(max(t.month), 0) AS BIGINT)
     FROM (SELECT sum(CASE WHEN day = $1 THEN ",
    spent!(),
    " ELSE 0 END) AS today, sum(",
    spent!(),
    ") AS month FROM model_attempts WHERE month = $2 GROUP BY tenant_id) t"
);

fn cost_row(r: &AnyRow) -> CostRow {
    CostRow {
        key: r.get(0),
        attempts: r.get(1),
        cost_known_micro_usd: r.get(2),
        unknown_cost_attempts: r.get(3),
    }
}

/// Cost of the period over readable repositories, without the tenant's budget.
pub struct Spend {
    pub by_repository: Vec<CostRow>,
    pub by_model: Vec<CostRow>,
    pub by_purpose: Vec<CostRow>,
    pub top_changes: Vec<ChangeCost>,
    pub superseded_micro_usd: i64,
    pub useful_findings: i64,
}

impl Storage {
    /// Model cost since `since` over readable repositories.
    pub async fn spend(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Spend, StorageError> {
        let rows = |sql| self.period_rows(sql, v, repository_id, since);
        let scalar = |rows: Vec<AnyRow>| rows.first().map_or(0, |r| r.get(0));
        Ok(Spend {
            by_repository: rows(BY_REPOSITORY).await?.iter().map(cost_row).collect(),
            by_model: rows(BY_MODEL).await?.iter().map(cost_row).collect(),
            by_purpose: rows(BY_PURPOSE).await?.iter().map(cost_row).collect(),
            top_changes: rows(TOP_CHANGES)
                .await?
                .iter()
                .map(|r| ChangeCost {
                    change_id: r.get(0),
                    repository: r.get(1),
                    number: r.get(2),
                    attempts: r.get(3),
                    cost_known_micro_usd: r.get(4),
                    unknown_cost_attempts: r.get(5),
                })
                .collect(),
            superseded_micro_usd: scalar(rows(SUPERSEDED).await?),
            useful_findings: scalar(rows(USEFUL_FINDINGS).await?),
        })
    }

    /// The tenant's spend on UTC `day` (`YYYY-MM-DD`) and over its `month` (`YYYY-MM`).
    pub async fn spent(
        &self,
        tenant_id: &str,
        day: &str,
        month: &str,
    ) -> Result<(i64, i64), StorageError> {
        let row = sqlx::query(SPENT)
            .bind(tenant_id)
            .bind(day)
            .bind(month)
            .fetch_one(self.pool())
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    /// The highest spend of any tenant on UTC `day` and over its `month`, for `/metrics`.
    pub(crate) async fn busiest_spend(
        &self,
        day: &str,
        month: &str,
    ) -> Result<(i64, i64), StorageError> {
        let row = sqlx::query(BUSIEST)
            .bind(day)
            .bind(month)
            .fetch_one(self.pool())
            .await?;
        Ok((row.get(0), row.get(1)))
    }

    /// Time to delivery, run duration and queue wait of the period over readable repositories.
    // shortcut: every duration of the period is read to rank it; fine for a team's volume, a
    // rollup once a period holds hundreds of thousands of runs.
    pub async fn latency(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<[Spread; 3], StorageError> {
        let spread = |rows: Vec<AnyRow>| Spread::of(rows.iter().map(|r| r.get(0)).collect());
        let rows = |sql| self.period_rows(sql, v, repository_id, since);
        Ok([
            spread(rows(TO_DELIVERY).await?),
            spread(rows(RUN_DURATION).await?),
            spread(rows(QUEUE_WAIT).await?),
        ])
    }
}
