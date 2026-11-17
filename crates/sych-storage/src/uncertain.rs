//! `UNCERTAIN` operations: the reconciler's due list and what a pass found, and the operator's
//! view of every unresolved publication.

use sqlx::Row;
use sych_core::Timestamp;

use crate::publish::observe;
use crate::{Storage, StorageError};

/// An `UNCERTAIN` operation due for reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncertain {
    pub op_id: i64,
    pub object_id: i64,
    pub kind: String,
    pub change_id: i64,
    pub marker: String,
    pub remote_id: Option<String>,
    pub body: String,
    pub reconcile_deadline: i64,
}

impl Storage {
    /// `UNCERTAIN` operations due for a reconciliation pass.
    pub async fn uncertain_due(&self, now: Timestamp) -> Result<Vec<Uncertain>, StorageError> {
        let rows = sqlx::query(
            "SELECT o.id, o.object_id, p.change_id, p.marker, p.remote_id, coalesce(o.body, ''),
                 o.reconcile_deadline, p.kind
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             WHERE o.state = 'UNCERTAIN' AND o.next_action_at <= $1 ORDER BY o.id",
        )
        .bind(now.as_second())
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Uncertain {
                op_id: r.get(0),
                object_id: r.get(1),
                kind: r.get(7),
                change_id: r.get(2),
                marker: r.get(3),
                remote_id: r.get(4),
                body: r.get(5),
                reconcile_deadline: r.get(6),
            })
            .collect())
    }

    /// Reconciliation found the object: it is delivered after all. An operation another pass
    /// or the operator already resolved stays as it is.
    pub async fn uncertain_found(
        &self,
        u: &Uncertain,
        remote_id: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let Some(row) = sqlx::query(
            "UPDATE publication_operations SET state = 'DELIVERED', remote_id = $1, finished_at = $2
             WHERE id = $3 AND state = 'UNCERTAIN' RETURNING generation",
        )
        .bind(remote_id)
        .bind(at)
        .bind(u.op_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(());
        };
        let generation: i64 = row.get(0);
        observe(&mut tx, u.object_id, remote_id, generation, &u.body, at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Not found this pass: look again later; past the window the operation stays `UNCERTAIN`
    /// for the operator and is not retried by itself.
    pub async fn uncertain_unresolved(
        &self,
        u: &Uncertain,
        next: i64,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let exhausted = now.as_second() >= u.reconcile_deadline;
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE publication_operations SET next_action_at = $1, last_error = $2
             WHERE id = $3 AND state = 'UNCERTAIN'",
        )
        .bind(if exhausted { i64::MAX } else { next })
        .bind(if exhausted {
            "RECONCILIATION_WINDOW_EXHAUSTED: operator decision needed"
        } else {
            "not found yet"
        })
        .bind(u.op_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

/// An unresolved publication as the operator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub op_id: i64,
    pub change_id: i64,
    pub slot: String,
    pub state: String,
    pub attempts: i64,
    pub created_at: i64,
    pub reconcile_deadline: Option<i64>,
    pub last_error: Option<String>,
}

impl Storage {
    /// `PUBLISHING` and `UNCERTAIN` operations, optionally of one change, oldest first.
    pub async fn unresolved_publications(
        &self,
        change_id: Option<i64>,
    ) -> Result<Vec<Unresolved>, StorageError> {
        let rows = sqlx::query(
            "SELECT o.id, p.change_id, p.slot, o.state, o.attempts, o.created_at,
                 o.reconcile_deadline, o.last_error
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             WHERE o.state IN ('PUBLISHING', 'UNCERTAIN')
                 AND (CAST($1 AS BIGINT) IS NULL OR p.change_id = $1)
             ORDER BY o.created_at, o.id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Unresolved {
                op_id: r.get(0),
                change_id: r.get(1),
                slot: r.get(2),
                state: r.get(3),
                attempts: r.get(4),
                created_at: r.get(5),
                reconcile_deadline: r.get(6),
                last_error: r.get(7),
            })
            .collect())
    }
}
