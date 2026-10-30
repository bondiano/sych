//! Checkpoints of a run (spec §8.5): each accepted model answer is pinned under the run's
//! fence as it arrives, so a successor can reuse it even when this run is superseded
//! mid-analysis. A successor reads its parent's only within its own partition, so a matching
//! content hash never crosses a tenant or access scope.

use std::cell::RefCell;

use serde::de::DeserializeOwned;
use sqlx::Row;
use sych_core::model::{Checkpoint, Denied, Ledger, LedgerError, Limits, Reservation, Settlement};
use sych_core::{AttemptId, RunId, Timestamp};

use crate::{ArtifactKind, Partition, RunHandle, Storage, StorageError};

/// The role prefix of every checkpoint artifact.
pub const CHECKPOINT_ROLE: &str = "checkpoint/";

/// The ledger of one run that also keeps its checkpoints. Once a write shows the run is no
/// longer worked under its fence, it reserves no further model call: that result could
/// never be committed.
pub struct RunLedger<'a> {
    storage: &'a Storage,
    run: &'a RunHandle,
    now: fn() -> Timestamp,
    lost: RefCell<Option<String>>,
    scope: Option<String>,
}

impl<'a> RunLedger<'a> {
    pub fn new(storage: &'a Storage, run: &'a RunHandle, now: fn() -> Timestamp) -> Self {
        Self {
            storage,
            run,
            now,
            lost: RefCell::default(),
            scope: None,
        }
    }

    /// Run caps count only the attempts of `scope` (one follow-up session).
    #[must_use]
    pub fn scoped(self, scope: String) -> Self {
        Self {
            scope: Some(scope),
            ..self
        }
    }
}

impl Ledger for RunLedger<'_> {
    async fn reserve(
        &self,
        reservation: &Reservation,
        limits: &Limits,
    ) -> Result<Result<(), Denied>, LedgerError> {
        if let Some(reason) = self.lost.borrow().clone() {
            return Err(LedgerError(reason));
        }

        self.storage
            .reserve_scoped(reservation, limits, self.scope.as_deref())
            .await
            .map_err(|e| LedgerError(e.to_string()))
    }

    async fn settle(
        &self,
        attempt: AttemptId,
        settlement: &Settlement,
        at: Timestamp,
    ) -> Result<(), LedgerError> {
        Ledger::settle(self.storage, attempt, settlement, at).await
    }

    /// One retry for a store failure; a lost fence is final.
    async fn checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), LedgerError> {
        let role = format!("{CHECKPOINT_ROLE}{}", checkpoint.key);
        let mut failure = String::new();
        for _ in 0..2 {
            let kind = ArtifactKind::Checkpoint;
            let now = (self.now)();
            match self
                .storage
                .put_artifact(self.run, &role, kind, checkpoint, now)
                .await
            {
                Ok(_) => return Ok(()),
                Err(StorageError::Conflict(reason)) => {
                    self.lost.replace(Some(reason.clone()));
                    return Err(LedgerError(reason));
                }
                Err(e) => failure = e.to_string(),
            }
        }
        Err(LedgerError(failure))
    }
}

impl Storage {
    /// Every document `run` pinned under a role starting with `prefix`, in `partition` only.
    pub async fn load_artifacts<T: DeserializeOwned>(
        &self,
        partition: &Partition,
        run: RunId,
        prefix: &str,
    ) -> Result<Vec<T>, StorageError> {
        let rows = sqlx::query(
            "SELECT ra.sha256, a.store_key FROM runs r
             JOIN run_artifacts ra ON ra.run_id = r.id
             LEFT JOIN artifacts a ON a.tenant_id = ra.tenant_id
                 AND a.access_scope = ra.access_scope AND a.sha256 = ra.sha256
             WHERE r.id = $1 AND r.tenant_id = $2 AND r.access_scope = $3
                 AND substr(ra.role, 1, length($4)) = $5",
        )
        .bind(run.to_string())
        .bind(&partition.tenant_id)
        .bind(&partition.access_scope)
        .bind(prefix)
        .bind(prefix)
        .fetch_all(self.pool())
        .await?;
        rows.iter()
            .map(|row| self.read_document(row.get(0), row.get(1)))
            .collect()
    }
}
