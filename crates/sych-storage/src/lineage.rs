//! Finding lineage, lifecycle history, feedback and waivers (spec §5.6, §11.6, §18.4). One
//! transaction applies a revision's lineage; history rows are only ever appended.

use sqlx::{Row, Sqlite, Transaction};
use sych_core::config::RuleException;
use sych_core::lineage::{Decision, Prior};
use sych_core::{FindingCategory, ReviewResult, Timestamp};

use crate::{Storage, StorageError};

fn category(text: &str) -> FindingCategory {
    match text {
        "RULE_VIOLATION" => FindingCategory::RuleViolation,
        "IMPROVEMENT" => FindingCategory::Improvement,
        "NIT" => FindingCategory::Nit,
        _ => FindingCategory::Defect,
    }
}

fn name(c: FindingCategory) -> &'static str {
    match c {
        FindingCategory::Defect => "DEFECT",
        FindingCategory::RuleViolation => "RULE_VIOLATION",
        FindingCategory::Improvement => "IMPROVEMENT",
        FindingCategory::Nit => "NIT",
    }
}

#[allow(clippy::too_many_arguments)] // one history row
async fn event(
    tx: &mut Transaction<'_, Sqlite>,
    finding: i64,
    run: Option<&str>,
    from: Option<&str>,
    to: &str,
    reason: &str,
    actor: Option<&str>,
    evidence: &[String],
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO finding_events (finding_id, run_id, from_state, to_state, reason, actor,
             evidence, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(finding)
    .bind(run)
    .bind(from)
    .bind(to)
    .bind(reason)
    .bind(actor)
    .bind(serde_json::to_string(evidence)?)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// One finding's lifecycle row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingRow {
    pub id: i64,
    pub fingerprint: String,
    pub state: String,
    pub predecessor_id: Option<i64>,
}

impl Storage {
    /// Open findings of a change, as lineage needs them.
    pub async fn open_findings(&self, change_id: i64) -> Result<Vec<Prior>, StorageError> {
        let rows = sqlx::query(
            "SELECT fingerprint, category, rule_id, path FROM findings
             WHERE change_id = ? AND state = 'OPEN' ORDER BY id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Prior {
                fingerprint: r.get(0),
                category: category(&r.get::<String, _>(1)),
                rule_id: r.get(2),
                path: r.get(3),
            })
            .collect())
    }

    /// Records a revision's findings and the decisions about earlier open ones, once per run.
    pub async fn apply_lineage(
        &self,
        change_id: i64,
        run_id: &str,
        result: &ReviewResult,
        decisions: &[(String, Decision)],
        at: i64,
    ) -> Result<bool, StorageError> {
        let mut tx = self.write().await?;
        let fresh =
            sqlx::query("UPDATE runs SET lineage_applied = 1 WHERE id = ? AND lineage_applied = 0")
                .bind(run_id)
                .execute(&mut *tx)
                .await?;
        if fresh.rows_affected() == 0 {
            return Ok(false);
        }

        record_findings(&mut tx, change_id, run_id, result, at).await?;
        record_decisions(&mut tx, change_id, run_id, decisions, at).await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Completed server runs whose lineage is not applied yet, oldest generation first.
    pub async fn lineage_pending(&self) -> Result<Vec<(i64, String, String)>, StorageError> {
        let rows = sqlx::query(
            "SELECT u.change_id, u.id, u.tenant_id FROM runs u JOIN changes c ON c.id = u.change_id
             WHERE u.state = 'COMPLETED' AND u.lineage_applied = 0 AND c.repository_id IS NOT NULL
             ORDER BY u.change_id, u.generation",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    pub async fn findings_of(&self, change_id: i64) -> Result<Vec<FindingRow>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, fingerprint, state, predecessor_id FROM findings WHERE change_id = ? ORDER BY id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| FindingRow {
                id: r.get(0),
                fingerprint: r.get(1),
                state: r.get(2),
                predecessor_id: r.get(3),
            })
            .collect())
    }

    /// `(to_state, reason, actor)` of a finding's history, oldest first.
    pub async fn finding_history(
        &self,
        finding: i64,
    ) -> Result<Vec<(String, String, Option<String>)>, StorageError> {
        let rows = sqlx::query(
            "SELECT to_state, reason, actor FROM finding_events WHERE finding_id = ? ORDER BY id",
        )
        .bind(finding)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// Structured feedback by an actor the caller authorized. `FALSE_POSITIVE` and `WONT_FIX`
    /// dismiss the finding with that actor and rationale; the technical evidence is untouched.
    #[allow(clippy::too_many_arguments)] // one feedback record
    pub async fn record_feedback(
        &self,
        tenant_id: &str,
        finding: i64,
        actor: &str,
        judgement: &str,
        scope: &str,
        rationale: &str,
        at: i64,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        let row = sqlx::query(
            "SELECT f.change_id, f.state, (SELECT policy_snapshot_id FROM runs WHERE id = f.last_run_id)
             FROM findings f JOIN changes c ON c.id = f.change_id WHERE f.id = ? AND c.tenant_id = ?",
        )
        .bind(finding)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::Conflict(format!("finding {finding} not found")))?;
        let (change_id, state, policy): (i64, String, Option<String>) =
            (row.get(0), row.get(1), row.get(2));
        sqlx::query(
            "INSERT INTO feedback (tenant_id, change_id, finding_id, actor, judgement, scope,
                 rationale, policy_snapshot_id, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(tenant_id)
        .bind(change_id)
        .bind(finding)
        .bind(actor)
        .bind(judgement)
        .bind(scope)
        .bind(rationale)
        .bind(policy)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        if matches!(judgement, "FALSE_POSITIVE" | "WONT_FIX") && state == "OPEN" {
            sqlx::query("UPDATE findings SET state = 'DISMISSED', updated_at = ? WHERE id = ?")
                .bind(at)
                .bind(finding)
                .execute(&mut *tx)
                .await?;
            let reason = format!("{judgement}: {rationale}");
            event(
                &mut tx,
                finding,
                None,
                Some("OPEN"),
                "DISMISSED",
                &reason,
                Some(actor),
                &[],
                at,
            )
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}

/// A scoped waiver as the API records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaiverRecord {
    pub rule_id: String,
    pub exception: RuleException,
}

impl Storage {
    /// Records a waiver for the change's repository; the caller authorized the author.
    #[allow(clippy::too_many_arguments)] // one waiver record
    pub async fn add_waiver(
        &self,
        tenant_id: &str,
        change_id: i64,
        rule_id: &str,
        exception: &RuleException,
        policy_snapshot_id: Option<&str>,
        at: i64,
    ) -> Result<i64, StorageError> {
        let id = sqlx::query_scalar(
            "INSERT INTO waivers (tenant_id, repository_id, rule_id, paths, reason, owner, author,
                 expires_at, until, policy_snapshot_id, created_at)
             SELECT ?, c.repository_id, ?, ?, ?, ?, ?, ?, ?, ?, ? FROM changes c
             WHERE c.id = ? AND c.tenant_id = ? AND c.repository_id IS NOT NULL
             RETURNING id",
        )
        .bind(tenant_id)
        .bind(rule_id)
        .bind(serde_json::to_string(&exception.paths)?)
        .bind(&exception.reason)
        .bind(&exception.owner)
        .bind(&exception.author)
        .bind(exception.expires_at.map(Timestamp::as_second))
        .bind(&exception.until)
        .bind(policy_snapshot_id)
        .bind(at)
        .bind(change_id)
        .bind(tenant_id)
        .fetch_optional(self.pool())
        .await?;
        id.ok_or_else(|| StorageError::Conflict(format!("change {change_id} not found")))
    }

    /// Waivers in force for the change's repository at `at`.
    pub async fn active_waivers(
        &self,
        change_id: i64,
        at: i64,
    ) -> Result<Vec<WaiverRecord>, StorageError> {
        let rows = sqlx::query(
            "SELECT w.rule_id, w.paths, w.reason, w.owner, w.author, w.expires_at, w.until
             FROM waivers w JOIN changes c ON c.repository_id = w.repository_id
                 AND c.tenant_id = w.tenant_id
             WHERE c.id = ? AND w.revoked_at IS NULL AND (w.expires_at IS NULL OR w.expires_at > ?)
             ORDER BY w.id",
        )
        .bind(change_id)
        .bind(at)
        .fetch_all(self.pool())
        .await?;
        rows.iter()
            .map(|r| {
                Ok(WaiverRecord {
                    rule_id: r.get(0),
                    exception: RuleException {
                        paths: serde_json::from_str(&r.get::<String, _>(1))?,
                        reason: r.get(2),
                        owner: r.get(3),
                        author: r.get(4),
                        expires_at: r
                            .get::<Option<i64>, _>(5)
                            .and_then(|s| Timestamp::from_second(s).ok()),
                        until: r.get(6),
                    },
                })
            })
            .collect()
    }
}

