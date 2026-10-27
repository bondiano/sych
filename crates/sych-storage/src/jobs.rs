//! Durable jobs with leases (spec §8.2, §20.2): claim and a new fencing epoch in one
//! transaction, heartbeat, reaper and the fence predicate every authoritative write carries.

use sqlx::Row;
use sych_core::schedule::RunInputs;
use sych_core::{PolicySnapshotId, RunId, SourceSnapshotId, Timestamp};

use crate::{Partition, RunHandle, Storage, StorageError};

/// The lease a worker holds; a write without the current one is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    pub job_id: i64,
    pub owner: String,
    pub epoch: i64,
}

#[derive(Debug, Clone)]
pub struct Claim {
    pub fence: Fence,
    pub run: RunHandle,
    pub change_id: i64,
    pub inputs: RunInputs,
    pub stage: String,
    pub attempts: u32,
    pub deadline: i64,
    pub evidence_deadline: Option<i64>,
}

impl Storage {
    /// Claims the oldest ready job: queued and due, or claimed with an expired lease.
    pub async fn claim(
        &self,
        owner: &str,
        lease_seconds: i64,
        now: Timestamp,
    ) -> Result<Option<Claim>, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let row = sqlx::query(
            "UPDATE jobs SET state = 'CLAIMED', owner = ?, lease_epoch = lease_epoch + 1,
                 lease_until = ?, attempts = attempts + 1
             WHERE id = (SELECT id FROM jobs WHERE state = 'QUEUED' AND next_attempt_at <= ?
                 ORDER BY next_attempt_at, id LIMIT 1)
             RETURNING id, lease_epoch, run_id, attempts, deadline",
        )
        .bind(owner)
        .bind(at + lease_seconds)
        .bind(at)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(job) = row else {
            return Ok(None);
        };
        let run_id: String = job.get(2);
        let run = sqlx::query(
            "SELECT tenant_id, access_scope, generation, change_id, inputs, stage,
                 evidence_deadline FROM runs WHERE id = ?",
        )
        .bind(&run_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let inputs: String = run.get(4);
        let fence = Fence {
            job_id: job.get(0),
            owner: owner.to_owned(),
            epoch: job.get(1),
        };
        Ok(Some(Claim {
            fence: fence.clone(),
            run: RunHandle {
                id: RunId(
                    run_id
                        .parse()
                        .map_err(|_| StorageError::Corrupt(format!("run id {run_id}")))?,
                ),
                partition: Partition {
                    tenant_id: run.get(0),
                    access_scope: run.get(1),
                },
                generation: u64::try_from(run.get::<i64, _>(2)).unwrap_or_default(),
                fence: Some(fence.clone()),
            },
            change_id: run.get(3),
            inputs: serde_json::from_str(&inputs)?,
            stage: run.get(5),
            attempts: u32::try_from(job.get::<i64, _>(3)).unwrap_or(u32::MAX),
            deadline: job.get(4),
            evidence_deadline: run.get(6),
        }))
    }

    /// Extends the lease; `false` means it is lost (cancelled, superseded or reclaimed).
    pub async fn heartbeat(
        &self,
        fence: &Fence,
        lease_seconds: i64,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let at = now.as_second();
        let done = sqlx::query(
            "UPDATE jobs SET lease_until = ? WHERE id = ? AND owner = ? AND lease_epoch = ?
                 AND state = 'CLAIMED' AND lease_until >= ?",
        )
        .bind(at + lease_seconds)
        .bind(fence.job_id)
        .bind(&fence.owner)
        .bind(fence.epoch)
        .bind(at)
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Returns expired leases to the queue; past the attempt limit or deadline the job and its
    /// run fail instead. Returns how many jobs it moved.
    pub async fn reap(&self, now: Timestamp) -> Result<u64, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let failed: Vec<String> = sqlx::query(
            "UPDATE jobs SET state = 'FAILED', lease_epoch = lease_epoch + 1, finished_at = ?,
                 last_error = 'lease expired'
             WHERE state = 'CLAIMED' AND lease_until < ?
                 AND (attempts > max_attempts OR deadline <= ?)
             RETURNING run_id",
        )
        .bind(at)
        .bind(at)
        .bind(at)
        .fetch_all(&mut *tx)
        .await?
        .iter()
        .map(|r| r.get(0))
        .collect();
        for run in &failed {
            sqlx::query(concat!(
                "UPDATE runs SET state = 'FAILED', failure = 'worker lease expired',
                     finished_at = ? WHERE id = ? AND state IN ",
                active!()
            ))
            .bind(at)
            .bind(run)
            .execute(&mut *tx)
            .await?;
        }
        let requeued = sqlx::query(
            "UPDATE jobs SET state = 'QUEUED', owner = NULL, lease_epoch = lease_epoch + 1,
                 next_attempt_at = ?, last_error = 'lease expired'
             WHERE state = 'CLAIMED' AND lease_until < ?",
        )
        .bind(at)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(requeued.rows_affected() + failed.len() as u64)
    }

