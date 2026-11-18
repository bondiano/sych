//! The publisher. A write happens only after the operation is durably `PUBLISHING` and both
//! gates pass; an unknown outcome is left to the reconciler.

use sych_adapters::github::{CheckOutput, Comment, MAX_PR_FILES, Secret, TokenScope};
use sych_core::publication::{
    InlineAnchor, Placement, SummaryContext, dismiss, gone_body, inline_body, plan, render_summary,
    revisit,
};
use sych_core::scm::{ScmError, ScmErrorKind};
use sych_core::{Candidate, DiffManifest, ReviewResult, RunId, Timestamp};
use sych_storage::{ChangeRow, INVALID_ANCHOR, ObjectKind, PublishClaim};

use super::engine::{Conn, Engine};

/// How long an unknown outcome is reconciled before it waits for the operator.
pub const UNCERTAIN_WINDOW: i64 = 3_600;
pub(super) const CHECK_NAME: &str = "Sych (advisory)";

/// The provider object a write produced or a reconciliation found.
pub(super) struct Written {
    pub(super) id: String,
    pub(super) body: String,
}

pub(super) fn bad(m: impl Into<String>) -> ScmError {
    ScmError::new(ScmErrorKind::UnsupportedCapability, m)
}

fn wait(e: &ScmError) -> i64 {
    e.retry_after_seconds
        .map_or(30, |s| i64::try_from(s).unwrap_or(30))
}

impl Engine {
    /// Publishes one due operation; `false` when none was ready.
    pub async fn publish_once(&self) -> anyhow::Result<bool> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let Some(c) = storage
            .claim_publication(&self.owner, self.lease_seconds, now)
            .await?
        else {
            return Ok(false);
        };
        let op = c.clone();
        if let Err(e) = self.publish(c, now).await {
            // Nothing was sent if the operation is still pending (a body that cannot be
            // rendered, a connection gone): count it and pause, doubling up to eight minutes.
            let reason = format!("{e:#}");
            tracing::warn!(op = op.op_id, error = %reason, "publication not sent");
            let at = now.as_second() + (30 << op.attempts.clamp(0, 4));
            storage.publication_unsent(&op, &reason, at).await?;
        }

