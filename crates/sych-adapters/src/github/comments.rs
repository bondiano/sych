//! Issue comments of a pull request: the summary lives in one, found again by its marker and
//! its author. A write whose reply is lost is `PUBLICATION_UNKNOWN`, never "not sent".

use reqwest::Method;
use serde_json::{Value, json};
use sych_core::scm::{ScmCall, ScmError, ScmErrorKind};

use super::{Github, Secret};

/// Comments read at most per change; more is a reconciliation limitation, not "absent".
pub const MAX_COMMENTS: usize = 3_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    pub author: String,
    /// GitHub marks App installations' users as `Bot`; a human copying the marker is not one.
    pub author_is_bot: bool,
    pub body: String,
}

impl Comment {
    pub(super) fn from_json(v: &Value) -> Result<Self, ScmError> {
        Ok(Self {
            id: v["id"]
                .as_u64()
                .ok_or_else(|| ScmError::new(ScmErrorKind::Transient, "comment without id"))?,
            author: v["user"]["login"].as_str().unwrap_or_default().to_owned(),
            author_is_bot: v["user"]["type"].as_str() == Some("Bot"),
            body: v["body"].as_str().unwrap_or_default().to_owned(),
        })
    }

    /// Written by this App's bot user.
    pub fn owned_by(&self, bot_login: &str) -> bool {
        self.author_is_bot && self.author.eq_ignore_ascii_case(bot_login)
    }
}

impl Github {
    /// Every comment of the change; `complete` is false when the list was cut.
    pub async fn comments(
        &self,
        repo: &str,
        number: u64,
        token: &Secret,
    ) -> Result<(Vec<Comment>, bool), ScmError> {
        let (items, complete) = self
            .list(
                &format!("/repos/{repo}/issues/{number}/comments"),
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

    pub async fn comment(&self, repo: &str, id: u64, token: &Secret) -> Result<Comment, ScmError> {
        let v: Value = self
            .get(&format!("/repos/{repo}/issues/comments/{id}"), token)
            .await?;
        Comment::from_json(&v)
    }

    pub async fn create_comment(
        &self,
        repo: &str,
        number: u64,
        body: &str,
        token: &Secret,
    ) -> Result<Comment, ScmError> {
        let path = format!("/repos/{repo}/issues/{number}/comments");
        let v: Value = self
            .write(
                Method::POST,
                &path,
                token,
                &json!({ "body": body }),
                ScmCall::Write,
            )
            .await?;
        Comment::from_json(&v)
    }

    pub async fn update_comment(
        &self,
        repo: &str,
        id: u64,
        body: &str,
        token: &Secret,
    ) -> Result<Comment, ScmError> {
        let path = format!("/repos/{repo}/issues/comments/{id}");
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
}
