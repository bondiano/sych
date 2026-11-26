//! Local synthetic ref ownership and retention. Only the run that created a pin records it.

use std::path::PathBuf;

use sqlx::Row;
use sych_core::{GitObjectId, PolicySnapshotId, RunId, SourceSnapshotId, Timestamp};

use crate::gc::ORPHAN_GRACE_SECONDS;
use crate::{Partition, RunHandle, Storage, StorageError, TENANT_LOCAL};

pub struct LocalHeadPin<'a> {
    pub git_dir: &'a str,
    pub oid: &'a GitObjectId,
}

pub struct PrunableLocalHead {
    pub run: RunId,
    pub git_dir: PathBuf,
    pub oid: GitObjectId,
}

impl Storage {
    /// Records a RUNNING run with the next generation of its change, in one transaction.
    pub async fn begin_run(
        &self,
        partition: &Partition,
        change_key: &str,
        snapshot: &SourceSnapshotId,
        policy: &PolicySnapshotId,
        now: Timestamp,
    ) -> Result<RunHandle, StorageError> {
        self.begin_run_with_local_head(partition, change_key, snapshot, policy, now, None)
            .await
    }

    /// Records a local run and its private Git retention metadata in one transaction.
    pub async fn begin_run_with_local_head(
        &self,
        partition: &Partition,
        change_key: &str,
        snapshot: &SourceSnapshotId,
        policy: &PolicySnapshotId,
        now: Timestamp,
        pin: Option<LocalHeadPin<'_>>,
    ) -> Result<RunHandle, StorageError> {
        let mut tx = self.write().await?;
        let row = sqlx::query(
            "INSERT INTO changes (tenant_id, access_scope, change_key, desired_generation)
             VALUES ($1, $2, $3, 1)
             ON CONFLICT (tenant_id, access_scope, change_key)
             DO UPDATE SET desired_generation = changes.desired_generation + 1
             RETURNING id, desired_generation",
        )
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(change_key)
        .fetch_one(&mut *tx)
        .await?;
        let change_id: i64 = row.get(0);
        let generation: i64 = row.get(1);
        let id = RunId::new_v7();
        sqlx::query(
            "INSERT INTO runs (id, tenant_id, access_scope, change_id, generation,
                 source_snapshot_id, policy_snapshot_id, state, stage, created_at,
                 local_git_dir, local_head_oid)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'RUNNING', 'PLAN', $8, $9, $10)",
        )
        .bind(id.to_string())
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(change_id)
        .bind(generation)
        .bind(snapshot.0.to_string())
        .bind(policy.0.to_string())
        .bind(now.as_second())
        .bind(pin.as_ref().map(|p| p.git_dir))
        .bind(pin.map(|p| p.oid.to_string()))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(run = %id, generation, change = %change_key, "run started");
        Ok(RunHandle {
            id,
            partition: partition.clone(),
            generation: u64::try_from(generation).unwrap_or_default(),
            fence: None,
        })
    }

    /// Lists terminal run pins whose source has expired. Active runs and pending
    /// publications keep their refs even when they outlive the source retention.
    pub async fn prunable_local_heads(
        &self,
        now: Timestamp,
    ) -> Result<Vec<PrunableLocalHead>, StorageError> {
        let now = now.as_second();
        let rows = sqlx::query(
            "SELECT r.id, r.local_git_dir, r.local_head_oid FROM runs r
             WHERE r.tenant_id = $1 AND r.local_head_oid IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM publication_operations p
                WHERE p.run_id = r.id AND p.state IN ('PENDING', 'PUBLISHING', 'UNCERTAIN'))
             AND ((r.state = 'COMPLETED'
               AND NOT EXISTS (
                 SELECT 1 FROM run_artifacts ra JOIN artifacts a
                   ON a.tenant_id = ra.tenant_id AND a.access_scope = ra.access_scope
                   AND a.sha256 = ra.sha256
                 WHERE ra.run_id = r.id AND ra.role = 'source_snapshot'
                 AND a.retain_until > $2))
              OR (r.state IN ('FAILED', 'CANCELLED', 'SUPERSEDED')
                AND coalesce(r.finished_at, r.created_at) <= $3))
             ORDER BY r.id",
        )
        .bind(TENANT_LOCAL)
        .bind(now)
        .bind(now - ORPHAN_GRACE_SECONDS)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter()
            .map(|row| {
                let invalid = || StorageError::Corrupt("invalid local head pin".into());
                let run: &str = row.try_get("id")?;
                let oid: &str = row.try_get("local_head_oid")?;
                let git_dir: &str = row.try_get("local_git_dir")?;
                let git_dir = PathBuf::from(git_dir);
                if !git_dir.is_absolute() {
                    return Err(invalid());
                }

                Ok(PrunableLocalHead {
                    run: RunId(run.parse().map_err(|_| invalid())?),
                    oid: oid.parse().map_err(|_| invalid())?,
                    git_dir,
                })
            })
            .collect()
    }

    /// Clears ownership after the caller removes the expected ref. An interrupted
    /// deletion can resume because deleting the same ref repeatedly is safe.
    pub async fn forget_local_head(&self, pin: &PrunableLocalHead) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE runs SET local_git_dir = NULL, local_head_oid = NULL
             WHERE id = $1 AND tenant_id = $2 AND local_head_oid = $3",
        )
        .bind(pin.run.to_string())
        .bind(TENANT_LOCAL)
        .bind(pin.oid.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
