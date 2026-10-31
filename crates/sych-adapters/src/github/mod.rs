//! GitHub App adapter: webhook admission checks, App credentials and the
//! normalized read operations. Writes and reconciliation build on [`Github`].

mod actions;
mod app;
mod client;
mod comments;
mod reviews;
pub mod webhook;

use serde_json::Value;
use sych_core::scm::{ProviderCapabilities, RepositoryRef, ScmError, ScmErrorKind};

pub use actions::{Artifact, Job, WorkflowRun, host_allowed};
pub use app::{AppKey, InstallationToken, Secret, TokenCache};
pub use client::{Github, Installation};
pub use comments::{Comment, MAX_COMMENTS, own_comment};
pub use reviews::{CheckOutput, CheckRun};

/// GitHub lists at most this many files of a pull request.
pub const MAX_PR_FILES: usize = 3000;

/// Capabilities from the permissions an installation was granted. Every read and write the
/// flow uses is checked; `checks_write` matters only when check runs are enabled.
pub fn capabilities(permissions: &Value) -> ProviderCapabilities {
    let level = |name: &str| permissions[name].as_str().unwrap_or("none");
    let read = |name: &str| matches!(level(name), "read" | "write" | "admin");
    let write = |name: &str| matches!(level(name), "write" | "admin");
    let contents = read("contents");
    let pulls = read("pull_requests");
    let actions = read("actions");
    ProviderCapabilities {
        change_refs: pulls,
        full_diff: pulls && contents,
        blobs: contents,
        ci_runs: actions || read("checks"),
        ci_artifacts: actions,
        own_comments: pulls,
        summary_write: write("pull_requests"),
        inline_write: write("pull_requests"),
        // Collaborator permission is readable with `metadata`, which every App has.
        actor_permissions: read("metadata"),
        reconciliation: pulls,
        checks_write: write("checks"),
        max_diff_files: u32::try_from(MAX_PR_FILES).unwrap_or(u32::MAX),
    }
}

/// The current state of a pull request, read from the API rather than the webhook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullState {
    pub number: u64,
    pub open: bool,
    pub merged: bool,
    pub draft: bool,
    pub head_sha: String,
    /// `owner/name` of the head repository; `None` when the fork was deleted.
    pub head_repo: Option<String>,
    pub base_ref: String,
    pub base_sha: String,
    pub changed_files: u64,
}

fn missing(field: &str) -> ScmError {
    ScmError::new(
        ScmErrorKind::Transient,
        format!("pull request without {field}"),
    )
}

impl PullState {
    pub fn from_json(v: &Value) -> Result<Self, ScmError> {
        let text = |p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            number: v["number"].as_u64().ok_or_else(|| missing("number"))?,
            open: v["state"].as_str() == Some("open"),
            merged: v["merged"].as_bool().unwrap_or(false),
            draft: v["draft"].as_bool().unwrap_or(false),
            head_sha: text("/head/sha").ok_or_else(|| missing("head.sha"))?,
            head_repo: text("/head/repo/full_name"),
            base_ref: text("/base/ref").ok_or_else(|| missing("base.ref"))?,
            base_sha: text("/base/sha").ok_or_else(|| missing("base.sha"))?,
            changed_files: v["changed_files"].as_u64().unwrap_or(0),
        })
    }

    pub fn fork(&self, full_name: &str) -> bool {
        self.head_repo.as_deref() != Some(full_name)
    }
}

impl Github {
    pub async fn pull(
        &self,
        repo: &str,
        number: u64,
        token: &Secret,
    ) -> Result<PullState, ScmError> {
        let v: Value = self
            .get(&format!("/repos/{repo}/pulls/{number}"), token)
            .await?;
        PullState::from_json(&v)
    }

    /// Changed files as the provider lists them. Over [`MAX_PR_FILES`] the list is cut, which
    /// is an `INCOMPLETE_DIFF` for the caller: it falls back to local Git or reports the gap.
    pub async fn pull_files(
        &self,
        repo: &str,
        number: u64,
        token: &Secret,
    ) -> Result<Vec<Value>, ScmError> {
        let (files, complete) = self
            .list(
                &format!("/repos/{repo}/pulls/{number}/files"),
                token,
                MAX_PR_FILES,
            )
            .await?;
        if complete {
            Ok(files)
        } else {
            Err(ScmError::new(
                ScmErrorKind::IncompleteDiff,
                format!("the files API lists at most {MAX_PR_FILES} files"),
            ))
        }
    }

    /// `admin | maintain | write | triage | read | none` of `login` on `repo`.
    pub async fn permission(
        &self,
        repo: &str,
        login: &str,
        token: &Secret,
    ) -> Result<String, ScmError> {
        let v: Value = self
            .get(
                &format!("/repos/{repo}/collaborators/{login}/permission"),
                token,
            )
            .await?;
        Ok(v["permission"].as_str().unwrap_or("none").to_owned())
    }

    /// Open pull requests, for reconciliation after downtime.
    pub async fn open_pulls(&self, repo: &str, token: &Secret) -> Result<Vec<PullState>, ScmError> {
        let (pulls, _) = self
            .list(&format!("/repos/{repo}/pulls?state=open"), token, 1_000)
            .await?;
        pulls.iter().map(PullState::from_json).collect()
    }

    /// The repository's stable id and name.
    pub async fn repository(&self, repo: &str, token: &Secret) -> Result<RepositoryRef, ScmError> {
        let v: Value = self.get(&format!("/repos/{repo}"), token).await?;
        Ok(RepositoryRef {
            external_id: v["id"].as_u64().ok_or_else(|| missing("id"))?.to_string(),
            full_name: v["full_name"]
                .as_str()
                .ok_or_else(|| missing("full_name"))?
                .to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn capabilities_follow_granted_permissions() {
        let full = capabilities(&json!({"contents": "read", "pull_requests": "write",
            "actions": "read", "metadata": "read"}));
        assert!(full.missing(false).is_empty(), "{:?}", full.missing(false));
        assert_eq!(full.missing(true), ["checks_write"]);
        let read_only = capabilities(&json!({"contents": "read", "pull_requests": "read",
            "metadata": "read"}));
        assert_eq!(
            read_only.missing(false),
            ["ci_runs", "ci_artifacts", "summary_write", "inline_write"]
        );
    }
}
