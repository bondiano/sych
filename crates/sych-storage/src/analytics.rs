//! Analytics behind the dashboard over the repositories a caller may read: how people judged
//! findings, how the findings of finished pull requests settled, and why reviews stopped short.
//! Counts only. The reasons are copied from a result when its run completes, so they outlive the
//! result's pruning.
//! Playground runs are left out.

use std::collections::BTreeMap;

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::api::{Count, CoverageDay, Honesty, Judged, Quality, SeverityCount};
use sych_core::{Coverage, ExecutionOutcome, ObligationStatus, ReviewResult};

use crate::{Storage, StorageError, Visibility};

// Every statement binds `$1` tenant, `$2` readable set, `$3` repository and `$4` the period's
// start. Nullable columns sort through `coalesce`: backends put NULL at different ends.

// A person's latest judgement of a finding; one they changed later is not counted.
const JUDGEMENTS: &str = concat!(
    "SELECT r.full_name, f.rule_id, f.category, f.severity, b.judgement, count(*)
     FROM feedback b JOIN findings f ON f.id = b.finding_id
     JOIN changes c ON c.id = f.change_id JOIN repositories r ON r.id = c.repository_id
     WHERE b.tenant_id = $1",
    scoped!("c.repository_id"),
    " AND b.at >= $4 AND NOT EXISTS (SELECT 1 FROM feedback n
         WHERE n.finding_id = b.finding_id AND n.actor = b.actor
         AND (n.at > b.at OR (n.at = b.at AND n.id > b.id)))
     GROUP BY r.full_name, f.rule_id, f.category, f.severity, b.judgement
     ORDER BY r.full_name, coalesce(f.rule_id, ''), f.category, coalesce(f.severity, ''),
         b.judgement"
);

const SETTLED: &str = concat!(
    "SELECT f.state, count(*) FROM findings f JOIN changes c ON c.id = f.change_id
     WHERE c.tenant_id = $1 AND c.scm_state IN ('MERGED', 'CLOSED') AND f.state <> 'OBSOLETE'",
    scoped!("c.repository_id"),
    " AND f.updated_at >= $4 GROUP BY f.state ORDER BY f.state"
);

const MERGED_WITH_OPEN: &str = concat!(
    "SELECT f.severity, count(DISTINCT c.id) FROM findings f JOIN changes c ON c.id = f.change_id
     WHERE c.tenant_id = $1 AND c.scm_state = 'MERGED' AND f.state = 'OPEN'",
    scoped!("c.repository_id"),
    " AND f.updated_at >= $4 GROUP BY f.severity ORDER BY coalesce(f.severity, '')"
);

// Runs completed since the table exists have an `OUTCOME` row; the others are counted apart.
const REASONS: &str = concat!(
    "SELECT x.kind, x.reason, CAST(sum(x.occurrences) AS BIGINT) FROM run_reasons x
     JOIN runs u ON u.id = x.run_id JOIN changes c ON c.id = u.change_id WHERE u.tenant_id = $1 AND u.mode = 'REVIEW'",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 GROUP BY x.kind, x.reason ORDER BY x.kind, x.reason"
);

const UNRECORDED: &str = concat!(
    "SELECT count(*) FROM runs u JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1 AND u.mode = 'REVIEW' AND u.coverage IS NOT NULL",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 AND NOT EXISTS (SELECT 1 FROM run_reasons x
         WHERE x.run_id = u.id AND x.kind = 'OUTCOME')"
);

const COVERAGE_BY_DAY: &str = concat!(
    "SELECT CAST(u.created_at / 86400 AS BIGINT) * 86400, count(*),
         CAST(sum(CASE WHEN u.coverage = 'COMPLETE_FOR_SCOPE' THEN 0 ELSE 1 END) AS BIGINT)
     FROM runs u JOIN changes c ON c.id = u.change_id
     WHERE u.tenant_id = $1 AND u.mode = 'REVIEW' AND u.coverage IS NOT NULL",
    scoped!("c.repository_id"),
    " AND u.created_at >= $4 GROUP BY CAST(u.created_at / 86400 AS BIGINT)
     ORDER BY CAST(u.created_at / 86400 AS BIGINT)"
);

/// Coverage as the API spells it.
pub(crate) fn coverage(c: Coverage) -> &'static str {
    match c {
        Coverage::CompleteForScope => "COMPLETE_FOR_SCOPE",
        Coverage::Partial => "PARTIAL",
        Coverage::Unavailable => "UNAVAILABLE",
    }
}

