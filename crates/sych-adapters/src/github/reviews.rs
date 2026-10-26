//! Inline review comments by explicit `commit_id`, `path`, `line` and `side` (never the
//! deprecated `position`), and advisory check runs. There is no approve, request-changes or
//! merge call in this adapter.

use reqwest::Method;
use serde_json::{Value, json};
use sych_core::publication::{DiffSide, InlineAnchor};
use sych_core::scm::{ScmCall, ScmError, ScmErrorKind};

use super::{Comment, Github, MAX_COMMENTS, Secret};

const fn side(s: DiffSide) -> &'static str {
    match s {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

/// A check run the App created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub id: u64,
    pub app_id: Option<u64>,
    pub external_id: Option<String>,
}

fn check_run(v: &Value) -> Result<CheckRun, ScmError> {
    Ok(CheckRun {
        id: v["id"]
            .as_u64()
            .ok_or_else(|| ScmError::new(ScmErrorKind::Transient, "check run without id"))?,
        app_id: v["app"]["id"].as_u64(),
        external_id: v["external_id"].as_str().map(str::to_owned),
    })
}

impl Github {
    pub async fn create_review_comment(
        &self,
        repo: &str,
        number: u64,
        anchor: &InlineAnchor,
        body: &str,
        token: &Secret,
    ) -> Result<Comment, ScmError> {
        let mut payload = json!({"body": body, "commit_id": anchor.commit_id, "path": anchor.path,
            "line": anchor.line, "side": side(anchor.side)});
        if let (Some(start), Some(start_side)) = (anchor.start_line, anchor.start_side) {
            payload["start_line"] = json!(start);
            payload["start_side"] = json!(side(start_side));
        }

        let path = format!("/repos/{repo}/pulls/{number}/comments");
        let v: Value = self
            .write(Method::POST, &path, token, &payload, ScmCall::InlineComment)
            .await?;
        Comment::from_json(&v)
    }

    pub async fn update_review_comment(
        &self,
        repo: &str,
        id: u64,
        body: &str,
        token: &Secret,
    ) -> Result<Comment, ScmError> {
        let path = format!("/repos/{repo}/pulls/comments/{id}");
        let v: Value = self
            .write(
                Method::PATCH,
                &path,
                token,
                &json!({ "body": body }),
                ScmCall::Write,
            )
            .await?;
        Comment::from_json(&v)
    }

    pub async fn review_comment(
        &self,
        repo: &str,
        id: u64,
        token: &Secret,
    ) -> Result<Comment, ScmError> {
        let v: Value = self
            .get(&format!("/repos/{repo}/pulls/comments/{id}"), token)
            .await?;
        Comment::from_json(&v)
    }

    /// Every inline comment of the change; `complete` is false when the list was cut.
    pub async fn review_comments(
        &self,
        repo: &str,
        number: u64,
        token: &Secret,
    ) -> Result<(Vec<Comment>, bool), ScmError> {
        let (items, complete) = self
            .list(
                &format!("/repos/{repo}/pulls/{number}/comments"),
                token,
                MAX_COMMENTS,
            )
            .await?;
        let comments = items
            .iter()
            .map(Comment::from_json)
            .collect::<Result<_, _>>()?;
        Ok((comments, complete))
    }

    /// A completed advisory check run pinned to `head_sha`; `external_id` is its marker.
    pub async fn create_check_run(
        &self,
        repo: &str,
        head_sha: &str,
        external_id: &str,
        output: &CheckOutput<'_>,
        token: &Secret,
    ) -> Result<CheckRun, ScmError> {
        let payload = json!({"name": output.name, "head_sha": head_sha, "external_id": external_id,
            "status": "completed", "conclusion": output.conclusion,
            "output": {"title": output.title, "summary": output.summary}});
        let v: Value = self
            .write(
                Method::POST,
                &format!("/repos/{repo}/check-runs"),
                token,
                &payload,
                ScmCall::Write,
            )
            .await?;
        check_run(&v)
    }

    pub async fn update_check_run(
        &self,
        repo: &str,
        id: u64,
        output: &CheckOutput<'_>,
        token: &Secret,
    ) -> Result<CheckRun, ScmError> {
        let payload = json!({"status": "completed", "conclusion": output.conclusion,
            "output": {"title": output.title, "summary": output.summary}});
        let path = format!("/repos/{repo}/check-runs/{id}");
        let v: Value = self
            .write(Method::PATCH, &path, token, &payload, ScmCall::Write)
            .await?;
        check_run(&v)
    }

    /// Check runs named `name` on `head_sha`, for reconciliation by external id and App.
    pub async fn check_runs(
        &self,
        repo: &str,
        head_sha: &str,
        name: &str,
        token: &Secret,
    ) -> Result<Vec<CheckRun>, ScmError> {
        let v: Value = self
            .get(
                &format!(
                    "/repos/{repo}/commits/{head_sha}/check-runs?check_name={name}&per_page=100"
                ),
                token,
            )
            .await?;
        v["check_runs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(check_run)
            .collect()
    }
}

/// What a check run says; `conclusion` is `neutral` for any advisory result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckOutput<'a> {
    pub name: &'a str,
    pub conclusion: &'a str,
    pub title: &'a str,
    pub summary: &'a str,
}
