//! The publication outbox (spec §18.3–18.6, §20.2). Before any network write an operation is
//! `PUBLISHING` with its body, request digest, attempt id and fence; the reply and the observed
//! remote id commit together. A lost reply, a crash or a lost lease is `UNCERTAIN` — never
//! `PENDING` — and only reconciliation or an audited operator decision moves it on.

use sqlx::Row;
use sych_core::{Timestamp, sha256};
use uuid::Uuid;

use crate::{Partition, Storage, StorageError};

/// Attempts of one revision whose refusal was definite (rate limit, gate retry).
pub const MAX_PUBLISH_ATTEMPTS: i64 = 5;

/// A claimed operation with what the publisher needs to render and send it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishClaim {
    pub op_id: i64,
    pub object_id: i64,
    /// `SUMMARY`, `INLINE` or `CHECK`.
    pub kind: String,
    pub fingerprint: Option<String>,
    /// The planned provider anchor of an inline comment, as JSON.
    pub anchor: Option<String>,
    pub change_id: i64,
    pub partition: Partition,
    pub slot: String,
    pub marker: String,
    pub remote_id: Option<String>,
    pub run_id: String,
    pub result_id: String,
    pub generation: i64,
    pub attempts: i64,
    pub owner: String,
    pub epoch: i64,
}

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
    /// Claims the oldest due `PENDING` operation whose object has nothing in flight.
    pub async fn claim_publication(
        &self,
        owner: &str,
        lease_seconds: i64,
        now: Timestamp,
    ) -> Result<Option<PublishClaim>, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let id: Option<i64> = sqlx::query_scalar(
            "UPDATE publication_operations SET owner = ?, lease_epoch = lease_epoch + 1,
                 lease_until = ?
             WHERE id = (SELECT o.id FROM publication_operations o
                 WHERE o.state = 'PENDING' AND o.next_action_at <= ?
                 AND NOT EXISTS (SELECT 1 FROM publication_operations b
                     WHERE b.object_id = o.object_id AND b.state IN ('PUBLISHING', 'UNCERTAIN'))
                 ORDER BY o.next_action_at, o.id LIMIT 1)
             RETURNING id",
        )
        .bind(owner)
        .bind(at + lease_seconds)
        .bind(at)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(id) = id else { return Ok(None) };
        let r = sqlx::query(
            "SELECT o.id, o.object_id, p.change_id, c.tenant_id, c.access_scope, p.slot, p.marker,
                 p.remote_id, o.run_id, o.result_id, o.generation, o.attempts, o.owner, o.lease_epoch,
                 p.kind, p.fingerprint, p.anchor
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             JOIN changes c ON c.id = p.change_id WHERE o.id = ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(PublishClaim {
            op_id: r.get(0),
            object_id: r.get(1),
            kind: r.get(14),
            fingerprint: r.get(15),
            anchor: r.get(16),
            change_id: r.get(2),
            partition: Partition {
                tenant_id: r.get(3),
                access_scope: r.get(4),
            },
            slot: r.get(5),
            marker: r.get(6),
            remote_id: r.get(7),
            run_id: r.get(8),
            result_id: r.get(9),
            generation: r.get(10),
            attempts: r.get(11),
            owner: r.get(12),
            epoch: r.get(13),
        }))
    }

    /// The local gate before a write: the operation is still the change's desired generation
    /// and the change is open as last read from the API. `Err` names why it is not.
    pub async fn publication_gate(
        &self,
        c: &PublishClaim,
    ) -> Result<Result<(), String>, StorageError> {
        let r = sqlx::query(
            "SELECT c.desired_generation, coalesce(c.scm_state, 'OPEN') FROM changes c WHERE c.id = ?",
        )
        .bind(c.change_id)
        .fetch_one(self.pool())
        .await?;
        let (desired, scm): (i64, String) = (r.get(0), r.get(1));
        Ok(if desired != c.generation {
            Err("SUPERSEDED".into())
        } else if !matches!(scm.as_str(), "OPEN" | "DRAFT") {
            Err(scm)
        } else {
            Ok(())
        })
    }

    /// `PENDING` → `PUBLISHING` with the exact body, before the network write; fenced.
    /// Returns the attempt id, or `None` when the claim is stale.
    pub async fn begin_publishing(
        &self,
        c: &PublishClaim,
        body: &str,
        now: Timestamp,
    ) -> Result<Option<String>, StorageError> {
        let attempt = Uuid::now_v7().to_string();
        let digest = sha256(
            format!(
                "{}\n{}\n{body}",
                c.slot,
                c.remote_id.as_deref().unwrap_or("")
            )
            .as_bytes(),
        );
        let done = sqlx::query(
            "UPDATE publication_operations SET state = 'PUBLISHING', body = ?, request_digest = ?,
                 attempt_id = ?, attempts = attempts + 1
             WHERE id = ? AND owner = ? AND lease_epoch = ? AND state = 'PENDING' AND lease_until >= ?",
        )
        .bind(body)
        .bind(digest.to_string())
        .bind(&attempt)
        .bind(c.op_id)
        .bind(&c.owner)
        .bind(c.epoch)
        .bind(now.as_second())
        .execute(self.pool())
        .await?;
        Ok((done.rows_affected() == 1).then_some(attempt))
    }

    /// The reply arrived: delivery and the observed object commit together.
    pub async fn publication_delivered(
        &self,
        c: &PublishClaim,
        remote_id: &str,
        body: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE publication_operations SET state = 'DELIVERED', remote_id = ?, finished_at = ?
             WHERE id = ? AND state IN ('PUBLISHING', 'UNCERTAIN')",
        )
        .bind(remote_id)
        .bind(at)
        .bind(c.op_id)
        .execute(&mut *tx)
        .await?;
        observe(&mut tx, c.object_id, remote_id, c.generation, body, at).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The outcome is unknown: reconcile until `window` passes, never resend blindly.
    pub async fn publication_uncertain(
        &self,
        op_id: i64,
        error: &str,
        window: i64,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let at = now.as_second();
        sqlx::query(
            "UPDATE publication_operations SET state = 'UNCERTAIN', last_error = ?,
                 reconcile_deadline = coalesce(reconcile_deadline, ?), next_action_at = ?
             WHERE id = ? AND state = 'PUBLISHING'",
        )
        .bind(error)
        .bind(at + window)
        .bind(at)
        .bind(op_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// A definite refusal or a gate that is not open yet: back to `PENDING` at `retry_at`, or
    /// `FAILED` past the attempt bound.
    pub async fn publication_retry(
        &self,
        c: &PublishClaim,
        error: &str,
        retry_at: i64,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE publication_operations SET
                 state = CASE WHEN attempts >= ? THEN 'FAILED' ELSE 'PENDING' END,
                 owner = NULL, last_error = ?, next_action_at = ?
             WHERE id = ? AND state IN ('PENDING', 'PUBLISHING')",
        )
        .bind(MAX_PUBLISH_ATTEMPTS)
        .bind(error)
        .bind(retry_at)
        .bind(c.op_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Ends an operation that must not be sent (`FAILED`, `CANCELLED`, `SUPERSEDED`).
    pub async fn publication_end(
        &self,
        op_id: i64,
        state: &str,
        reason: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE publication_operations SET state = ?, last_error = ?, finished_at = ?
             WHERE id = ? AND state IN ('PENDING', 'PUBLISHING')",
        )
        .bind(state)
        .bind(reason)
        .bind(now.as_second())
        .bind(op_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// `PUBLISHING` with an expired lease: the request may have left, so it is `UNCERTAIN`.
    pub async fn reap_publications(
        &self,
        window: i64,
        now: Timestamp,
    ) -> Result<u64, StorageError> {
        let at = now.as_second();
        let done = sqlx::query(
            "UPDATE publication_operations SET state = 'UNCERTAIN', last_error = 'lease lost while publishing',
                 reconcile_deadline = ?, next_action_at = ?
             WHERE state = 'PUBLISHING' AND lease_until < ?",
        )
        .bind(at + window)
        .bind(at)
        .bind(at)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected())
    }

    /// The remote object was deleted by someone: the next revision creates it again, which
    /// is safe because its absence is known, not guessed.
    pub async fn forget_remote(&self, object_id: i64) -> Result<(), StorageError> {
        sqlx::query("UPDATE publication_objects SET remote_id = NULL WHERE id = ?")
            .bind(object_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Delivered comments with the body last sent, for detecting a late older write.
    pub async fn delivered_objects(&self) -> Result<Vec<(i64, i64, String, String)>, StorageError> {
        let rows = sqlx::query(
            "SELECT p.id, p.change_id, p.remote_id, o.body FROM publication_objects p
             JOIN publication_operations o ON o.id = (SELECT max(id) FROM publication_operations
                 WHERE object_id = p.id AND state = 'DELIVERED')
             WHERE p.remote_id IS NOT NULL AND p.kind = 'SUMMARY'
                 AND NOT EXISTS (SELECT 1 FROM publication_operations b WHERE b.object_id = p.id
                     AND b.state IN ('PENDING', 'PUBLISHING', 'UNCERTAIN'))",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get(0),
                    r.get(1),
                    r.get(2),
                    r.get::<Option<String>, _>(3).unwrap_or_default(),
                )
            })
            .collect())
    }

    /// `UNCERTAIN` operations due for a reconciliation pass.
    pub async fn uncertain_due(&self, now: Timestamp) -> Result<Vec<Uncertain>, StorageError> {
        let rows = sqlx::query(
            "SELECT o.id, o.object_id, p.change_id, p.marker, p.remote_id, coalesce(o.body, ''),
                 o.reconcile_deadline, p.kind
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             WHERE o.state = 'UNCERTAIN' AND o.next_action_at <= ? ORDER BY o.id",
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

    /// Reconciliation found the object: it is delivered after all.
    pub async fn uncertain_found(
        &self,
        u: &Uncertain,
        remote_id: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let generation: i64 = sqlx::query(
            "UPDATE publication_operations SET state = 'DELIVERED', remote_id = ?, finished_at = ?
             WHERE id = ? AND state = 'UNCERTAIN' RETURNING generation",
        )
        .bind(remote_id)
        .bind(at)
        .bind(u.op_id)
        .fetch_one(&mut *tx)
        .await?
        .get(0);
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
        sqlx::query(
            "UPDATE publication_operations SET next_action_at = ?, last_error = ?
             WHERE id = ? AND state = 'UNCERTAIN'",
        )
        .bind(if exhausted { i64::MAX } else { next })
        .bind(if exhausted {
            "RECONCILIATION_WINDOW_EXHAUSTED: operator decision needed"
        } else {
            "not found yet"
        })
        .bind(u.op_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

async fn observe(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    object_id: i64,
    remote_id: &str,
    generation: i64,
    body: &str,
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE publication_objects SET remote_id = ?, observed_generation = ?,
             observed_body_sha256 = ?, updated_at = ?,
             relevance = CASE WHEN ? >= (SELECT desired_generation FROM changes c
                 WHERE c.id = change_id) THEN 'CURRENT' ELSE 'STALE' END
         WHERE id = ?",
    )
    .bind(remote_id)
    .bind(generation)
    .bind(sha256(body.as_bytes()).to_string())
    .bind(at)
    .bind(generation)
    .bind(object_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
             WHERE o.state IN ('PUBLISHING', 'UNCERTAIN') AND (?1 IS NULL OR p.change_id = ?1)
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

impl Storage {
    /// The delivered summary operation of a change, shaped as a claim for re-rendering.
    pub async fn delivered_summary(
        &self,
        change_id: i64,
    ) -> Result<Option<PublishClaim>, StorageError> {
        let r = sqlx::query(
            "SELECT o.id, o.object_id, p.change_id, c.tenant_id, c.access_scope, p.slot, p.marker,
                 p.remote_id, o.run_id, o.result_id, o.generation, o.attempts,
                 coalesce(o.owner, ''), o.lease_epoch, p.kind, p.fingerprint, p.anchor
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             JOIN changes c ON c.id = p.change_id
             WHERE p.change_id = ? AND p.kind = 'SUMMARY' AND o.state = 'DELIVERED'
             ORDER BY o.id DESC LIMIT 1",
        )
        .bind(change_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(r.map(|r| PublishClaim {
            op_id: r.get(0),
            object_id: r.get(1),
            kind: r.get(14),
            fingerprint: r.get(15),
            anchor: r.get(16),
            change_id: r.get(2),
            partition: Partition {
                tenant_id: r.get(3),
                access_scope: r.get(4),
            },
            slot: r.get(5),
            marker: r.get(6),
            remote_id: r.get(7),
            run_id: r.get(8),
            result_id: r.get(9),
            generation: r.get(10),
            attempts: r.get(11),
            owner: r.get(12),
            epoch: r.get(13),
        }))
    }

    /// Records the body a delivered operation now carries remotely, so verification compares
    /// against it.
    pub async fn record_body(&self, op_id: i64, body: &str) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE publication_operations SET body = ? WHERE id = ? AND state = 'DELIVERED'",
        )
        .bind(body)
        .bind(op_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