fn outcome(o: ExecutionOutcome) -> &'static str {
    match o {
        ExecutionOutcome::PlanExecuted => "PLAN_EXECUTED",
        ExecutionOutcome::LimitReached => "LIMIT_REACHED",
        ExecutionOutcome::RetriesExhausted => "RETRIES_EXHAUSTED",
        ExecutionOutcome::InputUnavailable => "INPUT_UNAVAILABLE",
    }
}

/// The leading code of a reason, without the detail some reasons carry after it.
fn code(reason: &str) -> &str {
    let end = reason
        .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
        .unwrap_or(reason.len());
    if end == 0 { "OTHER" } else { &reason[..end] }
}

/// The result's outcome, gaps by reason and UNKNOWN obligations by reason code, in the
/// transaction that completes its run.
pub(crate) async fn record_reasons(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    result: &ReviewResult,
) -> Result<(), StorageError> {
    let mut reasons = BTreeMap::from([(("OUTCOME", outcome(result.execution_outcome)), 1_i64)]);
    let gaps = result
        .excluded_or_unreviewed
        .iter()
        .map(|g| ("GAP", code(&g.reason)));
    let unknown = result.obligations.iter().filter_map(|o| match &o.status {
        ObligationStatus::Unknown { reason_code, .. } => Some(("UNKNOWN", code(reason_code))),
        _ => None,
    });
    for key in gaps.chain(unknown) {
        *reasons.entry(key).or_default() += 1;
    }
    for ((kind, reason), occurrences) in reasons {
        sqlx::query(
            "INSERT INTO run_reasons (run_id, kind, reason, occurrences) VALUES ($1, $2, $3, $4)",
        )
        .bind(result.run_id.to_string())
        .bind(kind)
        .bind(reason)
        .bind(occurrences)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

impl Storage {
    /// Judgements and settled findings since `since` over readable repositories.
    pub async fn quality(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Quality, StorageError> {
        let rows = |sql| self.period_rows(sql, v, repository_id, since);
        Ok(Quality {
            since,
            judgements: rows(JUDGEMENTS)
                .await?
                .iter()
                .map(|r| Judged {
                    repository: r.get(0),
                    rule_id: r.get(1),
                    category: r.get(2),
                    severity: r.get(3),
                    judgement: r.get(4),
                    count: r.get(5),
                })
                .collect(),
            settled: rows(SETTLED)
                .await?
                .iter()
                .map(|r| Count {
                    key: r.get(0),
                    count: r.get(1),
                })
                .collect(),
            merged_with_open: rows(MERGED_WITH_OPEN)
                .await?
                .iter()
                .map(|r| SeverityCount {
                    severity: r.get(0),
                    changes: r.get(1),
                })
                .collect(),
        })
    }

    /// Why reviews since `since` stopped short, over readable repositories.
    pub async fn honesty(
        &self,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Honesty, StorageError> {
        let rows = |sql| self.period_rows(sql, v, repository_id, since);
        let reasons = rows(REASONS).await?;
        let of = |kind: &str| {
            reasons
                .iter()
                .filter(|r| r.get::<String, _>(0) == kind)
                .map(|r| Count {
                    key: r.get(1),
                    count: r.get(2),
                })
                .collect()
        };
        Ok(Honesty {
            since,
            outcomes: of("OUTCOME"),
            gaps: of("GAP"),
            unknown: of("UNKNOWN"),
            unrecorded_runs: rows(UNRECORDED).await?.first().map_or(0, |r| r.get(0)),
            coverage_by_day: rows(COVERAGE_BY_DAY)
                .await?
                .iter()
                .map(|r| CoverageDay {
                    day: r.get(0),
                    runs: r.get(1),
                    partial: r.get(2),
                })
                .collect(),
        })
    }

    pub(crate) async fn period_rows(
        &self,
        sql: &'static str,
        v: Visibility<'_>,
        repository_id: Option<i64>,
        since: i64,
    ) -> Result<Vec<AnyRow>, StorageError> {
        Ok(sqlx::query(sql)
            .bind(v.tenant_id)
            .bind(v.set())
            .bind(repository_id)
            .bind(since)
            .fetch_all(self.pool())
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::code;

    #[test]
    fn a_reason_keeps_only_its_leading_code() {
        assert_eq!(code("BLOB_TOO_LARGE: 2 MiB"), "BLOB_TOO_LARGE");
        assert_eq!(code("K8S_MANIFESTS_MISSING"), "K8S_MANIFESTS_MISSING");
        assert_eq!(code("free text"), "OTHER");
    }
}
