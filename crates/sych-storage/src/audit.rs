//! The audit log: one row per operator or client action, written in the same transaction as
//! the action, and read back by maintainers of the repository it concerns. Never pruned.

use serde_json::Value;
use sqlx::{Any, Row, Transaction};
use sych_core::api::{AuditEvent, Page};
use uuid::Uuid;

use crate::views::page;
use crate::{Storage, StorageError, Visibility};

/// Who did what to which object; `metadata` carries no tokens, prompts or source.
pub(crate) struct Audit<'a> {
    pub tenant_id: &'a str,
    pub actor: &'a str,
    pub action: &'a str,
    pub target: String,
    pub repository_id: Option<i64>,
    pub metadata: Value,
    pub at: i64,
}

impl Audit<'_> {
    pub(crate) async fn record(&self, tx: &mut Transaction<'_, Any>) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO audit_events (tenant_id, actor, action, target, correlation_id,
                 metadata, at, repository_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(self.tenant_id)
        .bind(self.actor)
        .bind(self.action)
        .bind(&self.target)
        .bind(Uuid::now_v7().to_string())
        .bind(self.metadata.to_string())
        .bind(self.at)
        .bind(self.repository_id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

impl Storage {
    /// Actions on the repositories in `v`, newest first; those of no repository only when
    /// `unscoped`.
    pub async fn audit_events(
        &self,
        v: Visibility<'_>,
        unscoped: bool,
        action: Option<&str>,
        before: Option<i64>,
        limit: i64,
    ) -> Result<Page<AuditEvent>, StorageError> {
        let rows = sqlx::query(concat!(
            "SELECT id, actor, action, target, repository_id, metadata, at FROM audit_events
             WHERE tenant_id = $1 AND ((repository_id IS NOT NULL AND ",
            visible!("repository_id", "$2"),
            ") OR (repository_id IS NULL AND CAST($3 AS BIGINT) = 1))
              AND (CAST($4 AS TEXT) IS NULL OR action = $4)
              AND (CAST($5 AS BIGINT) IS NULL OR id < $5)
             ORDER BY id DESC LIMIT $6"
        ))
        .bind(v.tenant_id)
        .bind(v.set())
        .bind(i64::from(unscoped))
        .bind(action)
        .bind(before)
        .bind(limit + 1)
        .fetch_all(self.pool())
        .await?;
        let events = rows
            .iter()
            .map(|r| {
                Ok(AuditEvent {
                    id: r.get(0),
                    actor: r.get(1),
                    action: r.get(2),
                    target: r.get(3),
                    repository_id: r.get(4),
                    metadata: serde_json::from_str(&r.get::<String, _>(5))?,
                    at: r.get(6),
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        Ok(page(events, limit, |e| e.id))
    }
}
