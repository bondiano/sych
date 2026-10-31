//! Durable local runs, result pointers and immutable artifacts.
//!
//! One serialized writer across `SQLite` (WAL) or `PostgreSQL` connections. Schema changes happen
//! only through [`Storage::migrate`]; [`Storage::open`] refuses an unmigrated database.

#[macro_use]
mod sql;
mod admission;
mod api;
mod artifacts;
mod checkpoints;
mod connection;
mod gc;
mod import;
mod jobs;
mod ledger;
mod lineage;
mod outbox;
mod publish;
mod recreate;
mod retention;
mod scheduler;
mod totals;
mod tour_sessions;
mod views;

use std::path::PathBuf;
use std::{fmt, io};

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::{AnyPool, Row};
use sych_core::{
    DecodeError, PolicySnapshotId, ReviewResult, RunId, SCHEMA_VERSION, Sha256Digest,
    SourceSnapshotId, Timestamp, canonical_json, decode, sha256,
};
use thiserror::Error;

pub use admission::{Admitted, Connection, Intent, MAX_DEBOUNCES};
pub use api::{IDEMPOTENCY_TTL_SECONDS, Idempotency, Metrics, RunView};
pub use artifacts::ArtifactStore;
pub use checkpoints::{CHECKPOINT_ROLE, RunLedger};
pub use gc::{PruneCandidate, PruneReason, PruneReport};
pub use jobs::{Claim, Fence};
pub use lineage::{FindingRow, WaiverRecord};
pub use outbox::Operation;
pub use publish::{MAX_PUBLISH_ATTEMPTS, PublishClaim, Uncertain, Unresolved};
pub use retention::RecordPrune;
pub use scheduler::ChangeRow;
pub use tour_sessions::tour_role;
pub use views::{ChangeFilter, FindingFilter, Visibility};

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("../../migrations/postgres");

pub const TENANT_LOCAL: &str = "local";
const DAY: i64 = 86_400;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database {0} is not migrated to this build; run `sych db migrate`")]
    NotMigrated(String),
    #[error("database {0} has migrations this build does not know; use a newer sych")]
    NewerSchema(String),
    #[error("run {0} was not found")]
    UnknownRun(RunId),
    #[error("run {run} is {state}, not COMPLETED; it has no saved result")]
    NotCompleted { run: RunId, state: String },
    /// Pruned or lost bytes are reported, never regenerated.
    #[error("artifact {0} is no longer available")]
    ArtifactUnavailable(Sha256Digest),
    #[error("storage conflict: {0}")]
    Conflict(String),
    #[error("corrupt storage: {0}")]
    Corrupt(String),
    #[error("cannot decode stored document: {0}")]
    Decode(#[from] DecodeError),
    #[error("cannot encode document: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migrate(#[from] MigrateError),
    #[error("artifact store: {0}")]
    Io(#[from] io::Error),
}

/// Tenant and access scope; every query and artifact path is partitioned by both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub tenant_id: String,
    pub access_scope: String,
}

/// Retention by artifact kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    SourceSnapshot,
    DiffManifest,
    PolicySnapshot,
    ExternalInputManifest,
    EvidenceManifest,
    Evidence,
    Facts,
    ContextPackets,
    /// Model passes, attempts and what the engine accepted or refused from them.
    Analysis,
    ReviewResult,
    /// An accepted model answer with its dependencies, for successor runs.
    Checkpoint,
    /// A guided tour of the run's result; one per generation, never rewritten.
    Tour,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceSnapshot => "source_snapshot",
            Self::DiffManifest => "diff_manifest",
            Self::PolicySnapshot => "policy_snapshot",
            Self::ExternalInputManifest => "external_input_manifest",
            Self::EvidenceManifest => "evidence_manifest",
            Self::Evidence => "evidence",
            Self::Facts => "facts",
            Self::ContextPackets => "context_packets",
            Self::Analysis => "analysis",
            Self::ReviewResult => "review_result",
            Self::Checkpoint => "checkpoint",
            Self::Tour => "tour",
        }
    }

    /// Source data 7 days; normalized results and evidence 30 days.
    pub const fn retention_seconds(self) -> i64 {
        match self {
            Self::SourceSnapshot | Self::DiffManifest => 7 * DAY,
            _ => 30 * DAY,
        }
    }
}

