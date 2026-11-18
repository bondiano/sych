//! New inline comments of one revision as one `COMMENT` review, or each alone when GitHub
//! refuses the review for one position.

use sych_adapters::github::Secret;
use sych_core::publication::InlineAnchor;
use sych_core::scm::{ScmError, ScmErrorKind};
use sych_storage::{ChangeRow, PublishClaim};

use super::engine::{Conn, Engine};
use super::publisher::Written;

impl Engine {
    /// New inline comments of one revision as one review, so they read as one in the timeline.
    /// GitHub refuses the whole review for one invalid position, so then each is sent alone and
    /// only that one moves to the summary.
    pub(super) async fn publish_review(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        claims: Vec<PublishClaim>,
        token: &Secret,
    ) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let mut ready = Vec::with_capacity(claims.len());
        for c in claims {
            let Ok(anchor) =
                serde_json::from_str::<InlineAnchor>(c.anchor.as_deref().unwrap_or(""))
            else {
                let reason = "inline object without an anchor";
                storage
                    .publication_end(c.op_id, "FAILED", reason, now)
                    .await?;
                continue;
            };
            let body = self.render(&c, &change.full_name, true).await?;
            if storage.begin_publishing(&c, &body, now).await?.is_some() {
                ready.push((c, anchor, body));
            }
        }

        if let [(c, _, body)] = ready.as_slice() {
            let reply = self.send(conn, change, c, body, token).await;
            return self.settle(conn, change, c, reply).await;
        }

        if ready.is_empty() {
            return Ok(());
        }

        let comments: Vec<_> = ready.iter().map(|(_, a, b)| (a, b.as_str())).collect();
        match conn
            .gh
            .create_review(&change.full_name, change.number, &comments, token)
            .await
        {
            Ok(created) => {
                for (c, _, body) in &ready {
                    let marker = format!("<!-- {} -->", c.marker);
                    let reply = created
                        .iter()
                        .find(|x| x.body.contains(&marker))
                        .map(|x| Written {
                            id: x.id.to_string(),
                            body: body.clone(),
                        })
                        .ok_or_else(|| {
                            ScmError::new(
                                ScmErrorKind::PublicationUnknown,
                                "the review does not list this comment",
                            )
                        });
                    self.settle(conn, change, c, reply).await?;
                }
            }
            Err(e) if e.kind == ScmErrorKind::InvalidAnchor => {
                tracing::warn!(error = %e, "review refused; inline comments sent one by one");
                for (c, _, body) in &ready {
                    let reply = self.send(conn, change, c, body, token).await;
                    self.settle(conn, change, c, reply).await?;
                }
            }
            Err(e) => {
                for (c, ..) in &ready {
                    self.settle(conn, change, c, Err(e.clone())).await?;
                }
            }
        }
        Ok(())
    }
}