    /// Gives the job back for a later attempt (retry or evidence wait); fenced.
    pub async fn release(
        &self,
        fence: &Fence,
        next_attempt_at: i64,
        error: Option<&str>,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let done = sqlx::query(
            "UPDATE jobs SET state = 'QUEUED', owner = NULL, next_attempt_at = ?,
                 last_error = coalesce(?, last_error)
             WHERE id = ? AND owner = ? AND lease_epoch = ? AND state = 'CLAIMED'
                 AND lease_until >= ?",
        )
        .bind(next_attempt_at)
        .bind(error)
        .bind(fence.job_id)
        .bind(&fence.owner)
        .bind(fence.epoch)
        .bind(now.as_second())
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Pins the snapshot and policy, and either waits for `expected` CI sources until
    /// `evidence_deadline` or (with no deadline) proceeds; fenced.
    #[allow(clippy::too_many_arguments)] // one fenced update with what it pins
    pub async fn prepare_run(
        &self,
        fence: &Fence,
        run: &RunHandle,
        snapshot: &SourceSnapshotId,
        policy: &PolicySnapshotId,
        expected: &serde_json::Value,
        evidence_deadline: Option<i64>,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let state = if evidence_deadline.is_some() {
            "WAITING_EVIDENCE"
        } else {
            "RUNNING"
        };
        let done = sqlx::query(concat!(
            "UPDATE runs SET state = ?, stage = 'EVIDENCE', source_snapshot_id = ?,
                 policy_snapshot_id = ?, expected_sources = ?, evidence_deadline = ?
             WHERE id = ? AND state IN ",
            active!(),
            " AND ",
            fenced!()
        ))
        .bind(state)
        .bind(snapshot.0.to_string())
        .bind(policy.0.to_string())
        .bind(expected.to_string())
        .bind(evidence_deadline)
        .bind(run.id.to_string())
        .bind(fence.job_id)
        .bind(&fence.owner)
        .bind(fence.epoch)
        .bind(now.as_second())
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Freezes the inputs: from here a new external input needs a successor. `sources` records
    /// what was expected and what is missing; fenced.
    pub async fn freeze(
        &self,
        fence: &Fence,
        run: &RunId,
        sources: &serde_json::Value,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let done = sqlx::query(concat!(
            "UPDATE runs SET state = 'RUNNING', stage = 'ANALYZE', expected_sources = ?
             WHERE id = ? AND state IN ",
            active!(),
            " AND ",
            fenced!()
        ))
        .bind(sources.to_string())
        .bind(run.to_string())
        .bind(fence.job_id)
        .bind(&fence.owner)
        .bind(fence.epoch)
        .bind(now.as_second())
        .execute(self.pool())
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// Ends a claimed run FAILED with its job; fenced, so a late worker changes nothing.
    pub async fn fail_claimed(
        &self,
        fence: &Fence,
        run: &RunId,
        reason: &str,
        now: Timestamp,
    ) -> Result<bool, StorageError> {
        let at = now.as_second();
        let mut tx = self.write().await?;
        let done = sqlx::query(concat!(
            "UPDATE runs SET state = 'FAILED', failure = ?, finished_at = ?
             WHERE id = ? AND state IN ",
            active!(),
            " AND ",
            fenced!()
        ))
        .bind(reason)
        .bind(at)
        .bind(run.to_string())
        .bind(fence.job_id)
        .bind(&fence.owner)
        .bind(fence.epoch)
        .bind(at)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 1 {
            finish_job(&mut tx, fence, "FAILED", Some(reason), at).await?;
        }

        tx.commit().await?;
        Ok(done.rows_affected() == 1)
    }

    /// `(state, lease_epoch, attempts, next_attempt_at)` of the jobs of a run.
    pub async fn jobs_of(&self, run: &str) -> Result<Vec<(String, i64, i64, i64)>, StorageError> {
        let rows = sqlx::query(
            "SELECT state, lease_epoch, attempts, next_attempt_at FROM jobs WHERE run_id = ?
             ORDER BY id",
        )
        .bind(run)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
            .collect())
    }
}

pub(crate) async fn finish_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    fence: &Fence,
    state: &str,
    error: Option<&str>,
    at: i64,
) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE jobs SET state = ?, finished_at = ?, last_error = coalesce(?, last_error)
         WHERE id = ? AND lease_epoch = ?",
    )
    .bind(state)
    .bind(at)
    .bind(error)
    .bind(fence.job_id)
    .bind(fence.epoch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
