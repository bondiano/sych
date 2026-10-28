//! The worker half of the server (spec §8.2–8.4): claim a job, fetch the pinned commits, wait
//! for expected CI until its deadline, freeze the inputs and review under the lease. No SQL
//! transaction is held across git, model or SCM calls.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::json;
use sych_adapters::git::{SnapshotError, fetch_commits};
use sych_adapters::github::Secret;
use sych_core::config::{ExpectedReport, RuleException};
use sych_core::glob_match;
use sych_core::schedule::{CI_WAIT_SECONDS, Trigger, retry_at};
use sych_core::scm::ScmErrorKind;
use sych_storage::Claim;
use tokio::sync::{oneshot, watch};
use tokio::time;

use super::ci::{Collected, Head};
use super::engine::{AFTER_EVIDENCE_SECONDS, Engine};
use super::mirrors;
use crate::args::{Comparison, ExitPolicy, Output, ReviewArguments};
use crate::render::Format;
use crate::review;

const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// What the PREPARE step found at the target tip's policy.
struct Prepared {
    expected: Vec<ExpectedReport>,
    wait_seconds: u32,
}

impl Engine {
    fn mirror(&self, connection: &str, repo: &str) -> PathBuf {
        mirrors::path(&self.store.artifacts, connection, repo)
    }

    fn arguments(&self, mirror: PathBuf, claim: &Claim) -> ReviewArguments {
        ReviewArguments {
            change: None,
            comparison: Comparison {
                repo: mirror,
                base: claim.inputs.base_sha.clone(),
                head: claim.inputs.head_sha.clone(),
                diff_base: None,
            },
            config: Some(self.review.config.clone()),
            models: self.review.models.clone(),
            policy_ref: None,
            fake_responses: None,
            semantic: self.review.semantic,
            ci_facts: Vec::new(),
            ci_manifest: None,
            exit_policy: ExitPolicy::Operational,
            out: Output {
                format: Format::Json,
                output: None,
            },
            risk_audit: true,
            verify: true,
            ci_imported: Vec::new(),
            ci_gaps: Vec::new(),
            checks: claim.inputs.targeted.clone(),
            execution: None,
            waivers: Vec::new(),
            run_mode: self.review.mode,
            deep: matches!(claim.inputs.trigger, Trigger::Manual { deep: true, .. }),
            parent: match &claim.inputs.trigger {
                Trigger::Successor { parent, .. } => parent.parse().ok().map(sych_core::RunId),
                _ => None,
            },
        }
    }

    /// Claims and works one job; `false` when nothing was ready.
    pub async fn work_once(&self) -> anyhow::Result<bool> {
        let storage = &self.app.storage;
        let Some(claim) = storage
            .claim(&self.owner, self.lease_seconds, (self.now)())
            .await?
        else {
            return Ok(false);
        };
        tracing::info!(run = %claim.run.id, epoch = claim.fence.epoch, stage = %claim.stage, "job claimed");
        if let Err(e) = self.work(&claim).await {
            let reason = format!("{e:#}");
            tracing::warn!(run = %claim.run.id, error = %reason, "job failed");
            storage
                .fail_claimed(&claim.fence, &claim.run.id, &reason, (self.now)())
                .await?;
            // A failure is published as one where checks are on, never as silence.
            storage
                .enqueue_failure_check(&claim.run, (self.now)().as_second())
                .await?;
        }

        Ok(true)
    }

    /// The whole claim is held: preparation under [`Self::holding`], analysis in
    /// [`Self::analyze`].
    async fn work(&self, claim: &Claim) -> anyhow::Result<()> {
        match self.holding(claim, self.prepare(claim)).await? {
            Some(args) => self.analyze(claim, args).await,
            None => Ok(()),
        }
    }

    /// Runs `step` while heartbeats keep the lease; `None` when the lease is lost first, since
    /// another worker may already hold the job. Both are polled together: `step` may hold the
    /// store's only connection while a heartbeat waits for it.
    async fn holding<T>(
        &self,
        claim: &Claim,
        step: impl Future<Output = anyhow::Result<Option<T>>>,
    ) -> anyhow::Result<Option<T>> {
        let lost = async {
            loop {
                time::sleep(self.heartbeat).await;
                let now = (self.now)();
                if !self
                    .app
                    .storage
                    .heartbeat(&claim.fence, self.lease_seconds, now)
                    .await?
                {
                    return anyhow::Ok(());
                }
            }
        };
        tokio::select! {
            out = step => out,
            lost = lost => {
                lost?;
                tracing::warn!(run = %claim.run.id, "lease lost; abandoning preparation");
                Ok(None)
            }
        }
    }

