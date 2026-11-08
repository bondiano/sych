//! The change's conversation since its last published summary: the doubts that summary left
//! open and the replies the pull request's author wrote after it. Only the author's own
//! issue comments and review-thread replies count; other people's and bots' never do.

use sych_adapters::github::{Comment, Secret};
use sych_core::analysis::prior::{self, PriorDoubts};
use sych_core::scm::ScmError;
use sych_core::{RunId, SchemaVersion, Timestamp};
use sych_storage::ChangeRow;

use super::engine::{Conn, Engine};

/// `(created at, body)` of the comments `author`, a person, wrote after `since`.
fn by_author(comments: Vec<Comment>, author: &str, since: Timestamp) -> Vec<(Timestamp, String)> {
    comments
        .into_iter()
        .filter(|c| !author.is_empty() && !c.author_is_bot)
        .filter(|c| c.author.eq_ignore_ascii_case(author))
        .filter_map(|c| Some((c.created_at.filter(|at| *at > since)?, c.body)))
        .collect()
}

impl Engine {
    /// The doubts of the run whose summary was delivered last, with its author's replies since;
    /// `None` without doubts, or when the conversation cannot be read.
    pub(super) async fn prior_doubts(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        token: &Secret,
    ) -> anyhow::Result<Option<PriorDoubts>> {
        let storage = &self.app.storage;
        let Some((run, at)) = storage.last_summary(change.id).await? else {
            return Ok(None);
        };
        let run = RunId(run.parse()?);
        let Ok(result) = storage.load_result(&change.partition.tenant_id, run).await else {
            return Ok(None);
        };
        let doubts = prior::doubts(&result);
        if doubts.is_empty() {
            return Ok(None);
        }

        let published_at = Timestamp::from_second(at)?;
        let replies = match self.replies(conn, change, token, published_at).await {
            Ok(replies) => replies,
            Err(e) => {
                tracing::warn!(change = change.id, error = %e, "conversation not read; prior doubts left out");
                return Ok(None);
            }
        };
        let (replies, omitted_replies) = prior::bound_replies(replies);
        Ok(Some(PriorDoubts {
            schema_version: SchemaVersion,
            run_id: run,
            published_at,
            doubts,
            replies,
            omitted_replies,
        }))
    }

    // shortcut: a list cut at MAX_COMMENTS loses its newest replies unseen; page from the end
    // if a change ever has that many comments.
    async fn replies(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        token: &Secret,
        since: Timestamp,
    ) -> Result<Vec<(Timestamp, String)>, ScmError> {
        let (repo, number) = (&change.full_name, change.number);
        let author = conn.gh.pull(repo, number, token).await?.author;
        let (issue, _) = conn.gh.comments(repo, number, token).await?;
        let (review, _) = conn.gh.review_comments(repo, number, token).await?;
        let threads = review.into_iter().filter(|c| c.in_reply_to.is_some());
        Ok(by_author(
            issue.into_iter().chain(threads).collect(),
            &author,
            since,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(author: &str, bot: bool, at: i64) -> Comment {
        Comment {
            id: 1,
            author: author.into(),
            author_is_bot: bot,
            body: format!("{author}@{at}"),
            created_at: Timestamp::from_second(at).ok(),
            in_reply_to: None,
        }
    }

    #[test]
    fn only_the_authors_own_later_comments_count() {
        let since = Timestamp::from_second(100).expect("since");
        let comments = vec![
            comment("Dev", false, 101),
            comment("dev", false, 100),
            comment("other", false, 102),
            comment("dev", true, 103),
            comment("sych[bot]", true, 104),
        ];
        let kept: Vec<String> = by_author(comments.clone(), "dev", since)
            .into_iter()
            .map(|(_, b)| b)
            .collect();
        assert_eq!(kept, ["Dev@101"]);
        assert!(by_author(comments, "", since).is_empty());
    }
}
