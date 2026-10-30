//! Retention of records (spec §20.4): operational rows 30 days, audit and usage 90 days.
//! Unresolved model charges (`RESERVED`, `UNKNOWN`) and publication outcomes that are not final
//! (`PENDING`, `PUBLISHING`, `UNCERTAIN`) are never removed by age; runs are kept.

use sqlx::Row;
use sych_core::Timestamp;

use crate::{Storage, StorageError};

const DAY: i64 = 86_400;
const OPERATIONAL: i64 = 30 * DAY;
const AUDIT: i64 = 90 * DAY;

/// `(table, rule, count, delete)`; both statements bind the cutoff once.
const RULES: &[(&str, &str, &str, &str, i64)] = &[
    (
        "change_intents",
        "consumed over 30 days ago",
        "SELECT count(*) FROM change_intents WHERE consumed_at < $1",
        "DELETE FROM change_intents WHERE consumed_at < $1",
        OPERATIONAL,
    ),
    (
        "inbox_events",
        "received over 30 days ago, no kept intent refers to it",
        "SELECT count(*) FROM inbox_events i WHERE received_at < $1 AND NOT EXISTS
             (SELECT 1 FROM change_intents c WHERE coalesce(c.consumed_at, $1) >= $1
                 AND i.id IN (c.first_inbox_id, c.last_inbox_id))",
        "DELETE FROM inbox_events WHERE received_at < $1 AND NOT EXISTS
             (SELECT 1 FROM change_intents c WHERE coalesce(c.consumed_at, $1) >= $1
                 AND inbox_events.id IN (c.first_inbox_id, c.last_inbox_id))",
        OPERATIONAL,
    ),
    (
        "jobs",
        "finished over 30 days ago",
        "SELECT count(*) FROM jobs WHERE finished_at < $1 AND state IN ('DONE', 'FAILED', 'CANCELLED')",
        "DELETE FROM jobs WHERE finished_at < $1 AND state IN ('DONE', 'FAILED', 'CANCELLED')",
        OPERATIONAL,
    ),
    (
        "change_events",
        "logged over 90 days ago",
        "SELECT count(*) FROM change_events WHERE at < $1",
        "DELETE FROM change_events WHERE at < $1",
        AUDIT,
    ),
    (
        "audit_events",
        "logged over 90 days ago",
        "SELECT count(*) FROM audit_events WHERE at < $1",
        "DELETE FROM audit_events WHERE at < $1",
        AUDIT,
    ),
    (
        "model_attempts",
        "settled or released over 90 days ago; unknown charges stay",
        "SELECT count(*) FROM model_attempts WHERE created_at < $1 AND state IN ('SETTLED', 'RELEASED')",
        "DELETE FROM model_attempts WHERE created_at < $1 AND state IN ('SETTLED', 'RELEASED')",
        AUDIT,
    ),
    (
        "publication_operations",
        "final over 90 days ago; unresolved outcomes stay",
        "SELECT count(*) FROM publication_operations WHERE finished_at < $1
             AND state IN ('DELIVERED', 'FAILED', 'CANCELLED', 'SUPERSEDED')",
        "DELETE FROM publication_operations WHERE finished_at < $1
             AND state IN ('DELIVERED', 'FAILED', 'CANCELLED', 'SUPERSEDED')",
        AUDIT,
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordPrune {
    pub table: &'static str,
    pub rule: &'static str,
    pub rows: i64,
}

impl Storage {
    /// Counts, and unless `dry_run` deletes, the records past their retention at `now`.
    pub async fn prune_records(
        &self,
        now: Timestamp,
        dry_run: bool,
    ) -> Result<Vec<RecordPrune>, StorageError> {
        let mut out = Vec::new();
        let mut tx = self.write().await?;
        for (table, rule, count, delete, age) in RULES {
            let cutoff = now.as_second() - age;
            let rows: i64 = sqlx::query(*count)
                .bind(cutoff)
                .fetch_one(&mut *tx)
                .await?
                .get(0);
            if !dry_run && rows > 0 {
                sqlx::query(*delete).bind(cutoff).execute(&mut *tx).await?;
            }

            out.push(RecordPrune { table, rule, rows });
        }
        tx.commit().await?;
        Ok(out)
    }

    /// `(connection, owner/name)` of repositories with an active run; their mirrors stay.
    pub async fn active_repositories(&self) -> Result<Vec<(String, String)>, StorageError> {
        let rows = sqlx::query(
            "SELECT DISTINCT r.connection_id, r.full_name FROM runs u
             JOIN changes c ON c.id = u.change_id JOIN repositories r ON r.id = c.repository_id
             WHERE u.state IN ('QUEUED', 'WAITING_EVIDENCE', 'RUNNING')",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }
}
