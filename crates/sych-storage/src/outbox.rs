//! Outbox intents created by finalization: the publication a committed result asks
//! for lands in the same transaction as the result pointer, so a crash after commit leaves a
//! durable intent for the publisher and never a second analysis.

use sqlx::{Any, Row, Transaction};
use sych_core::{ResultId, schedule::RunInputs};

use crate::{RunHandle, StorageError};

fn publication_flags(inputs: Option<&str>) -> Result<(bool, bool), StorageError> {
    let inputs = inputs.map(serde_json::from_str::<RunInputs>).transpose()?;
    Ok(inputs.map_or((false, false), |inputs| (inputs.publish, inputs.checks)))
}

/// Queues the summary revision of `result` when the run's inputs ask for publication; an older
/// unsent revision of the same object is superseded. Returns the operation id.
pub(crate) async fn enqueue(
    tx: &mut Transaction<'_, Any>,
    run: &RunHandle,
    result: ResultId,
    at: i64,
) -> Result<Option<i64>, StorageError> {
    enqueue_summary(tx, &run.id.to_string(), &result.to_string(), at, false).await
}

/// The summary operation of a committed run; `force` publishes even when the run's inputs did
/// not ask for it (an authorized `sych publish`).
async fn enqueue_summary(
    tx: &mut Transaction<'_, Any>,
    run_id: &str,
    result: &str,
    at: i64,
    force: bool,
) -> Result<Option<i64>, StorageError> {
    let row = sqlx::query("SELECT change_id, generation, inputs FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
    let (change_id, generation, inputs): (i64, i64, Option<String>) =
        (row.get(0), row.get(1), row.get(2));
    let (publish, _) = publication_flags(inputs.as_deref())?;
    enqueue_check(tx, run_id, result, at).await?;
    if !publish && !force {
        return Ok(None);
    }

    let object: i64 = sqlx::query(
        "INSERT INTO publication_objects (change_id, slot, kind, marker, desired_generation,
             desired_result_id, updated_at)
         VALUES ($1, 'summary', 'SUMMARY', $2, $3, $4, $5)
         ON CONFLICT (change_id, slot) DO UPDATE SET
             desired_generation = excluded.desired_generation,
             desired_result_id = excluded.desired_result_id, updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(change_id)
    .bind(format!("sych:summary:{change_id}"))
    .bind(generation)
    .bind(result)
    .bind(at)
    .fetch_one(&mut **tx)
    .await?
    .get(0);
    sqlx::query(
        "UPDATE publication_operations SET state = 'SUPERSEDED', finished_at = $1
         WHERE object_id = $2 AND state = 'PENDING'",
    )
    .bind(at)
    .bind(object)
    .execute(&mut **tx)
    .await?;
    let op: Option<i64> = sqlx::query(
        "INSERT INTO publication_operations (object_id, run_id, result_id, generation, state,
             next_action_at, created_at)
         VALUES ($1, $2, $3, $4, 'PENDING', $5, $6)
         ON CONFLICT (object_id, generation) DO NOTHING RETURNING id",
    )
    .bind(object)
    .bind(run_id)
    .bind(result)
    .bind(generation)
    .bind(at)
    .bind(at)
    .fetch_optional(&mut **tx)
    .await?
    .map(|r| r.get(0));
    Ok(op)
}

impl crate::Storage {
    /// Queues the summary of a committed run on an authorized request; a revision already
    /// queued or sent is left as it is.
    pub async fn enqueue_publication(
        &self,
        tenant_id: &str,
        run_id: &str,
        at: i64,
    ) -> Result<Option<i64>, StorageError> {
        let mut tx = self.write().await?;
        let result: String = sqlx::query_scalar(
            "SELECT result_id FROM runs WHERE id = $1 AND tenant_id = $2 AND state = 'COMPLETED'",
        )
        .bind(run_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StorageError::Conflict(format!("run {run_id} has no committed result")))?;
        let op = enqueue_summary(&mut tx, run_id, &result, at, true).await?;
        tx.commit().await?;
        Ok(op)
    }
}

/// One queued or sent revision of a publication object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub id: i64,
    pub object_id: i64,
    pub slot: String,
    pub state: String,
    pub generation: i64,
    pub result_id: String,
}

impl crate::Storage {
    /// Every operation of a change's publication objects, oldest first.
    pub async fn operations(&self, change_id: i64) -> Result<Vec<Operation>, StorageError> {
        let rows = sqlx::query(
            "SELECT o.id, o.object_id, p.slot, o.state, o.generation, o.result_id
             FROM publication_operations o JOIN publication_objects p ON p.id = o.object_id
             WHERE p.change_id = $1 ORDER BY o.id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| Operation {
                id: r.get(0),
                object_id: r.get(1),
                slot: r.get(2),
                state: r.get(3),
                generation: r.get(4),
                result_id: r.get(5),
            })
            .collect())
    }
}

