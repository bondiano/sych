//! Retention of records: operational rows 30 days, final publication operations 90 days, the
//! change event log and settled usage as `storage.retention` says, sessions once expired.
//! Unresolved model charges (`RESERVED`, `UNKNOWN`) and publication outcomes that are not final
//! (`PENDING`, `PUBLISHING`, `UNCERTAIN`) are never removed by age; runs and the audit log are
//! kept.

use sqlx::Row;
use sych_core::Timestamp;
use sych_core::config::Retention;

use crate::{Storage, StorageError};

const DAY: i64 = 86_400;

/// A rule's age in seconds under the configured retention.
type Age = fn(&Retention) -> i64;

const OPERATIONAL: Age = |_| 30 * DAY;
const PUBLICATIONS: Age = |_| 90 * DAY;
const EXPIRED: Age = |_| 0;
const CHANGE_EVENTS: Age = |r| i64::from(r.change_events_days) * DAY;
const MODEL_ATTEMPTS: Age = |r| i64::from(r.model_attempts_days) * DAY;

/// `(table, rule, count, delete, age)`; both statements bind the cutoff once.
const RULES: &[(&str, &str, &str, &str, Age)] = &[
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
        "logged before storage.retention.change_events_days",
        "SELECT count(*) FROM change_events WHERE at < $1",
        "DELETE FROM change_events WHERE at < $1",
        CHANGE_EVENTS,
    ),
    (
        "sessions",
        "expired",
        "SELECT count(*) FROM sessions WHERE expires_at < $1",
        "DELETE FROM sessions WHERE expires_at < $1",
        EXPIRED,
    ),
    (
        "model_attempts",
        "settled or released before storage.retention.model_attempts_days; unknown charges stay",
        "SELECT count(*) FROM model_attempts WHERE created_at < $1 AND state IN ('SETTLED', 'RELEASED')",
        "DELETE FROM model_attempts WHERE created_at < $1 AND state IN ('SETTLED', 'RELEASED')",
        MODEL_ATTEMPTS,
    ),
    (
        "publication_operations",
        "final over 90 days ago; unresolved outcomes stay",
        "SELECT count(*) FROM publication_operations WHERE finished_at < $1
             AND state IN ('DELIVERED', 'FAILED', 'CANCELLED', 'SUPERSEDED')",
        "DELETE FROM publication_operations WHERE finished_at < $1
             AND state IN ('DELIVERED', 'FAILED', 'CANCELLED', 'SUPERSEDED')",
        PUBLICATIONS,
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
        retention: &Retention,
        dry_run: bool,
    ) -> Result<Vec<RecordPrune>, StorageError> {
        let mut out = Vec::new();
        let mut tx = self.write().await?;
        for (table, rule, count, delete, age) in RULES {
            let cutoff = now.as_second() - age(retention);
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
