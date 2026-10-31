//! Finding lineage after each committed revision, and feedback and waivers by authorized
//! actors.

use std::fmt::Display;

use sych_core::RunId;
use sych_core::config::RuleException;
use sych_core::lineage::decide;

use super::engine::Engine;

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
            storage
                .apply_lineage(*change, run, &result, &decisions, at)
                .await?;
        }
        Ok(pending.len())
    }

    /// The actor's repository permission, if it is one of `allowed`.
    async fn authorize(
        &self,
        change_id: i64,
        actor: &str,
        allowed: &[&str],
    ) -> Result<(), Refused> {
        let unavailable = |e: &dyn Display| Refused::Unavailable(e.to_string());
        let change = self
            .app
            .storage
            .change(change_id)
            .await
            .map_err(|e| unavailable(&e))?;
        let conn = self
            .conns
            .get(&change.connection_id)
            .ok_or_else(|| Refused::Unavailable("connection is not configured".into()))?;
        let token = self
            .token(conn, &change)
            .await
            .map_err(|e| unavailable(&e))?;
        let level = conn
            .gh
            .permission(&change.full_name, actor, &token)
            .await
            .map_err(|e| unavailable(&e))?;
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