/// A local CLI database or the server database; `PostgreSQL` credentials are never displayed.
#[derive(Clone, PartialEq, Eq)]
pub enum Database {
    Sqlite(PathBuf),
    Postgres(String),
}

impl fmt::Display for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(path) => fmt::Display::fmt(&path.display(), f),
            Self::Postgres(_) => f.write_str("PostgreSQL"),
        }
    }
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub database: Database,
    pub artifacts: PathBuf,
}

/// An active run; its artifacts are pinned against GC until it is terminal. A server run
/// carries the lease it is worked under; its pins and result need that fence.
#[derive(Debug, Clone)]
pub struct RunHandle {
    pub id: RunId,
    pub partition: Partition,
    pub generation: u64,
    pub fence: Option<Fence>,
}

pub struct Storage {
    pool: AnyPool,
    store: ArtifactStore,
    postgres: bool,
}

fn int(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
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
                 source_snapshot_id, policy_snapshot_id, state, stage, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, 'RUNNING', 'PLAN', $8)",
        )
        .bind(id.to_string())
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(change_id)
        .bind(generation)
        .bind(snapshot.0.to_string())
        .bind(policy.0.to_string())
        .bind(now.as_second())
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

    /// Stores a document as canonical JSON and pins it to the run under `role`.
    /// The row is written before the bytes, so a crash leaves a row GC can see.
    pub async fn put_artifact<T: Serialize>(
        &self,
        run: &RunHandle,
        role: &str,
        kind: ArtifactKind,
        value: &T,
        now: Timestamp,
    ) -> Result<Sha256Digest, StorageError> {
        let bytes = canonical_json(value)?;
        let digest = sha256(&bytes);
        let key = ArtifactStore::key(&run.partition, &digest);
        let p = &run.partition;
        let mut tx = self.write().await?;
        sqlx::query(
            "INSERT INTO artifacts (tenant_id, access_scope, sha256, kind, schema_version,
                 store_key, size, created_at, retain_until)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (tenant_id, access_scope, sha256)
             DO UPDATE SET retain_until = CASE
                 WHEN artifacts.retain_until > excluded.retain_until THEN artifacts.retain_until
                 ELSE excluded.retain_until END",
        )
        .bind(&p.tenant_id)
        .bind(&p.access_scope)
        .bind(digest.to_string())
        .bind(kind.as_str())
        .bind(i64::from(SCHEMA_VERSION))
        .bind(&key)
        .bind(int(bytes.len() as u64))
        .bind(now.as_second())
        .bind(now.as_second() + kind.retention_seconds())
        .execute(&mut *tx)
        .await?;
        // A retried server run pins again under its current fence; a stale one pins nothing.
        let fence = run.fence.as_ref();
        let pinned = sqlx::query(concat!(
            "INSERT INTO run_artifacts (run_id, role, tenant_id, access_scope, sha256)
             SELECT $1, $2, $3, $4, $5 WHERE CAST($6 AS BIGINT) IS NULL
                 OR EXISTS (SELECT 1 FROM runs WHERE runs.id = $7 AND runs.state IN ",
            active!(),
            " AND ",
            fenced!("$8", "$9", "$10", "$11"),
            ")
             ON CONFLICT (run_id, role) DO UPDATE SET sha256 = excluded.sha256"
        ))
        .bind(run.id.to_string())
        .bind(role)
        .bind(&p.tenant_id)
        .bind(&p.access_scope)
        .bind(digest.to_string())
        .bind(fence.map(|f| f.job_id))
        .bind(run.id.to_string())
        .bind(fence.map(|f| f.job_id))
        .bind(fence.map(|f| f.owner.clone()))
        .bind(fence.map(|f| f.epoch))
        .bind(now.as_second())
        .execute(&mut *tx)
        .await?;
        if pinned.rows_affected() != 1 {
            return Err(StorageError::Conflict(format!(
                "run {} is no longer worked under this lease",
                run.id
            )));
        }

        tx.commit().await?;
        self.store.put(&key, &bytes)?;
        Ok(digest)
    }

    /// Result pointer, usage and terminal state in one transaction. Fails, and
    /// leaves the run RUNNING, if the result bytes are not stored or the run is not active; a
    /// server run also needs its current `fence` and generation, and finishes its job.
    pub async fn complete_run(
        &self,
        run: &RunHandle,
        result: &ReviewResult,
        result_sha256: &Sha256Digest,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        if result.run_id != run.id {
            return Err(StorageError::Conflict(format!(
                "result belongs to run {}, not {}",
                result.run_id, run.id
            )));
        }

        let p = &run.partition;
        let fence = run.fence.as_ref();
        let mut tx = self.write().await?;
        let stored = sqlx::query(
            "SELECT a.store_key FROM run_artifacts ra
             JOIN artifacts a ON a.tenant_id = ra.tenant_id
                 AND a.access_scope = ra.access_scope AND a.sha256 = ra.sha256
             WHERE ra.run_id = $1 AND ra.role = 'review_result' AND ra.sha256 = $2",
        )
        .bind(run.id.to_string())
        .bind(result_sha256.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = stored else {
            return Err(StorageError::Conflict(
                "result artifact is not stored for this run".into(),
            ));
        };
        let key: String = row.get(0);
        // The pointer may only reference bytes that are already durable.
        self.store.get(&key, result_sha256)?;
        let usage = &result.usage_summary;
        let updated = sqlx::query(concat!(
            "UPDATE runs SET state = 'COMPLETED', stage = 'REDUCE', result_id = $1,
                 result_sha256 = $2, model_attempts = $3, input_tokens = $4, output_tokens = $5,
                 cost_known_micro_usd = $6, unknown_cost_attempts = $7, finished_at = $8,
                 coverage = $9, unknown_obligations = $10
             WHERE id = $11 AND tenant_id = $12 AND access_scope = $13 AND state = 'RUNNING'
                 AND (CAST($14 AS BIGINT) IS NULL OR (stage = 'ANALYZE' AND ",
            fenced!("$15", "$16", "$17", "$18"),
            "))"
        ))
        .bind(result.result_id.to_string())
        .bind(result_sha256.to_string())
        .bind(i64::from(usage.model_attempts))
        .bind(int(usage.input_tokens))
        .bind(int(usage.output_tokens))
        .bind(int(usage.cost.known.0))
        .bind(i64::from(usage.cost.unknown_cost_attempts))
        .bind(now.as_second())
        .bind(format!("{:?}", result.coverage))
        .bind(int(result
            .obligations
            .iter()
            .filter(|o| matches!(o.status, sych_core::ObligationStatus::Unknown { .. }))
            .count() as u64))
        .bind(run.id.to_string())
        .bind(&p.tenant_id)
        .bind(&p.access_scope)
        .bind(fence.map(|f| f.job_id))
        .bind(fence.map(|f| f.job_id))
        .bind(fence.map(|f| f.owner.clone()))
        .bind(fence.map(|f| f.epoch))
        .bind(now.as_second())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StorageError::Conflict(format!(
                "run {} is not RUNNING with frozen inputs under a current lease and generation",
                run.id
            )));
        }

        // A server run's job and its publication intent commit with the result.
        if let Some(fence) = fence {
            jobs::finish_job(&mut tx, fence, "DONE", None, now.as_second()).await?;
            outbox::enqueue(&mut tx, run, result.result_id, now.as_second()).await?;
        }

        tx.commit().await?;
        tracing::info!(run = %run.id, "run completed");
        Ok(())
    }

    /// Marks an active run FAILED; its artifacts become GC orphans.
    pub async fn fail_run(
        &self,
        run: &RunHandle,
        reason: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        tracing::warn!(run = %run.id, %reason, "run failed");
        self.end_run(run, "FAILED", reason, now).await
    }

    /// Marks an active run CANCELLED after an accepted user cancellation; no result is kept.
    pub async fn cancel_run(&self, run: &RunHandle, now: Timestamp) -> Result<(), StorageError> {
        self.end_run(run, "CANCELLED", "cancelled by user", now)
            .await
    }

    async fn end_run(
        &self,
        run: &RunHandle,
        state: &str,
        reason: &str,
        now: Timestamp,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "UPDATE runs SET state = $1, failure = $2, finished_at = $3
             WHERE id = $4 AND state = 'RUNNING'",
        )
        .bind(state)
        .bind(reason)
        .bind(now.as_second())
        .bind(run.id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// A handle to a saved local run, for work on its result (a tour) after it finished.
    pub async fn run_handle(&self, tenant_id: &str, run: RunId) -> Result<RunHandle, StorageError> {
        let row = sqlx::query(
            "SELECT access_scope, generation FROM runs WHERE id = $1 AND tenant_id = $2",
        )
        .bind(run.to_string())
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::UnknownRun(run))?;
        let generation: i64 = row.get(1);
        Ok(RunHandle {
            id: run,
            partition: Partition {
                tenant_id: tenant_id.to_owned(),
                access_scope: row.get(0),
            },
            generation: u64::try_from(generation).map_err(|_| {
                StorageError::Corrupt(format!("run {run} has generation {generation}"))
            })?,
            fence: None,
        })
    }

    pub async fn run_state(&self, tenant_id: &str, run: RunId) -> Result<String, StorageError> {
        let row = sqlx::query("SELECT state FROM runs WHERE id = $1 AND tenant_id = $2")
            .bind(run.to_string())
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|r| r.get(0)).ok_or(StorageError::UnknownRun(run))
    }

    /// Reads a saved result; never runs analysis.
    pub async fn load_result(
        &self,
        tenant_id: &str,
        run: RunId,
    ) -> Result<ReviewResult, StorageError> {
        let row = sqlx::query(
            "SELECT r.state, r.result_sha256, a.store_key FROM runs r
             LEFT JOIN artifacts a ON a.tenant_id = r.tenant_id
                 AND a.access_scope = r.access_scope AND a.sha256 = r.result_sha256
             WHERE r.id = $1 AND r.tenant_id = $2",
        )
        .bind(run.to_string())
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(StorageError::UnknownRun(run))?;
        let state: String = row.get(0);
        let digest: Option<String> = row.get(1);
        let key: Option<String> = row.get(2);
        let (Some(digest), true) = (digest, state == "COMPLETED") else {
            return Err(StorageError::NotCompleted { run, state });
        };
        self.read_document(&digest, key)
    }

    /// Reads a document pinned to a run under `role`; lost bytes are reported, never rebuilt.
    pub async fn load_artifact<T: DeserializeOwned>(
        &self,
        tenant_id: &str,
        run: RunId,
        role: &str,
    ) -> Result<T, StorageError> {
        let row = sqlx::query(
            "SELECT ra.sha256, a.store_key FROM runs r
             JOIN run_artifacts ra ON ra.run_id = r.id
             LEFT JOIN artifacts a ON a.tenant_id = ra.tenant_id
                 AND a.access_scope = ra.access_scope AND a.sha256 = ra.sha256
             WHERE r.id = $1 AND r.tenant_id = $2 AND ra.role = $3",
        )
        .bind(run.to_string())
        .bind(tenant_id)
        .bind(role)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StorageError::Corrupt(format!("run {run} has no `{role}` artifact")))?;
        let digest: String = row.get(0);
        let key: Option<String> = row.get(1);
        self.read_document(&digest, key)
    }

    fn read_document<T: DeserializeOwned>(
        &self,
        digest: &str,
        key: Option<String>,
    ) -> Result<T, StorageError> {
        let digest: Sha256Digest = digest
            .parse()
            .map_err(|e: sych_core::FormatError| StorageError::Corrupt(e.to_string()))?;
        let key = key.ok_or_else(|| StorageError::ArtifactUnavailable(digest.clone()))?;
        let bytes = self.store.get(&key, &digest)?;
        let text = String::from_utf8(bytes).map_err(|e| StorageError::Corrupt(e.to_string()))?;
        Ok(decode(&text)?)
    }

    pub(crate) fn store(&self) -> &ArtifactStore {
        &self.store
    }
}
