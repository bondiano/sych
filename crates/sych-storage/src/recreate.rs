//! Moving an `UNCERTAIN` operation with no known remote object back to `PENDING`:
//! by an audited operator decision, or by the reconciler once a complete read found nothing.

use sych_core::Timestamp;

use crate::audit::Audit;
use crate::publish::MAX_PUBLISH_ATTEMPTS;
use crate::{Storage, StorageError};

struct Recreate<'a> {
    actor: &'a str,
    action: &'a str,
    note: &'a str,
    metadata: serde_json::Value,
    max_attempts: i64,
}

impl Storage {
    /// An operator's audited decision to create again despite the known duplicate risk.
    pub async fn authorize_recreate(
        &self,
        op_id: i64,
        tenant_id: &str,
        actor: &str,
        reason: &str,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let r = Recreate {
            actor,
            action: "PUBLICATION_RECREATE_AUTHORIZED",
            note: "operator authorized a new CREATE",
            metadata: serde_json::json!({"reason": reason, "risk": "a duplicate comment may appear"}),
            max_attempts: i64::MAX,
        };
        self.recreate(op_id, tenant_id, &r, now).await
    }

    /// The reconciler read every page and found no object of its own: absence is known, so a
    /// new CREATE is safe. Bounded by the attempt limit; past it the operator decides.
    pub async fn recreate_absent(
        &self,
        op_id: i64,
        tenant_id: &str,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let r = Recreate {
            actor: "reconciler",
            action: "PUBLICATION_RECREATE_AUTOMATIC",
            note: "absent after a complete read: CREATE again",
            metadata: serde_json::json!({"reason": "a complete marker search found nothing"}),
            max_attempts: MAX_PUBLISH_ATTEMPTS,
        };
        self.recreate(op_id, tenant_id, &r, now).await
    }

    async fn recreate(
        &self,
        op_id: i64,
        tenant_id: &str,
        r: &Recreate<'_>,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let done = sqlx::query(
            "UPDATE publication_operations SET state = 'PENDING', owner = NULL, next_action_at = $1,
                 reconcile_deadline = NULL, last_error = $2
             WHERE id = $3 AND state = 'UNCERTAIN' AND attempts < $4
                 AND (SELECT remote_id FROM publication_objects p WHERE p.id = object_id) IS NULL",
        )
        .bind(at)
        .bind(r.note)
        .bind(op_id)
        .bind(r.max_attempts)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 1 {
            let repository_id: Option<i64> = sqlx::query_scalar(
                "SELECT c.repository_id FROM publication_operations o
                 JOIN runs u ON u.id = o.run_id JOIN changes c ON c.id = u.change_id
                 WHERE o.id = $1",
            )
            .bind(op_id)
            .fetch_one(&mut *tx)
            .await?;
            Audit {
                tenant_id,
                actor: r.actor,
                action: r.action,
                target: format!("publication_operation:{op_id}"),
                repository_id,
                metadata: r.metadata.clone(),
                at,
            }
            .record(&mut tx)
            .await?;
        }

        tx.commit().await?;
        Ok(done.rows_affected() == 1)
    }
}
