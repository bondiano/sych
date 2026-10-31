//! Storing a run: the durable session around analysis, its artifacts and terminal state.

use std::fmt::Display;

use serde::Serialize;

use sych_core::broker::ContextPackets;
use sych_core::{Evidence, ExternalInputManifest, SchemaVersion, Sha256Digest, Timestamp};
use sych_storage::{
    ArtifactKind, Partition, RunHandle, Storage, StorageError, StoreConfig, TENANT_LOCAL,
};
use tokio::{signal, time};

use super::{Saved, Session, Stopped, Target};

impl Session<'_> {
    pub(super) fn evidence(&self) -> impl Iterator<Item = &Evidence> {
        self.script
            .evidence
            .iter()
            .chain(&self.derived)
            .chain(&self.external)
    }

    /// One runtime for the whole storage session; a failed or cancelled run never becomes
    /// COMPLETED. A claimed server run is already recorded; it stops when `cancel` flips and
    /// every write it makes carries its fence.
    pub(super) async fn save(self, store: &StoreConfig, target: Target) -> Result<Saved, Stopped> {
        let failed = |e: &dyn Display| Stopped::Failed(e.to_string());
        let storage = Storage::open(store).await.map_err(|e| failed(&e))?;
        let (run, mut cancel) = match target {
            Target::Local { change_key } => {
                let partition = Partition {
                    tenant_id: TENANT_LOCAL.into(),
                    access_scope: self.snap.snapshot.repository.clone(),
                };
                let run = storage
                    .begin_run(
                        &partition,
                        &change_key,
                        &self.snap.snapshot.id,
                        &self.policy.id,
                        self.now,
                    )
                    .await
                    .map_err(|e| failed(&e))?;
                (run, None)
            }
            Target::Claimed { run, cancel } => (run, Some(cancel)),
        };
        let cancelled = async {
            match &mut cancel {
                // Cancelled when the flag flips or its sender (the worker) is gone.
                Some(cancel) => {
                    while !*cancel.borrow_and_update() {
                        if cancel.changed().await.is_err() {
                            break;
                        }
                    }
                }
                // shortcut: the handler is installed on first poll, so Ctrl-C between
                // `begin_run` and here kills the process and leaves the run RUNNING until a
                // server reconciles it.
                None => {
                    signal::ctrl_c().await.ok();
                }
            }
        };
        let saved = tokio::select! {
            saved = self.analyze_and_store(&storage, &run) => saved.map_err(|e| Stopped::Failed(format!("{e:#}"))),
            () = cancelled => Err(Stopped::Cancelled),
        };
        let marked = match (&saved, &run.fence) {
            // The scheduler already ended a claimed run it cancelled or superseded.
            (Ok(_), _) | (Err(Stopped::Cancelled), Some(_)) => Ok(()),
            (Err(Stopped::Failed(err)), Some(fence)) => storage
                .fail_claimed(fence, &run.id, err, Timestamp::now())
                .await
                .map(drop),
            (Err(Stopped::Failed(err)), None) => {
                storage.fail_run(&run, err, Timestamp::now()).await
            }
            (Err(Stopped::Cancelled), None) => storage.cancel_run(&run, Timestamp::now()).await,
        };
        if let Err(mark) = marked {
            tracing::error!(run = %run.id, error = %mark, "cannot mark run terminal");
        }

        saved
    }

    /// With the fake adapter and no fake responses there are no assessments, so every
    /// applicable obligation stays `UNKNOWN/NOT_ASSESSED` instead of being invented.
    async fn analyze_and_store(&self, storage: &Storage, run: &RunHandle) -> anyhow::Result<Saved> {
        time::sleep(self.script.delay).await;
        let packets = self.packets(run);
        let analysis = match self.model {
            Some(model) => Some(self.analyze(storage, run, model, &packets).await?),
            None => None,
        };
        // A parent that never completed has no result to link; reuse still read its checkpoints.
        let parent = match self.parent {
            Some(id) => storage.load_result(&run.partition.tenant_id, id).await.ok(),
            None => None,
        };
        let parent = parent.map(|r| r.result_id);
        let (external, evidence_manifest, result, evidence) =
            self.result(run, parent, &packets, analysis.as_ref())?;
        let now = self.now;
        self.store_inputs(storage, run, &external).await?;
        for ev in &evidence {
            let role = format!("evidence/{}", ev.id);
            put(storage, run, &role, ArtifactKind::Evidence, ev, now).await?;
        }
        put(storage, run, "facts", ArtifactKind::Facts, self.facts, now).await?;
        if let Some(analysis) = &analysis {
            let record = &analysis.outcome.record;
            put(
                storage,
                run,
                "analysis",
                ArtifactKind::Analysis,
                record,
                now,
            )
            .await?;
        }

        let kind = ArtifactKind::ContextPackets;
        let document = ContextPackets {
            schema_version: SchemaVersion,
            packets,
        };
        put(storage, run, "context_packets", kind, &document, now).await?;
        let kind = ArtifactKind::EvidenceManifest;
        put(
            storage,
            run,
            "evidence_manifest",
            kind,
            &evidence_manifest,
            now,
        )
        .await?;
        let kind = ArtifactKind::ReviewResult;
        let digest = put(storage, run, "review_result", kind, &result, now).await?;
        storage
            .complete_run(run, &result, &digest, Timestamp::now())
            .await?;
        Ok(Saved {
            external,
            evidence_manifest,
            evidence,
            packets: document.packets,
            result,
            candidates: analysis.map(|a| a.outcome.candidates).unwrap_or_default(),
        })
    }

    /// Snapshot, diff, policy and external inputs, the same for model and fake runs.
    async fn store_inputs(
        &self,
        storage: &Storage,
        run: &RunHandle,
        external: &ExternalInputManifest,
    ) -> anyhow::Result<()> {
        let now = self.now;
        let snap = self.snap;
        put(
            storage,
            run,
            "source_snapshot",
            ArtifactKind::SourceSnapshot,
            &snap.snapshot,
            now,
        )
        .await?;
        put(
            storage,
            run,
            "diff_manifest",
            ArtifactKind::DiffManifest,
            &snap.manifest,
            now,
        )
        .await?;
        put(
            storage,
            run,
            "policy_snapshot",
            ArtifactKind::PolicySnapshot,
            self.policy,
            now,
        )
        .await?;
        let review = self.policy_review;
        put(
            storage,
            run,
            "policy_review",
            ArtifactKind::PolicySnapshot,
            review,
            now,
        )
        .await?;
        let kind = ArtifactKind::ExternalInputManifest;
        put(storage, run, "external_input_manifest", kind, external, now).await?;
        Ok(())
    }
}

async fn put<T: Serialize>(
    storage: &Storage,
    run: &RunHandle,
    role: &str,
    kind: ArtifactKind,
    value: &T,
    now: Timestamp,
) -> Result<Sha256Digest, StorageError> {
    storage.put_artifact(run, role, kind, value, now).await
}