impl crate::Storage {
    /// Queues inline comments planned for a revision: one object per fingerprint, created once;
    /// a fingerprint that already has an object is left alone. Returns the new operation ids.
    pub async fn enqueue_inline(
        &self,
        change_id: i64,
        run_id: &str,
        result_id: &str,
        generation: i64,
        items: &[(String, String)],
        at: i64,
    ) -> Result<Vec<i64>, StorageError> {
        let mut tx = self.write().await?;
        let mut ops = Vec::new();
        for (fingerprint, anchor) in items {
            let object: Option<i64> = sqlx::query_scalar(
                "INSERT INTO publication_objects (change_id, slot, kind, marker, desired_generation,
                     desired_result_id, fingerprint, anchor, updated_at)
                 VALUES ($1, $2, 'INLINE', $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (change_id, slot) DO NOTHING RETURNING id",
            )
            .bind(change_id)
            .bind(format!("inline:{fingerprint}"))
            .bind(format!("sych:inline:{fingerprint}"))
            .bind(generation)
            .bind(result_id)
            .bind(fingerprint)
            .bind(anchor)
            .bind(at)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(object) = object else { continue };
            ops.push(
                sqlx::query_scalar(
                    "INSERT INTO publication_operations (object_id, run_id, result_id, generation,
                         state, next_action_at, created_at)
                     VALUES ($1, $2, $3, $4, 'PENDING', $5, $6) RETURNING id",
                )
                .bind(object)
                .bind(run_id)
                .bind(result_id)
                .bind(generation)
                .bind(at)
                .bind(at)
                .fetch_one(&mut *tx)
                .await?,
            );
        }
        tx.commit().await?;
        Ok(ops)
    }

    /// `(fingerprint, rejected)` of the change's inline objects; `rejected` when the provider
    /// refused the position and nothing was delivered.
    pub async fn inline_objects(
        &self,
        change_id: i64,
    ) -> Result<Vec<(String, bool)>, StorageError> {
        let rows = sqlx::query(
            "SELECT p.fingerprint, CAST(CASE WHEN p.remote_id IS NULL AND EXISTS
                 (SELECT 1 FROM publication_operations o WHERE o.object_id = p.id
                     AND o.state = 'FAILED' AND o.last_error LIKE 'INVALID_ANCHOR%')
                 THEN 1 ELSE 0 END AS BIGINT)
             FROM publication_objects p WHERE p.change_id = $1 AND p.kind = 'INLINE'",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get::<i64, _>(1) != 0))
            .collect())
    }

    /// Queues the advisory check run of a failed server run, when its inputs ask for checks;
    /// a failure is shown as such, never as an empty success.
    pub async fn enqueue_failure_check(
        &self,
        run: &crate::RunHandle,
        at: i64,
    ) -> Result<Option<i64>, StorageError> {
        let mut tx = self.write().await?;
        let op = enqueue_check(&mut tx, &run.id.to_string(), "", at).await?;
        tx.commit().await?;
        Ok(op)
    }
}

/// The check object of the run's change and an operation for this generation, when the run's
/// inputs ask for checks.
pub(crate) async fn enqueue_check(
    tx: &mut Transaction<'_, Any>,
    run_id: &str,
    result_id: &str,
    at: i64,
) -> Result<Option<i64>, StorageError> {
    let row = sqlx::query("SELECT change_id, generation, inputs FROM runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&mut **tx)
        .await?;
    let (change_id, generation, inputs): (i64, i64, Option<String>) =
        (row.get(0), row.get(1), row.get(2));
    let (_, checks) = publication_flags(inputs.as_deref())?;
    if !checks {
        return Ok(None);
    }

    let object: i64 = sqlx::query(
        "INSERT INTO publication_objects (change_id, slot, kind, marker, desired_generation,
             desired_result_id, updated_at)
         VALUES ($1, 'check', 'CHECK', $2, $3, $4, $5)
         ON CONFLICT (change_id, slot) DO UPDATE SET desired_generation = excluded.desired_generation,
             desired_result_id = excluded.desired_result_id, updated_at = excluded.updated_at
         RETURNING id",
    )
    .bind(change_id)
    .bind(format!("sych:check:{change_id}"))
    .bind(generation)
    .bind(result_id)
    .bind(at)
    .fetch_one(&mut **tx)
    .await?
    .get(0);
    let op = sqlx::query_scalar(
        "INSERT INTO publication_operations (object_id, run_id, result_id, generation, state,
             next_action_at, created_at)
         VALUES ($1, $2, $3, $4, 'PENDING', $5, $6)
         ON CONFLICT (object_id, generation) DO NOTHING RETURNING id",
    )
    .bind(object)
    .bind(run_id)
    .bind(result_id)
    .bind(generation)
    .bind(at)
    .bind(at)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(op)
}

#[cfg(test)]
mod tests {
    use super::publication_flags;

    #[test]
    fn publication_requires_valid_stored_inputs() {
        assert_eq!(publication_flags(None).expect("no inputs"), (false, false));
        let mut inputs = serde_json::json!({
            "head_sha": "head",
            "base_sha": "base",
            "base_ref": "main",
            "fork": false,
            "trigger": {"kind": "CHANGED"}
        });
        assert_eq!(
            publication_flags(Some(&inputs.to_string())).expect("default flags"),
            (false, false)
        );
        inputs["publish"] = true.into();
        inputs["checks"] = true.into();
        assert_eq!(
            publication_flags(Some(&inputs.to_string())).expect("publication flags"),
            (true, true)
        );
        assert!(publication_flags(Some("{")).is_err());
        assert!(publication_flags(Some("{}")).is_err());
        inputs["checks"] = "true".into();
        assert!(publication_flags(Some(&inputs.to_string())).is_err());
    }
}
