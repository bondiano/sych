//! Storing a run: the durable session around analysis, its artifacts and terminal state.

use std::fmt::Display;

use serde::Serialize;

use sych_adapters::git::LocalSnapshot;
use sych_core::analysis::{Discovered, PriorRevisionContext};
use sych_core::broker::{ContextPackets, EvidencePacket, Limits};
use sych_core::config::{Budgets, Rule, RuleException};
use sych_core::db_schema::DbSchemaConfig;
use sych_core::{
    Candidate, Evidence, EvidenceManifest, ExternalInputManifest, PolicySnapshot, ReviewPlan,
    ReviewResult, RunFacts, SchemaVersion, Sha256Digest, Timestamp,
};
use sych_storage::{
    ArtifactKind, LocalHeadPin, Partition, RunHandle, Storage, StorageError, StoreConfig,
    TENANT_LOCAL,
};
use tokio::{signal, time};

use super::Target;
use crate::backend::Routing;
use crate::fake::Scripted;
use crate::render::PolicyReview;

pub(super) struct Saved {
    pub(super) external: ExternalInputManifest,
    pub(super) evidence_manifest: EvidenceManifest,
    pub(super) evidence: Vec<Evidence>,
    pub(super) packets: Vec<EvidencePacket>,
    pub(super) result: ReviewResult,
    /// Every candidate of the model passes, verified or not.
    pub(super) candidates: Vec<Candidate>,
    /// Packets a finished general discovery searched, by index into `packets`; an
    /// obligation's risk audit looks for that rule only, so it does not count.
    pub(super) searched: Vec<usize>,
}

pub(super) enum Stopped {
    Failed(String),
    Cancelled,
}

pub(super) struct Session<'a> {
    pub(super) snap: &'a LocalSnapshot,
    pub(super) policy: &'a PolicySnapshot,
    pub(super) policy_review: &'a PolicyReview,
    pub(super) plan: &'a ReviewPlan,
    pub(super) script: Scripted,
    pub(super) facts: &'a RunFacts,
    /// Evidence derived in this run from `facts`.
    pub(super) derived: Vec<Evidence>,
    /// Imported CI reports, frozen as external inputs.
    pub(super) external: Vec<Evidence>,
    pub(super) limits: Limits,
    /// The resolved route plan; `None` runs the fake adapter.
    pub(super) model: Option<&'a Routing>,
    pub(super) budgets: &'a Budgets,
    pub(super) risk_audit: bool,
    pub(super) verify: bool,
    /// `context.expansion_rounds` of the trusted policy.
    pub(super) expansion_rounds: u32,
    /// `--intent-file`, else the change's commit messages: for the discovery, review and
    /// expansion prompts.
    pub(super) intent: Option<String>,
    /// `eval measure --replay`: these discoveries are verified again instead of a new search.
    pub(super) replay: Option<&'a [Discovered]>,
    /// Expected CI reports that are not evidence; each is a task gap.
    pub(super) ci_gaps: &'a [String],
    /// The parent run whose checkpoints may answer this run's tasks.
    pub(super) parent: Option<sych_core::RunId>,
    /// `review.language` of the trusted policy.
    pub(super) language: sych_core::ReviewLanguage,
    pub(super) fixture_paths: &'a [String],
    /// The `security` profile: added lines are scanned for secrets and hidden text, and
    /// discovery also hunts security defects.
    pub(super) security: bool,
    /// `review.block_on` of the trusted policy.
    pub(super) block_on: &'a [String],
    /// The trusted repository rules: a required `ARCH-*` rule makes an architecture finding a
    /// rule violation.
    pub(super) rules: &'a [Rule],
    /// Server-recorded scoped waivers, also checked against built-in `DB-*` rules.
    pub(super) waivers: &'a [(String, RuleException)],
    /// Effective trusted schema linter config (inline `.sych/config.yaml` + optional `.sych/schema-lint.yaml`).
    pub(super) schema_config: DbSchemaConfig,
    /// Prior revision findings and dismissed rationales of this change (server runs).
    pub(super) prior_revision: &'a PriorRevisionContext,
    pub(super) now: Timestamp,
}

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
        let storage = if target.is_local_cli() {
            Storage::open_local(store).await
        } else {
            Storage::open(store).await
        }
        .map_err(|e| failed(&e))?;
        let (run, mut cancel) = match target {
            Target::Local {
                change_key,
                mut local_head,
                ..
            } => {
                let partition = Partition {
                    tenant_id: TENANT_LOCAL.into(),
                    access_scope: self.snap.snapshot.repository.clone(),
                };
                let pin = local_head
                    .as_ref()
                    .map(|head| -> Result<LocalHeadPin<'_>, Stopped> {
                        let git_dir = head.repository().to_str().ok_or_else(|| {
                            Stopped::Failed("local head Git directory is not UTF-8".into())
                        })?;
                        Ok(LocalHeadPin {
                            git_dir,
                            oid: head.commit(),
                        })
                    })
                    .transpose()?;
                let run = storage
                    .begin_run_with_local_head(
                        &partition,
                        &change_key,
                        &self.snap.snapshot.id,
                        &self.policy.id,
                        self.now,
                        pin,
                    )
                    .await
                    .map_err(|e| failed(&e))?;
                if let Some(head) = &mut local_head {
                    head.keep();
                }

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
        // A stopped analysis was dropped with its calls; none of them will settle now.
        if saved.is_err()
            && let Err(e) = storage.hold_unsettled(&run.id, Timestamp::now()).await
        {
            tracing::error!(run = %run.id, error = %e, "cannot hold unsettled attempts");
        }

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
        // Only general searches count: a risk audit looks for its rule alone. A replay searches
        // nothing, so the packets its frozen discoveries searched stand in.
        let discovered = analysis.iter().flat_map(|a| &a.outcome.record.discovered);
        let mut searched: Vec<usize> = discovered
            .chain(self.replay.unwrap_or_default())
            .filter(|d| d.audit.is_none())
            .flat_map(|d| d.batch.iter().copied())
            .collect();
        searched.sort_unstable();
        searched.dedup();
        Ok(Saved {
            external,
            evidence_manifest,
            evidence,
            packets: document.packets,
            result,
            candidates: analysis.map(|a| a.outcome.candidates).unwrap_or_default(),
            searched,
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