        Ok(true)
    }

    /// Gates, renders and sends one claimed operation.
    async fn publish(&self, c: PublishClaim, now: Timestamp) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        if let Err(reason) = storage.publication_gate(&c).await? {
            let state = if reason == "SUPERSEDED" {
                "SUPERSEDED"
            } else {
                "CANCELLED"
            };
            storage
                .publication_end(c.op_id, state, &reason, now)
                .await?;
            return Ok(());
        }

        let change = storage.change(c.change_id).await?;
        let conn = self.conns.get(&change.connection_id).ok_or_else(|| {
            anyhow::anyhow!("connection `{}` is not configured", change.connection_id)
        })?;
        // The API gate: the change is open now, not only in the last webhook (a lost close).
        let gate = async {
            let token = self.token(conn, &change, TokenScope::Publish).await?;
            let pull = conn
                .gh
                .pull(&change.full_name, change.number, &token)
                .await?;
            Ok::<_, ScmError>((token, pull))
        };
        let (token, pull) = match gate.await {
            Ok(read) => read,
            Err(e) if e.kind.retryable() => {
                let at = now.as_second() + wait(&e);
                storage.publication_retry(&c, &e.to_string(), at).await?;
                return Ok(());
            }
            Err(e) => {
                storage
                    .publication_end(c.op_id, "FAILED", &e.to_string(), now)
                    .await?;
                return Ok(());
            }
        };
        if !pull.open {
            storage
                .publication_end(c.op_id, "CANCELLED", "CLOSED", now)
                .await?;
            return Ok(());
        }

        let check = c.kind == ObjectKind::Check;
        let caps = storage
            .repository_capabilities(&conn.binding, &change.repository_external_id)
            .await?;
        let need = if check {
            "checks_write"
        } else {
            "summary_write"
        };
        if caps.is_some_and(|caps| caps.missing(check).contains(&need)) {
            let reason = format!("UNSUPPORTED_CAPABILITY: {need}");
            storage
                .publication_end(c.op_id, "FAILED", &reason, now)
                .await?;
            return Ok(());
        }

        let complete = pull.changed_files <= MAX_PR_FILES as u64;
        if c.kind == ObjectKind::Inline && c.remote_id.is_none() {
            let mut claims = vec![c];
            let siblings = storage
                .claim_inline_siblings(&claims[0], self.lease_seconds, now)
                .await?;
            claims.extend(siblings);
            self.publish_review(conn, &change, claims, &token).await?;
            return Ok(());
        }

        let body = self.render(&c, &change.full_name, complete).await?;
        if storage.begin_publishing(&c, &body, now).await?.is_none() {
            return Ok(());
        }

        let reply = self.send(conn, &change, &c, &body, &token).await;
        self.settle(conn, &change, &c, reply).await?;
        Ok(())
    }

    /// What the previous revision left open; none when there is no parent or it cannot be read,
    /// which only lists every open question as new.
    async fn earlier(&self, tenant: &str, result: &ReviewResult) -> Vec<Candidate> {
        let Some(parent) = result.parent_result_id else {
            return Vec::new();
        };
        let storage = &self.app.storage;
        match storage.run_of_result(tenant, parent).await {
            Ok(Some(run)) => match storage.load_result(tenant, run).await {
                Ok(previous) => previous.unresolved_candidates,
                Err(err) => {
                    tracing::warn!(%err, "previous result unreadable; open questions all listed as new");
                    Vec::new()
                }
            },
            Ok(None) => Vec::new(),
            Err(err) => {
                tracing::warn!(%err, "previous result unreadable; open questions all listed as new");
                Vec::new()
            }
        }
    }

    /// The result of `c`'s run, with what the change dismissed marked so, and its diff.
    pub(super) async fn load(
        &self,
        c: &PublishClaim,
    ) -> anyhow::Result<(ReviewResult, DiffManifest)> {
        let run = RunId(c.run_id.parse()?);
        let tenant = &c.partition.tenant_id;
        let mut result = self.app.storage.load_result(tenant, run).await?;
        let rows = self.app.storage.findings_of(c.change_id).await?;
        let dismissed: Vec<String> = (rows.into_iter())
            .filter(|r| r.state == "DISMISSED")
            .map(|r| r.fingerprint)
            .collect();
        dismiss(&mut result, &dismissed);
        let manifest = self
            .app
            .storage
            .load_artifact(tenant, run, "diff_manifest")
            .await?;
        Ok((result, manifest))
    }

    /// The body of the claimed operation, from the immutable result. The summary also queues the
    /// inline comments of its revision before it is sent.
    pub(super) async fn render(
        &self,
        c: &PublishClaim,
        repo: &str,
        complete: bool,
    ) -> anyhow::Result<String> {
        let storage = &self.app.storage;
        if c.kind == ObjectKind::Check && c.result_id.is_none() {
            let gate = if c.block_on == Some(true) {
                " The rules `review.block_on` names were not checked, so the check fails."
            } else {
                ""
            };
            return Ok(format!(
                "Sych could not complete this review; the run failed before a result.{gate}"
            ));
        }

        let (result, manifest) = self.load(c).await?;
        if c.kind == ObjectKind::Inline {
            let fingerprint = c.fingerprint.as_deref().unwrap_or_default();
            let finding = result
                .findings
                .iter()
                .find(|f| f.fingerprint.to_string() == fingerprint);
            return match (finding, &c.remote_id) {
                (Some(finding), _) => Ok(inline_body(finding, result.language)),
                // A comment of an earlier revision whose finding this one no longer reports.
                (None, Some(_)) => {
                    let original = storage.delivered_body(c.object_id).await?;
                    let head = manifest.head.hex();
                    let original = original.unwrap_or_default();
                    Ok(gone_body(fingerprint, head, &original, result.language))
                }
                (None, None) => anyhow::bail!("finding {fingerprint:?} is not in the result"),
            };
        }

        let objects = storage.inline_objects(c.change_id).await?;
        let pick = |rejected: bool| -> Vec<String> {
            objects
                .iter()
                .filter(|(_, r)| *r == rejected)
                .map(|(f, _)| f.clone())
                .collect()
        };
        let planned = plan(&result, &manifest, &pick(false), &pick(true), complete);
        if c.kind == ObjectKind::Summary {
            let items: Vec<(String, String)> = planned
                .iter()
                .filter(|p| !p.already_inline)
                .filter_map(|p| match &p.placement {
                    Placement::Inline { anchor } => {
                        Some((p.fingerprint.clone(), serde_json::to_string(anchor).ok()?))
                    }
                    Placement::Summary { .. } => None,
                })
                .collect();
            let at = (self.now)().as_second();
            let Some(result_id) = c.result_id.as_deref() else {
                anyhow::bail!("summary operation {} has no result", c.op_id);
            };
            storage
                .enqueue_inline(c.change_id, &c.run_id, result_id, c.generation, &items, at)
                .await?;
            let posted = storage.posted_inline(c.change_id).await?;
            let edits = revisit(&posted, &result, &manifest);
            storage
                .enqueue_inline_edits(&edits, &c.run_id, result_id, c.generation, at)
                .await?;
        }

        let earlier = self.earlier(&c.partition.tenant_id, &result).await;
        let ctx = SummaryContext {
            earlier: &earlier,
            marker: &c.marker,
            repo: Some(repo),
            head_sha: manifest.head.hex(),
            base_sha: manifest.diff_base.hex(),
            result_url: self
                .public_base_url
                .as_ref()
                .map(|u| format!("{}/ui/runs/{}", u.trim_end_matches('/'), c.run_id)),
            duration_seconds: None,
        };
        Ok(render_summary(&result, &planned, &ctx))
    }

    pub(super) async fn send(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        c: &PublishClaim,
        body: &str,
        token: &Secret,
    ) -> Result<Written, ScmError> {
        let (gh, repo) = (&conn.gh, change.full_name.as_str());
        let id = |s: &String| {
            s.parse::<u64>()
                .map_err(|_| bad("stored remote id is not a number"))
        };
        let written = |id: u64| Written {
            id: id.to_string(),
            body: body.to_owned(),
        };
        let comment = |c: Comment| written(c.id);
        match (c.kind, &c.remote_id) {
            (ObjectKind::Summary, Some(r)) => gh
                .update_comment(repo, id(r)?, body, token)
                .await
                .map(comment),
            (ObjectKind::Summary, None) => gh
                .create_comment(repo, change.number, body, token)
                .await
                .map(comment),
            (ObjectKind::Inline, Some(r)) => gh
                .update_review_comment(repo, id(r)?, body, token)
                .await
                .map(comment),
            (ObjectKind::Inline, None) => {
                let anchor: InlineAnchor =
                    serde_json::from_str(c.anchor.as_deref().unwrap_or_default())
                        .map_err(|_| bad("inline object without an anchor"))?;
                gh.create_review_comment(repo, change.number, &anchor, body, token)
                    .await
                    .map(comment)
            }
            (ObjectKind::Check, remote) => {
                let failed = c.result_id.is_none();
                // Advisory unless `review.block_on` named a rule with an open finding: never
                // success, so no one reads a neutral run as a pass. A failed run under such a
                // policy checked none of them, so it fails closed: a required check passes on
                // neutral. A result that cannot be read now is retried, like any transient failure.
                let blocking = if failed {
                    c.block_on == Some(true)
                } else {
                    !self
                        .load(c)
                        .await
                        .map_err(|e| ScmError::new(ScmErrorKind::Transient, format!("{e:#}")))?
                        .0
                        .blocked_by
                        .is_empty()
                };
                let output = CheckOutput {
                    name: CHECK_NAME,
                    conclusion: if blocking { "failure" } else { "neutral" },
                    title: if failed {
                        "Sych could not complete this review"
                    } else if blocking {
                        "Blocked by review.block_on"
                    } else {
                        "Advisory review"
                    },
                    summary: body,
                };
                let head = change.head_sha.as_deref().unwrap_or_default();
                let run = match remote {
                    Some(r) => gh.update_check_run(repo, id(r)?, &output, token).await,
                    None => {
                        gh.create_check_run(repo, head, &c.marker, &output, token)
                            .await
                    }
                };
                run.map(|r| written(r.id))
            }
        }
    }

    pub(super) async fn settle(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        c: &PublishClaim,
        reply: Result<Written, ScmError>,
    ) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        match reply {
            Ok(w) => {
                storage
                    .publication_delivered(c, &w.id, &w.body, now)
                    .await?;
                tracing::info!(op = c.op_id, kind = c.kind.as_str(), remote = %w.id, "publication delivered");
            }
            Err(e) if e.kind == ScmErrorKind::PublicationUnknown => {
                tracing::warn!(op = c.op_id, error = %e, "publication outcome unknown");
                storage
                    .publication_uncertain(c.op_id, &e.to_string(), UNCERTAIN_WINDOW, now)
                    .await?;
            }
            // The provider refused the position: the finding moves to the summary instead.
            Err(e) if e.kind == ScmErrorKind::InvalidAnchor => {
                let reason = format!("{INVALID_ANCHOR}: {e}");
                storage
                    .publication_end(c.op_id, "FAILED", &reason, now)
                    .await?;
                self.refresh_summary(conn, change).await?;
            }
            // The object was deleted remotely: its absence is known, so creating is safe.
            Err(e) if e.kind == ScmErrorKind::NotFound && c.remote_id.is_some() => {
                storage.forget_remote(c.object_id).await?;
                storage
                    .publication_retry(c, &e.to_string(), now.as_second())
                    .await?;
            }
            Err(e) if e.kind.retryable() => {
                let at = now.as_second() + wait(&e);
                storage.publication_retry(c, &e.to_string(), at).await?;
            }
            Err(e) => {
                storage
                    .publication_end(c.op_id, "FAILED", &e.to_string(), now)
                    .await?;
            }
        }
        Ok(())
    }

    /// Re-renders a delivered summary in place, by its known id, after the plan changed.
    pub(super) async fn refresh_summary(
        &self,
        conn: &Conn,
        change: &ChangeRow,
    ) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let Some(c) = storage.delivered_summary(change.id).await? else {
            return Ok(());
        };
        let Some(remote) = c.remote_id.clone() else {
            return Ok(());
        };
        let body = self.render(&c, &change.full_name, true).await?;
        let token = self.token(conn, change, TokenScope::Publish).await?;
        conn.gh
            .update_comment(&change.full_name, remote.parse()?, &body, &token)
            .await?;
        storage.record_body(c.op_id, &body).await?;
        storage
            .log_event(change.id, None, "SUMMARY_REFRESHED", &remote, (self.now)())
            .await?;
        Ok(())
    }
}
