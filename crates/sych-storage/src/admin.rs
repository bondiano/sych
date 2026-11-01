//! Reads of the administration view: a connection's latest SCM read failure, its webhook
//! deliveries by decision, and the delivery log. Deliveries carry no tenant, so callers pass
//! the connections of theirs.

use sqlx::Row;
use sych_core::api::{Count, Delivery, Page, ScmReadFailure};

use crate::views::page;
use crate::{Storage, StorageError};

#[derive(Debug, Clone, Default)]
pub struct DeliveryFilter {
    pub decision: Option<String>,
    pub before: Option<i64>,
    pub limit: i64,
}

/// The bound form a `LIKE` over connection ids matches; config ids are `[A-Za-z0-9_-]`.
fn set(connections: &[&str]) -> String {
    connections
        .iter()
        .fold(String::from(","), |s, id| format!("{s}{id},"))
}

impl Storage {
    /// The newest `SCM_READ_FAILED` of a connection's changes in the tenant.
    pub async fn last_scm_failure(
        &self,
        tenant_id: &str,
        connection_id: &str,
    ) -> Result<Option<ScmReadFailure>, StorageError> {
        let row = sqlx::query(
            "SELECT e.at, e.change_id, e.detail FROM change_events e
             JOIN changes c ON c.id = e.change_id JOIN repositories r ON r.id = c.repository_id
             WHERE e.kind = 'SCM_READ_FAILED' AND c.tenant_id = $1 AND r.connection_id = $2
             ORDER BY e.id DESC LIMIT 1",
        )
        .bind(tenant_id)
        .bind(connection_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(|r| ScmReadFailure {
            at: r.get(0),
            change_id: r.get(1),
            detail: r.get(2),
        }))
    }

    /// A connection's deliveries received since `since`, counted by decision.
    pub async fn deliveries_by_decision(
        &self,
        connection_id: &str,
        since: i64,
    ) -> Result<Vec<Count>, StorageError> {
        let rows = sqlx::query(
            "SELECT decision, count(*) FROM inbox_events
             WHERE connection_id = $1 AND received_at >= $2 GROUP BY decision ORDER BY decision",
        )
        .bind(connection_id)
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

    /// Deliveries of `connections`, newest first by id.
    pub async fn deliveries(
        &self,
        connections: &[&str],
        f: &DeliveryFilter,
    ) -> Result<Page<Delivery>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, connection_id, delivery_id, event, action, decision, received_at
             FROM inbox_events
             WHERE $1 LIKE ('%,' || connection_id || ',%')
                 AND (CAST($2 AS TEXT) IS NULL OR decision = $2)
                 AND (CAST($3 AS BIGINT) IS NULL OR id < $3)
             ORDER BY id DESC LIMIT $4",
        )
        .bind(set(connections))
        .bind(&f.decision)
        .bind(f.before)
        .bind(f.limit + 1)
        .fetch_all(self.pool())
        .await?;
        let items = rows
            .iter()
            .map(|r| Delivery {
                id: r.get(0),
                connection_id: r.get(1),
                delivery_id: r.get(2),
                event: r.get(3),
                action: r.get(4),
                decision: r.get(5),
                received_at: r.get(6),
            })
            .collect();
        Ok(page(items, f.limit, |d| d.id))
    }
}