impl Storage {
    /// The policy snapshot of the change's latest run that pinned one.
    pub async fn latest_policy(&self, change_id: i64) -> Result<Option<String>, StorageError> {
        Ok(sqlx::query_scalar(
            "SELECT policy_snapshot_id FROM runs WHERE change_id = ? AND policy_snapshot_id IS NOT NULL
             ORDER BY generation DESC LIMIT 1",
        )
        .bind(change_id)
        .fetch_optional(self.pool())
        .await?
        .flatten())
    }
}

/// Every finding of the revision: new ones open with a `found` event, known ones are touched.
async fn record_findings(
    tx: &mut Transaction<'_, Sqlite>,
    change_id: i64,
    run_id: &str,
    result: &ReviewResult,
    at: i64,
) -> Result<(), StorageError> {
    for f in &result.findings {
        let path = f.anchors.first().map(|a| a.path.as_str().to_owned());
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO findings (change_id, fingerprint, finding_id, category, rule_id, path,
                 state, first_run_id, last_run_id, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, 'OPEN', ?, ?, ?)
             ON CONFLICT (change_id, fingerprint) DO UPDATE SET last_run_id = excluded.last_run_id,
                 updated_at = excluded.updated_at
             RETURNING CASE WHEN first_run_id = ? THEN id END",
        )
        .bind(change_id)
        .bind(f.fingerprint.to_string())
        .bind(f.id.to_string())
        .bind(name(f.category))
        .bind(&f.rule_id)
        .bind(path)
        .bind(run_id)
        .bind(run_id)
        .bind(at)
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
        if let Some(id) = inserted {
            let evidence: Vec<String> = f.evidence_ids.iter().map(ToString::to_string).collect();
            event(
                tx,
                id,
                Some(run_id),
                None,
                "OPEN",
                "found",
                None,
                &evidence,
                at,
            )
            .await?;
        }
    }

    Ok(())
}

