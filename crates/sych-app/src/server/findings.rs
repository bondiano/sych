//! Finding lineage after each committed revision, and feedback and waivers by authorized
//! actors.

use std::fmt::Display;

use sych_adapters::github::TokenScope;
use sych_core::RunId;
use sych_core::config::RuleException;
use sych_core::lineage::decide;
use sych_core::publication::problems;
use sych_core::scm::{ChangeSignal, WebhookEvent};
use sych_storage::{ChangeRow, Intent};

use super::engine::{Conn, Engine};

/// Repository permission an actor needs for feedback and for waivers.
pub const FEEDBACK_PERMISSIONS: &[&str] = &["admin", "maintain", "write", "triage"];
pub const WAIVER_PERMISSIONS: &[&str] = &["admin", "maintain"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refused {
    #[error("{actor} has `{level}` on the repository; {needed} is needed")]
    Permission {
        actor: String,
        level: String,
        needed: String,
    },
    #[error("{0}")]
    Unavailable(String),
}

impl Engine {
    /// Applies lineage of every committed revision not applied yet, in generation order.
    pub async fn lineage_once(&self) -> anyhow::Result<usize> {
        let storage = &self.app.storage;
        let pending = storage.lineage_pending().await?;
        for (change, run, tenant) in &pending {
            let id = RunId(run.parse()?);
            let result = storage.load_result(tenant, id).await?;
            let manifest = storage.load_artifact(tenant, id, "diff_manifest").await?;
            let prior = storage.open_findings(*change).await?;
            let decisions = decide(&prior, &result, &manifest);
            let at = (self.now)().as_second();
            // In generation order: a later revision's decisions read this one's findings.
            storage
                .apply_lineage(*change, run, &result, &decisions, at)
                .await?;
        }
        Ok(pending.len())
    }

    /// `login`'s level on `connection`'s `full_name`, read from GitHub now.
    pub(super) async fn level(
        &self,
        connection: &str,
        full_name: &str,
        login: &str,
    ) -> Result<String, Refused> {
        let unavailable = |e: &dyn Display| Refused::Unavailable(e.to_string());
        let conn = self
            .conns
            .get(connection)
            .ok_or_else(|| Refused::Unavailable("connection is not configured".into()))?;
        let now = (self.now)().as_second();
        let installation = conn
            .gh
            .installation(full_name, now)
            .await
            .map_err(|e| unavailable(&e))?;
        let token = conn
            .gh
            .token(installation.id, full_name, TokenScope::Read, now)
            .await
            .map_err(|e| unavailable(&e))?;
        conn.gh
            .permission(full_name, login, &token)
            .await
            .map_err(|e| unavailable(&e))
    }

    /// The actor's repository permission, if it is one of `allowed`.
    async fn authorize(
        &self,
        change_id: i64,
        actor: &str,
        allowed: &[&str],
    ) -> Result<(), Refused> {
        let change = self
            .app
            .storage
            .change(change_id)
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))?;
        let level = self
            .level(&change.connection_id, &change.full_name, actor)
            .await?;
        if allowed.contains(&level.as_str()) {
            Ok(())
        } else {
            Err(Refused::Permission {
                actor: actor.into(),
                level,
                needed: allowed.join("|"),
            })
        }
    }

    /// Structured feedback on a finding by an actor with at least triage access.
    #[allow(clippy::too_many_arguments)] // one feedback request
    pub async fn feedback(
        &self,
        change_id: i64,
        finding: i64,
        actor: &str,
        judgement: &str,
        scope: &str,
        rationale: &str,
    ) -> Result<(), Refused> {
        self.authorize(change_id, actor, FEEDBACK_PERMISSIONS)
            .await?;
        let tenant = self
            .app
            .storage
            .change(change_id)
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))?
            .partition
            .tenant_id;
        let at = (self.now)().as_second();
        self.app
            .storage
            .record_feedback(&tenant, finding, actor, judgement, scope, rationale, at)
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))
    }

    /// The `/sych dismiss N reason` lines of `intent`'s comment by `actor`, who has write access
    /// as for every command: each problem N of the latest summary, as numbered there, is
    /// dismissed as a false positive with its reason, then the summary is rendered again
    /// without them. A line that names no problem, or gives no reason, is logged and skipped.
    pub(super) async fn dismiss(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        intent: &Intent,
        actor: Option<&str>,
    ) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let refused = async |why: String| {
            storage
                .log_event(change.id, None, "DISMISS_REFUSED", &why, now)
                .await
        };
        let Some(actor) = actor else {
            let skipped = "UNAUTHORIZED_COMMAND";
            return Ok(storage
                .log_event(change.id, None, "SKIPPED", skipped, now)
                .await?);
        };
        let Ok(WebhookEvent {
            signal: Some(ChangeSignal::Command { argument, .. }),
            ..
        }) = storage.inbox_event(intent.last_inbox_id).await
        else {
            return Ok(());
        };
        let Some(summary) = storage.delivered_summary(change.id).await? else {
            return Ok(refused("no summary to dismiss from".into()).await?);
        };
        // The latest revision's findings get their rows before one is dismissed.
        self.lineage_once().await?;
        let (result, _) = self.load(&summary).await?;
        let shown = problems(&result);
        let rows = storage.findings_of(change.id).await?;
        for line in argument.lines() {
            let (n, reason) = line.split_once(' ').unwrap_or((line, ""));
            let row = (n.parse::<usize>().ok())
                .and_then(|n| shown.get(n.checked_sub(1)?))
                .and_then(|f| {
                    rows.iter()
                        .find(|r| r.fingerprint == f.fingerprint.to_string())
                });
            let done = match row {
                None => Err(format!("`{n}`: no such problem in the summary")),
                Some(_) if reason.trim().is_empty() => Err(format!("`{n}`: no reason given")),
                Some(row) => self
                    .feedback(
                        change.id,
                        row.id,
                        actor,
                        "FALSE_POSITIVE",
                        "finding",
                        reason.trim(),
                    )
                    .await
                    .map_err(|e| format!("`{n}`: {e}")),
            };
            if let Err(why) = done {
                refused(why).await?;
            }
        }
        if let Err(e) = self.refresh_summary(conn, change).await {
            tracing::warn!(change = change.id, error = %format!("{e:#}"), "summary not refreshed after a dismiss");
        }

        Ok(())
    }

    /// A scoped waiver of `rule` by a maintainer; it lowers the risk decision of later runs.
    pub async fn waive(
        &self,
        change_id: i64,
        rule: &str,
        exception: &RuleException,
    ) -> Result<i64, Refused> {
        self.authorize(change_id, &exception.author, WAIVER_PERMISSIONS)
            .await?;
        let storage = &self.app.storage;
        let change = storage
            .change(change_id)
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))?;
        let policy = storage
            .latest_policy(change_id)
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))?;
        let at = (self.now)().as_second();
        storage
            .add_waiver(
                &change.partition.tenant_id,
                change_id,
                rule,
                exception,
                policy.as_deref(),
                at,
            )
            .await
            .map_err(|e| Refused::Unavailable(e.to_string()))
    }
}
