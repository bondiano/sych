//! Controlled artifact GC. Active runs and runs with an unfinished publication
//! (pending, publishing or uncertain) pin their artifacts; results of completed runs live until
//! retention; artifacts of failed or never-committed runs are orphans removed after a grace
//! period.

use sqlx::Row;
use sych_core::{Sha256Digest, Timestamp};

use crate::{Storage, StorageError};

/// Orphans younger than this are kept: their run may still be writing.
pub const ORPHAN_GRACE_SECONDS: i64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneReason {
    /// Not referenced by an active or completed run, older than the grace period.
    Orphan,
    /// Referenced only by completed runs and past its retention.
    RetentionExpired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneCandidate {
    pub sha256: Sha256Digest,
    pub kind: String,
    pub reason: PruneReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub candidates: Vec<PruneCandidate>,
    pub deleted: usize,
}

impl Storage {
    /// Lists, and unless `dry_run` deletes, artifacts that GC may remove at `now`.
    pub async fn prune(&self, now: Timestamp, dry_run: bool) -> Result<PruneReport, StorageError> {
        let rows = sqlx::query(
            "SELECT a.tenant_id, a.access_scope, a.sha256, a.kind, a.store_key,
                 a.created_at, a.retain_until,
                 CAST(CASE WHEN EXISTS (SELECT 1 FROM run_artifacts ra JOIN runs r ON r.id = ra.run_id
                     WHERE ra.tenant_id = a.tenant_id AND ra.access_scope = a.access_scope
                     AND ra.sha256 = a.sha256
                     AND (r.state IN ('QUEUED', 'RUNNING', 'WAITING_EVIDENCE', 'RETRY_WAIT',
                                      'FINALIZING')
                          OR EXISTS (SELECT 1 FROM publication_operations po WHERE po.run_id = r.id
                              AND po.state IN ('PENDING', 'PUBLISHING', 'UNCERTAIN'))))
                     THEN 1 ELSE 0 END AS BIGINT) AS pinned,
                 CAST(CASE WHEN EXISTS (SELECT 1 FROM run_artifacts ra JOIN runs r ON r.id = ra.run_id
                     WHERE ra.tenant_id = a.tenant_id AND ra.access_scope = a.access_scope
                     AND ra.sha256 = a.sha256 AND r.state = 'COMPLETED')
                     THEN 1 ELSE 0 END AS BIGINT) AS kept
             FROM artifacts a ORDER BY a.sha256",
        )
        .fetch_all(self.pool())
        .await?;
        let now = now.as_second();
        let mut report = PruneReport::default();
        for row in rows {
            let pinned = row.get::<i64, _>("pinned") != 0;
            let kept = row.get::<i64, _>("kept") != 0;
            let created_at: i64 = row.get("created_at");
            let retain_until: i64 = row.get("retain_until");
            let reason = if pinned {
                None
            } else if kept {
                (retain_until <= now).then_some(PruneReason::RetentionExpired)
            } else {
                (created_at + ORPHAN_GRACE_SECONDS <= now).then_some(PruneReason::Orphan)
            };
            let Some(reason) = reason else { continue };
            let digest: String = row.get("sha256");
            let sha256: Sha256Digest = digest
                .parse()
                .map_err(|e: sych_core::FormatError| StorageError::Corrupt(e.to_string()))?;
            if !dry_run {
                let tenant_id: String = row.get("tenant_id");
                let access_scope: String = row.get("access_scope");
                if !self
                    .delete_if_eligible(&tenant_id, &access_scope, &digest, now)
                    .await?
                {
                    continue;
                }

                report.deleted += 1;
            }

            report.candidates.push(PruneCandidate {
                sha256,
                kind: row.get("kind"),
                reason,
            });
        }
        Ok(report)
    }

    /// Re-checks eligibility and deletes row and bytes under the database write lock, so a pin
    /// or retention extension committed after the listing, by any process, keeps the artifact.
    async fn delete_if_eligible(
        &self,
        tenant_id: &str,
        access_scope: &str,
        digest: &str,
        now: i64,
    ) -> Result<bool, StorageError> {
        // The shared write lock keeps the eligibility check and unlink in one transaction.
        let mut tx = self.write().await?;
        let key: Option<String> = sqlx::query_scalar(
            "DELETE FROM artifacts AS a
             WHERE a.tenant_id = $1 AND a.access_scope = $2 AND a.sha256 = $3
             AND NOT EXISTS (SELECT 1 FROM run_artifacts ra JOIN runs r ON r.id = ra.run_id
                 WHERE ra.tenant_id = a.tenant_id AND ra.access_scope = a.access_scope
                 AND ra.sha256 = a.sha256
                 AND (r.state IN ('QUEUED', 'RUNNING', 'WAITING_EVIDENCE', 'RETRY_WAIT',
                                  'FINALIZING')
                      OR EXISTS (SELECT 1 FROM publication_operations po WHERE po.run_id = r.id
                          AND po.state IN ('PENDING', 'PUBLISHING', 'UNCERTAIN'))))
             AND CASE WHEN EXISTS (SELECT 1 FROM run_artifacts ra JOIN runs r ON r.id = ra.run_id
                     WHERE ra.tenant_id = a.tenant_id AND ra.access_scope = a.access_scope
                     AND ra.sha256 = a.sha256 AND r.state = 'COMPLETED')
                 THEN a.retain_until <= $4
                 ELSE a.created_at + $5 <= $4 END
             RETURNING store_key",
        )
        .bind(tenant_id)
        .bind(access_scope)
        .bind(digest)
        .bind(now)
        .bind(ORPHAN_GRACE_SECONDS)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(key) = key else { return Ok(false) };
        self.store().delete(&key)?;
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use sych_core::{PolicySnapshotId, SourceSnapshotId, Timestamp, sha256};

    use crate::{
        ArtifactKind, ArtifactStore, Database, Partition, Storage, StoreConfig, TENANT_LOCAL,
    };

    const T0: i64 = 1_800_000_000;
    const LATER: i64 = 2 * 3600;

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_second(T0 + offset).expect("timestamp")
    }

    #[tokio::test]
    async fn pin_committed_after_listing_keeps_the_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = StoreConfig {
            database: Database::Sqlite(dir.path().join("sych.db")),
            artifacts: dir.path().join("artifacts"),
        };
        Storage::migrate(&config).await.expect("migrate");
        let gc = Storage::open(&config).await.expect("open");
        // A second connection stands for a concurrent `review` process.
        let writer = Storage::open(&config).await.expect("open second");
        let partition = Partition {
            tenant_id: TENANT_LOCAL.into(),
            access_scope: "local:repo".into(),
        };
        let snapshot = SourceSnapshotId(sha256(b"snapshot"));
        let policy = PolicySnapshotId(sha256(b"policy"));
        let failed = writer
            .begin_run(&partition, "old", &snapshot, &policy, at(0))
            .await
            .expect("begin");
        let digest = writer
            .put_artifact(&failed, "facts", ArtifactKind::Facts, &"shared", at(0))
            .await
            .expect("put");
        writer.fail_run(&failed, "boom", at(1)).await.expect("fail");
        let listed = gc.prune(at(LATER), true).await.expect("list");
        assert_eq!(listed.candidates.len(), 1);

        let active = writer
            .begin_run(&partition, "new", &snapshot, &policy, at(LATER))
            .await
            .expect("begin");
        writer
            .put_artifact(&active, "facts", ArtifactKind::Facts, &"shared", at(LATER))
            .await
            .expect("pin");
        let deleted = gc
            .delete_if_eligible(
                &partition.tenant_id,
                &partition.access_scope,
                &digest.to_string(),
                T0 + LATER,
            )
            .await
            .expect("delete");

        assert!(!deleted);
        let key = ArtifactStore::key(&partition, &digest);
        assert!(dir.path().join("artifacts").join(key).exists());
        let relisted = gc.prune(at(LATER), true).await.expect("list");
        assert!(relisted.candidates.is_empty(), "{relisted:?}");
    }
}
