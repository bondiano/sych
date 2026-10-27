//! Change state, generations and the event log (spec §8.3, §19.3). A generation, the supersede
//! of the active one and its job commit in one transaction.

use sqlx::Row;
use sych_core::schedule::{Latest, RunInputs, ScmState, Trigger};
use sych_core::{RunId, Timestamp, canonical_json, sha256};

use crate::{Partition, RunHandle, Storage, StorageError};

/// A change with its repository binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRow {
    pub id: i64,
    pub partition: Partition,
    pub change_key: String,
    pub connection_id: String,
    pub repository_external_id: String,
    pub full_name: String,
    pub installation_id: Option<u64>,
    pub number: u64,
    pub scm_state: Option<String>,
    pub head_sha: Option<String>,
}

fn json(value: &impl serde::Serialize) -> Result<String, StorageError> {
    Ok(String::from_utf8_lossy(&canonical_json(value)?).into_owned())
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

impl Storage {
    pub async fn change(&self, change_id: i64) -> Result<ChangeRow, StorageError> {
        let r = sqlx::query(
            "SELECT c.id, c.tenant_id, c.access_scope, c.change_key, r.connection_id,
                 r.full_name, r.installation_id, c.external_number, c.scm_state, c.head_sha,
                 r.external_id
             FROM changes c JOIN repositories r ON r.id = c.repository_id WHERE c.id = ?",
        )
        .bind(change_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::Conflict(format!("change {change_id} has no binding")))?;
        Ok(ChangeRow {
            id: r.get(0),
            partition: Partition {
                tenant_id: r.get(1),
                access_scope: r.get(2),
            },
            change_key: r.get(3),
            connection_id: r.get(4),
            repository_external_id: r.get(10),
            full_name: r.get(5),
            installation_id: r.get::<Option<i64>, _>(6).map(unsigned),
            number: unsigned(r.get::<Option<i64>, _>(7).unwrap_or_default()),
            scm_state: r.get(8),
            head_sha: r.get(9),
        })
    }

    /// Changes of a connection the scheduler believes open, for reconciliation.
    pub async fn open_changes(&self, connection_id: &str) -> Result<Vec<ChangeRow>, StorageError> {
        let ids: Vec<i64> = sqlx::query(
            "SELECT c.id FROM changes c JOIN repositories r ON r.id = c.repository_id
             WHERE r.connection_id = ? AND coalesce(c.scm_state, 'OPEN') IN ('OPEN', 'DRAFT')",
        )
        .bind(connection_id)
        .fetch_all(self.pool())
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
        let mut out = Vec::new();
        for id in ids {
            out.push(self.change(id).await?);
        }
        Ok(out)
    }

    /// The API state of the change, recorded before any decision.
    pub async fn sync_change(
        &self,
        change_id: i64,
        state: &ScmState,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let scm = match (state.open, state.merged, state.draft) {
            (false, true, _) => "MERGED",
            (false, false, _) => "CLOSED",
            (true, _, true) => "DRAFT",
            (true, _, false) => "OPEN",
        };
        sqlx::query(
            "UPDATE changes SET scm_state = ?, head_sha = ?, base_sha = ?, base_ref = ?,
                 fork = ?, synced_at = ? WHERE id = ?",
        )
        .bind(scm)
        .bind(&state.head_sha)
        .bind(&state.base_sha)
        .bind(&state.base_ref)
        .bind(state.fork)
        .bind(now.as_second())
        .bind(change_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The newest generation of a change, as the decision needs it.
    pub async fn latest(&self, change_id: i64) -> Result<Option<Latest>, StorageError> {
        let row = sqlx::query(
            "SELECT id, inputs, state, stage, expected_sources FROM runs
             WHERE change_id = ? AND inputs IS NOT NULL ORDER BY generation DESC LIMIT 1",
        )
        .bind(change_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(r) = row else { return Ok(None) };
        let inputs: String = r.get(1);
        let state: String = r.get(2);
        let expected: Option<String> = r.get(4);
        let missing = expected
            .and_then(|e| serde_json::from_str::<serde_json::Value>(&e).ok())
            .is_some_and(|e| e["missing"].as_array().is_some_and(|m| !m.is_empty()));
        Ok(Some(Latest {
            run_id: r.get(0),
            inputs: serde_json::from_str::<RunInputs>(&inputs)?,
            active: ["QUEUED", "WAITING_EVIDENCE", "RUNNING"].contains(&state.as_str()),
            // Inputs freeze when analysis starts; PREPARE and EVIDENCE may still take CI.
            frozen: !["PREPARE", "EVIDENCE"].contains(&r.get::<String, _>(3).as_str()),
            missing_sources: missing,
        }))
    }

    pub async fn log_event(
        &self,
        change_id: i64,
        run_id: Option<&str>,
        kind: &str,
        detail: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO change_events (change_id, run_id, kind, detail, at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(change_id)
        .bind(run_id)
        .bind(kind)
        .bind(detail)
        .bind(now.as_second())
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// `(kind, detail, run)` of every logged event of a change, oldest first.
    pub async fn change_events(
        &self,
        change_id: i64,
    ) -> Result<Vec<(String, String, Option<String>)>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, detail, run_id FROM change_events WHERE change_id = ? ORDER BY id",
        )
        .bind(change_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    pub async fn consume_intent(&self, intent_id: i64, now: Timestamp) -> Result<(), StorageError> {
        sqlx::query(
            "UPDATE change_intents SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(now.as_second())
        .bind(intent_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The next generation and its analysis job; an active older run is superseded and its
    /// job cancelled, which revokes its lease (spec §8.3).
    pub async fn generate(
        &self,
        change_id: i64,
        inputs: &RunInputs,
        deadline: i64,
        max_attempts: u32,
        now: Timestamp,
    ) -> Result<RunHandle, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let superseded = sqlx::query(concat!(
            "UPDATE runs SET state = 'SUPERSEDED', finished_at = ?
             WHERE change_id = ? AND state IN ",
            active!(),
            " RETURNING id"
        ))
        .bind(at)
        .bind(change_id)
        .fetch_all(&mut *tx)
        .await?;
        for r in &superseded {
            sqlx::query(
                "UPDATE jobs SET state = 'CANCELLED', lease_epoch = lease_epoch + 1,
                     finished_at = ? WHERE run_id = ? AND state IN ('QUEUED', 'CLAIMED')",
            )
            .bind(at)
            .bind(r.get::<String, _>(0))
            .execute(&mut *tx)
            .await?;
        }
        let change = sqlx::query(
            "UPDATE changes SET desired_generation = desired_generation + 1 WHERE id = ?
             RETURNING tenant_id, access_scope, desired_generation",
        )
        .bind(change_id)
        .fetch_one(&mut *tx)
        .await?;
        let partition = Partition {
            tenant_id: change.get(0),
            access_scope: change.get(1),
        };
        let generation: i64 = change.get(2);
        let id = RunId::new_v7();
        let text = json(inputs)?;
        let parent = match &inputs.trigger {
            Trigger::Successor { parent, .. } => Some(parent.clone()),
            _ => None,
        };
        sqlx::query(
            "INSERT INTO runs (id, tenant_id, access_scope, change_id, generation, parent_run_id,
                 inputs, inputs_digest, state, stage, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'QUEUED', 'PREPARE', ?)",
        )
        .bind(id.to_string())
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(change_id)
        .bind(generation)
        .bind(parent)
        .bind(&text)
        .bind(sha256(text.as_bytes()).to_string())
        .bind(at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO jobs (run_id, kind, task_key, state, deadline, max_attempts,
                 next_attempt_at, created_at)
             VALUES (?, 'ANALYSIS', '', 'QUEUED', ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(deadline)
        .bind(i64::from(max_attempts))
        .bind(at)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        for r in &superseded {
            log(
                &mut tx,
                change_id,
                &r.get::<String, _>(0),
                "SUPERSEDED",
                &id.to_string(),
                at,
            )
            .await?;
        }
        log(&mut tx, change_id, &id.to_string(), "GENERATED", &text, at).await?;
        tx.commit().await?;
        Ok(RunHandle {
            id,
            partition,
            generation: unsigned(generation),
            fence: None,
        })
    }

    /// Cancels the active runs and their jobs; a committed result keeps its state.
    pub async fn cancel_change(
        &self,
        change_id: i64,
        reason: &str,
        now: Timestamp,
    ) -> Result<Vec<String>, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let cancelled: Vec<String> = sqlx::query(concat!(
            "UPDATE runs SET state = 'CANCELLED', failure = ?, finished_at = ?
             WHERE change_id = ? AND state IN ",
            active!(),
            " RETURNING id"
        ))
        .bind(reason)
        .bind(at)
        .bind(change_id)
        .fetch_all(&mut *tx)
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
        for run in &cancelled {
            sqlx::query(
                "UPDATE jobs SET state = 'CANCELLED', lease_epoch = lease_epoch + 1,
                     finished_at = ? WHERE run_id = ? AND state IN ('QUEUED', 'CLAIMED')",
            )
            .bind(at)
            .bind(run)
            .execute(&mut *tx)
            .await?;
            log(&mut tx, change_id, run, "CANCELLED", reason, at).await?;
        }
        // Unsent publication stops too; an uncertain one stays for read-only reconciliation.
        sqlx::query(
            "UPDATE publication_operations SET state = 'CANCELLED', last_error = ?, finished_at = ?
             WHERE state = 'PENDING' AND object_id IN
                 (SELECT id FROM publication_objects WHERE change_id = ?)",
        )
        .bind(reason)
        .bind(at)
        .bind(change_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(cancelled)
    }

    /// Makes a run waiting for evidence claimable now.
    pub async fn wake(&self, run_id: &str, now: Timestamp) -> Result<bool, StorageError> {
        let done = sqlx::query(
            "UPDATE jobs SET next_attempt_at = ? WHERE run_id = ? AND state = 'QUEUED'",
        )
        .bind(now.as_second())
        .bind(run_id)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// `(state, stage, generation, parent)` of a run in a tenant.
    pub async fn run_row(
        &self,
        tenant_id: &str,
        run: &str,
    ) -> Result<(String, String, i64, Option<String>), StorageError> {
        let r = sqlx::query(
            "SELECT state, stage, generation, parent_run_id FROM runs WHERE id = ? AND tenant_id = ?",
        )
        .bind(run)
        .bind(tenant_id)
        .fetch_optional(self.pool())
        .await?
        .ok_or_else(|| StorageError::Conflict(format!("run {run} not found")))?;
        Ok((r.get(0), r.get(1), r.get(2), r.get(3)))
    }
}

async fn log(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    change_id: i64,
    run: &str,
    kind: &str,
    detail: &str,
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO change_events (change_id, run_id, kind, detail, at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(change_id)
    .bind(run)
    .bind(kind)
    .bind(detail)
    .bind(at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