/// What the lineage decided about earlier open findings, with one event each.
async fn record_decisions(
    tx: &mut Transaction<'_, Sqlite>,
    change_id: i64,
    run_id: &str,
    decisions: &[(String, Decision)],
    at: i64,
) -> Result<(), StorageError> {
    for (fingerprint, decision) in decisions {
        let (to, reason, evidence) = match decision {
            Decision::Matched => continue,
            Decision::Continued { successor } => {
                sqlx::query(
                    "UPDATE findings SET predecessor_id = (SELECT id FROM findings
                         WHERE change_id = ?1 AND fingerprint = ?2)
                     WHERE change_id = ?1 AND fingerprint = ?3",
                )
                .bind(change_id)
                .bind(fingerprint)
                .bind(successor)
                .execute(&mut **tx)
                .await?;
                (
                    "OBSOLETE",
                    format!("continued as {successor} after a rename or move"),
                    Vec::new(),
                )
            }
            Decision::Resolved { basis, evidence } => ("RESOLVED", basis.clone(), evidence.clone()),
            Decision::Obsolete { basis } => ("OBSOLETE", basis.clone(), Vec::new()),
            // Still open: the history notes why it was not closed.
            Decision::Open { reason } => ("OPEN", reason.clone(), Vec::new()),
        };
        let id: Option<i64> = sqlx::query_scalar(
            "UPDATE findings SET state = ?, last_run_id = ?, updated_at = ?
             WHERE change_id = ? AND fingerprint = ? AND state = 'OPEN' RETURNING id",
        )
        .bind(to)
        .bind(run_id)
        .bind(at)
        .bind(change_id)
        .bind(fingerprint)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(id) = id {
            event(
                tx,
                id,
                Some(run_id),
                Some("OPEN"),
                to,
                &reason,
                None,
                &evidence,
                at,
            )
            .await?;
        }
    }
    Ok(())
}
