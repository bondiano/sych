//! The publisher's reconciler: an unknown outcome is reconciled by the stored remote id or the
//! marker of an object the App itself wrote; CREATE again only once a complete read after a
//! grace shows the object absent. Delivered summaries are verified against a late older write.

use sych_adapters::github::{Comment, Secret, own_comment};
use sych_core::scm::ScmError;
use sych_core::sha256;
use sych_storage::{ChangeRow, Uncertain};

use super::engine::{Conn, Engine};
use super::publisher::{CHECK_NAME, UNCERTAIN_WINDOW, Written, bad};

/// Pause between reconciliation passes of one operation.
const RECONCILE_EVERY: i64 = 30;
/// How long a write answered 5xx may stay invisible before a complete empty read proves it absent.
const CREATE_GRACE: i64 = 300;

impl Engine {
    /// One pass over due `UNCERTAIN` operations.
    pub async fn reconcile_publications_once(&self) -> anyhow::Result<()> {
        let storage = &self.app.storage;
        let now = (self.now)();
        let later = now.as_second() + RECONCILE_EVERY;
        for u in storage.uncertain_due(now).await? {
            let change = storage.change(u.change_id).await?;
            let Some(conn) = self.conns.get(&change.connection_id) else {
                continue;
            };
            let Ok(token) = self.token(conn, &change).await else {
                storage.uncertain_unresolved(&u, later, now).await?;
                continue;
            };
            match self.find_own(conn, &change, &u, &token).await {
                Ok(Some(found)) => {
                    let differs = sha256(found.body.as_bytes()) != sha256(u.body.as_bytes());
                    if u.kind == "SUMMARY" && differs {
                        // Ours, with another revision: restore the desired body by its known id.
                        let id = found.id.parse()?;
                        conn.gh
                            .update_comment(&change.full_name, id, &u.body, &token)
                            .await?;
                    }

                    storage.uncertain_found(&u, &found.id, now).await?;
                }
                // Every page read and nothing of ours: after the grace a late-applied write
                // would be visible, so the absence is known and CREATE is safe.
                Ok(None)
                    if u.kind != "CHECK"
                        && u.remote_id.is_none()
                        && now.as_second()
                            >= u.reconcile_deadline - UNCERTAIN_WINDOW + CREATE_GRACE
                        && storage
                            .recreate_absent(u.op_id, &change.partition.tenant_id, now)
                            .await? =>
                {
                    tracing::warn!(op = u.op_id, kind = %u.kind, "absent after a complete read: CREATE again");
                }
                // A partial read, a check run (one page, no completeness) or the attempt bound
                // never allows a new CREATE.
                Ok(None) | Err(_) => storage.uncertain_unresolved(&u, later, now).await?,
            }
        }
        Ok(())
    }

    /// The App's own object of the operation: by stored id, else by marker over every page.
    async fn find_own(
        &self,
        conn: &Conn,
        change: &ChangeRow,
        u: &Uncertain,
        token: &Secret,
    ) -> Result<Option<Written>, ScmError> {
        let (gh, repo) = (&conn.gh, change.full_name.as_str());
        let bot = conn.config.bot_login.as_str();
        let found = |c: Comment| Written {
            id: c.id.to_string(),
            body: c.body,
        };
        if u.kind == "CHECK" {
            let head = change.head_sha.as_deref().unwrap_or_default();
            let runs = gh.check_runs(repo, head, CHECK_NAME, token).await?;
            let own = runs.into_iter().find(|r| {
                r.app_id == Some(conn.config.app_id) && r.external_id.as_deref() == Some(&u.marker)
            });
            return Ok(own.map(|r| Written {
                id: r.id.to_string(),
                body: u.body.clone(),
            }));
        }

        let inline = u.kind == "INLINE";
        if let Some(id) = &u.remote_id {
            let id = id.parse().map_err(|_| bad("bad id"))?;
            let comment = if inline {
                gh.review_comment(repo, id, token).await?
            } else {
                gh.comment(repo, id, token).await?
            };
            return Ok(comment.owned_by(bot).then(|| found(comment)));
        }

        let (comments, complete) = if inline {
            gh.review_comments(repo, change.number, token).await?
        } else {
            gh.comments(repo, change.number, token).await?
        };
        Ok(own_comment(comments, complete, bot, &u.marker)?.map(found))
    }

    /// Restores delivered summaries whose remote body was changed by a late older write.
    pub async fn verify_published_once(&self) -> anyhow::Result<u32> {
        let storage = &self.app.storage;
        let mut repaired = 0;
        for (_, change_id, remote_id, body) in storage.delivered_objects().await? {
            let change = storage.change(change_id).await?;
            let Some(conn) = self.conns.get(&change.connection_id) else {
                continue;
            };
            let Ok(token) = self.token(conn, &change).await else {
                continue;
            };
            // A row that cannot name its comment is skipped, so it never blocks the rest.
            let Ok(id) = remote_id.parse() else {
                tracing::warn!(change_id, %remote_id, "delivered summary with an unreadable remote id");
                continue;
            };
            let Ok(comment) = conn.gh.comment(&change.full_name, id, &token).await else {
                continue;
            };
            if comment.owned_by(&conn.config.bot_login) && comment.body != body {
                conn.gh
                    .update_comment(&change.full_name, comment.id, &body, &token)
                    .await?;
                storage
                    .log_event(
                        change_id,
                        None,
                        "PUBLICATION_REPAIRED",
                        &remote_id,
                        (self.now)(),
                    )
                    .await?;
                repaired += 1;
            }
        }
        Ok(repaired)
    }
}
