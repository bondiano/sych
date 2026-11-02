//! Analytics behind the dashboard over the repositories a caller may read: how people judged
//! findings and how the findings of finished pull requests settled. Counts only.

use sqlx::Row;
use sqlx::any::AnyRow;
use sych_core::api::{Count, Judged, Quality, SeverityCount};

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

    async fn period_rows(
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