    /// Fetches, pins and collects CI; the arguments of the analysis when it is due now.
    async fn prepare(&self, claim: &Claim) -> anyhow::Result<Option<ReviewArguments>> {
        let storage = &self.app.storage;
        let change = storage.change(claim.change_id).await?;
        let conn = self.conns.get(&change.connection_id).ok_or_else(|| {
            anyhow::anyhow!("connection `{}` is not configured", change.connection_id)
        })?;
        let mirror = self.mirror(&conn.config.id, &change.full_name);
        let token = match self.token(conn, &change).await {
            Ok(token) => token,
            Err(e) => {
                return self
                    .retry_or_fail(claim, e.kind, e.retry_after_seconds, &e.to_string())
                    .await
                    .map(|()| None);
            }
        };
        let url = format!(
            "{}/{}.git",
            conn.config.git_base_url.trim_end_matches('/'),
            change.full_name
        );
        if let Err(e) = fetch(&mirror, url, &token, claim).await? {
            // A fork head may be gone; anything else at the transport is worth a retry.
            return self
                .retry_or_fail(claim, ScmErrorKind::Transient, None, &e.to_string())
                .await
                .map(|()| None);
        }

        if let Err(e) = mirrors::touch(&mirror, (self.now)().as_second()) {
            tracing::warn!(error = %e, "cannot record the mirror's use");
        }

        let args = self.arguments(mirror, claim);
        let (args, pinned) = blocking(move || {
            let pinned = review::pin(&args).and_then(|p| Ok((p.policy()?, p)));
            (args, pinned)
        })
        .await?;
        let (policy, pinned) = pinned?;
        let prepared = expected_sources(&pinned);
        let ids: Vec<&str> = prepared.expected.iter().map(|r| r.id.as_str()).collect();
        let now = (self.now)().as_second();
        let deadline = match claim.evidence_deadline {
            Some(deadline) => Some(deadline),
            None if prepared.expected.is_empty() => None,
            None => Some(now + i64::from(prepared.wait_seconds)),
        };
        let expected = json!({ "expected": ids });
        if let Some(deadline) = deadline {
            // Retries stay possible after the whole CI wait the policy allows.
            storage
                .extend_deadline(
                    &claim.fence,
                    deadline + AFTER_EVIDENCE_SECONDS,
                    (self.now)(),
                )
                .await?;
        }

        let current = storage
            .prepare_run(
                &claim.fence,
                &claim.run,
                &pinned.snap.snapshot.id,
                &policy.id,
                &expected,
                deadline,
                (self.now)(),
            )
            .await?;
        if !current {
            return Ok(None);
        }

        let collected = Head {
            conn,
            repo: &change.full_name,
            sha: &claim.inputs.head_sha,
            token: &token,
            snap: &pinned.snap,
            mirror: &args.comparison.repo,
        }
        .collect(&prepared.expected)
        .await;
        if let Some(deadline) = deadline.filter(|d| *d > now && collected.pending()) {
            storage
                .release(&claim.fence, deadline, None, (self.now)())
                .await?;
            tracing::info!(run = %claim.run.id, deadline, "waiting for evidence");
            return Ok(None);
        }

        let sources = frozen_sources(&ids, &collected);
        if !storage
            .freeze(&claim.fence, &claim.run.id, &sources, (self.now)())
            .await?
        {
            return Ok(None);
        }

        let mut args = args;
        args.ci_gaps = collected.gaps.iter().map(ToString::to_string).collect();
        args.ci_imported = collected.facts;
        args.waivers = self.waivers(change.id, now).await?;
        Ok(Some(args))
    }

    async fn waivers(&self, change: i64, at: i64) -> anyhow::Result<Vec<(String, RuleException)>> {
        let active = self.app.storage.active_waivers(change, at).await?;
        Ok(active
            .into_iter()
            .map(|w| (w.rule_id, w.exception))
            .collect())
    }

