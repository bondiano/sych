//! Looking up saved results and dismissed findings across revisions of a change.

use sqlx::Row;
use sych_core::publication::DismissedFinding;
use sych_core::{ResultId, ReviewResult, RunId};

use crate::lineage::category;
use crate::{Storage, StorageError};

impl Storage {
    /// The run that saved `result` for this tenant, if any.
    pub async fn run_of_result(
        &self,
        tenant_id: &str,
        result: ResultId,
    ) -> Result<Option<RunId>, StorageError> {
        let row = sqlx::query("SELECT id FROM runs WHERE result_id = $1 AND tenant_id = $2")
            .bind(result.to_string())
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| {
            let id: String = r.get(0);
            id.parse()
                .map(RunId)
                .map_err(|_| StorageError::Corrupt(format!("run id {id}")))
        })
        .transpose()
    }

    /// The newest completed review result of `change_id`, if any.
    pub async fn latest_completed_result(
        &self,
        tenant_id: &str,
        change_id: i64,
    ) -> Result<Option<ReviewResult>, StorageError> {
        let row = sqlx::query(
            "SELECT id FROM runs
             WHERE change_id = $1 AND tenant_id = $2 AND mode = 'REVIEW' AND state = 'COMPLETED'
             ORDER BY generation DESC LIMIT 1",
        )
        .bind(change_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = row.get(0);
        let run = id
            .parse()
            .map(RunId)
            .map_err(|_| StorageError::Corrupt(format!("run id {id}")))?;
        self.load_result(tenant_id, run).await.map(Some)
    }

    /// Dismissed findings of `change_id`, each with its primary anchor span and latest dismissal
    /// rationale from `feedback`.
    pub async fn dismissed_findings(
        &self,
        change_id: i64,
    ) -> Result<Vec<DismissedFinding>, StorageError> {
        let rows = sqlx::query(
            "SELECT f.fingerprint, f.category, f.rule_id, f.path, f.start_line, f.end_line,
                    f.title,
                    (SELECT fb.rationale FROM feedback fb
                     WHERE fb.finding_id = f.id
                       AND fb.judgement IN ('FALSE_POSITIVE', 'WONT_FIX')
                     ORDER BY fb.id DESC LIMIT 1)
             FROM findings f
             WHERE f.change_id = $1 AND f.state = 'DISMISSED'
             ORDER BY f.id",
        )
        .bind(change_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|r| DismissedFinding {
                fingerprint: r.get(0),
                category: category(&r.get::<String, _>(1)),
                rule_id: r.get(2),
                path: r.get(3),
                start_line: r
                    .get::<Option<i64>, _>(4)
                    .and_then(|n| u32::try_from(n).ok()),
                end_line: r
                    .get::<Option<i64>, _>(5)
                    .and_then(|n| u32::try_from(n).ok()),
                title: r.get(6),
                rationale: r.get(7),
            })
            .collect())
    }
}
