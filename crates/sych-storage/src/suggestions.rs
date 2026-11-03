//! What rule suggestions are made of: each person's latest judgement of a repository's rule
//! findings, and the decisions maintainers recorded about the suggestions.

use sqlx::Row;
use sych_core::suggest::JudgedFinding;

use crate::audit::Audit;
use crate::{Storage, StorageError};

/// A maintainer's decision about one suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub fingerprint: String,
    pub state: String,
    pub pull_request: Option<String>,
}

impl Storage {
    /// Latest judgements since `since` of the findings of rules in one repository.
    pub async fn judged_findings(
        &self,
        tenant_id: &str,
        repository_id: i64,
        since: i64,
    ) -> Result<Vec<JudgedFinding>, StorageError> {
        let rows = sqlx::query(
            "SELECT f.id, f.rule_id, f.path, b.judgement FROM feedback b
             JOIN findings f ON f.id = b.finding_id JOIN changes c ON c.id = f.change_id
             WHERE b.tenant_id = $1 AND c.repository_id = $2 AND f.rule_id IS NOT NULL
                 AND b.at >= $3 AND NOT EXISTS (SELECT 1 FROM feedback n
                     WHERE n.finding_id = b.finding_id AND n.actor = b.actor
                     AND (n.at > b.at OR (n.at = b.at AND n.id > b.id)))
             ORDER BY f.id, b.id",
        )
        .bind(tenant_id)
        .bind(repository_id)
        .bind(since)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| JudgedFinding {
                finding_id: r.get(0),
                rule_id: r.get(1),
                path: r.get(2),
                judgement: r.get(3),
            })
            .collect())
    }

    /// Decisions recorded for one repository's suggestions.
    pub async fn suggestion_decisions(
        &self,
        tenant_id: &str,
        repository_id: i64,
    ) -> Result<Vec<Decision>, StorageError> {
        let rows = sqlx::query(
            "SELECT fingerprint, state, pull_request FROM suggestion_decisions
             WHERE tenant_id = $1 AND repository_id = $2",
        )
        .bind(tenant_id)
        .bind(repository_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Decision {
                fingerprint: r.get(0),
                state: r.get(1),
                pull_request: r.get(2),
            })
            .collect())
    }

    /// Records `decision` for a suggestion of `repository_id`, replacing an earlier one, with
    /// an audit row in the same transaction.
    pub async fn decide_suggestion(
        &self,
        tenant_id: &str,
        repository_id: i64,
        decision: &Decision,
        actor: &str,
        at: i64,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "INSERT INTO suggestion_decisions
                 (tenant_id, repository_id, fingerprint, state, actor, pull_request, at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (tenant_id, repository_id, fingerprint) DO UPDATE SET
                 state = excluded.state, actor = excluded.actor,
                 pull_request = excluded.pull_request, at = excluded.at",
        )
        .bind(tenant_id)
        .bind(repository_id)
        .bind(&decision.fingerprint)
        .bind(&decision.state)
        .bind(actor)
        .bind(&decision.pull_request)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        Audit {
            tenant_id,
            actor,
            action: if decision.state == "DONE" {
                "SUGGESTION_DONE"
            } else {
                "SUGGESTION_DISMISSED"
            },
            target: format!("suggestion:{}", decision.fingerprint),
            repository_id: Some(repository_id),
            metadata: serde_json::json!({"pull_request": decision.pull_request}),
            at,
        }
        .record(&mut tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