    /// Reviews on a blocking thread while heartbeats keep the lease; a lost lease cancels it.
    async fn analyze(&self, claim: &Claim, args: ReviewArguments) -> anyhow::Result<()> {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (store, run) = (self.store.clone(), claim.run.clone());
        let (done_tx, mut done_rx) = oneshot::channel();
        thread::spawn(move || {
            let outcome = review::run_claimed(&args, store, run, cancel_rx).map(drop);
            let _ = done_tx.send(outcome);
        });
        let outcome = loop {
            tokio::select! {
                outcome = &mut done_rx => break outcome?,
                () = time::sleep(self.heartbeat) => {
                    let alive = self.app.storage.heartbeat(&claim.fence, self.lease_seconds, (self.now)()).await?;
                    if !alive {
                        tracing::warn!(run = %claim.run.id, "lease lost; cancelling analysis");
                        let _ = cancel_tx.send(true);
                    }
                }
            }
        };
        // The session already marked a failed run under its fence.
        if let Err(e) = outcome {
            tracing::warn!(run = %claim.run.id, error = %format!("{e:#}"), "analysis ended without a result");
            self.app
                .storage
                .enqueue_failure_check(&claim.run, (self.now)().as_second())
                .await?;
        }

        Ok(())
    }

    async fn retry_or_fail(
        &self,
        claim: &Claim,
        kind: ScmErrorKind,
        retry_after: Option<u64>,
        error: &str,
    ) -> anyhow::Result<()> {
        let now = (self.now)().as_second();
        match retry_at(kind, claim.attempts, retry_after, 0, now, claim.deadline) {
            Some(at) => {
                tracing::info!(run = %claim.run.id, at, error, "job retried later");
                self.app
                    .storage
                    .release(&claim.fence, at, Some(error), (self.now)())
                    .await?;
                Ok(())
            }
            None => Err(anyhow::anyhow!("{error}")),
        }
    }
}

/// Expected CI reports whose `applies_to` matches a changed path (spec §15.2); default wait.
/// What a frozen run records about its CI sources: expected, imported and missing.
fn frozen_sources(ids: &[&str], collected: &Collected) -> serde_json::Value {
    let imported: Vec<_> = collected
        .facts
        .iter()
        .filter_map(|f| f.origin.as_ref())
        .map(|o| {
            json!({"report": o.report_path, "run": o.run_id, "attempt": o.run_attempt,
            "job": o.job_id, "artifact": o.artifact_id, "binding": o.binding})
        })
        .collect();
    let missing: Vec<&str> = collected.gaps.iter().map(|g| g.report.as_str()).collect();
    json!({"expected": ids, "imported": imported, "gaps": collected.gaps, "missing": missing})
}

fn expected_sources(pinned: &review::Pinned) -> Prepared {
    let ci = &pinned.effective.repository.config.ci;
    let paths: Vec<&str> = pinned
        .snap
        .manifest
        .files
        .iter()
        .filter_map(|f| f.new.as_ref().or(f.old.as_ref()))
        .map(|side| side.path.as_str())
        .collect();
    let expected = ci
        .expected_reports
        .iter()
        .filter(|r| {
            r.applies_to.is_empty()
                || r.applies_to
                    .iter()
                    .any(|g| paths.iter().any(|p| glob_match(g, p)))
        })
        .cloned()
        .collect();
    Prepared {
        expected,
        wait_seconds: if ci.wait_seconds == 0 {
            CI_WAIT_SECONDS
        } else {
            ci.wait_seconds
        },
    }
}

/// Fetches the head and target commits of the claim into the mirror.
async fn fetch(
    mirror: &Path,
    url: String,
    token: &Secret,
    claim: &Claim,
) -> anyhow::Result<Result<(), SnapshotError>> {
    let commits = [claim.inputs.head_sha.clone(), claim.inputs.base_sha.clone()];
    let (mirror, token) = (mirror.to_owned(), token.expose().to_owned());
    blocking(move || {
        let commits: Vec<&str> = commits.iter().map(String::as_str).collect();
        fetch_commits(&mirror, &url, Some(&token), &commits, FETCH_TIMEOUT)
    })
    .await
}

/// Runs blocking work (git, snapshot) off the async thread.
pub(super) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<T> {
    let (tx, rx) = oneshot::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    Ok(rx.await?)
}
