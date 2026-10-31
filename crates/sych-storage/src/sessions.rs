//! Browser sessions: a token's SHA-256 with whom it signs in and until when. Starting and
//! ending one is audited; an expired one is never returned and retention deletes it.

use serde_json::json;
use sqlx::Row;
use sych_core::api::Grant;

use crate::audit::Audit;
use crate::{Storage, StorageError};

/// A person (`USER`) with their roles at sign-in, or an API client (`CLIENT`) by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub tenant_id: String,
    pub actor: String,
    pub kind: String,
    pub grants: Vec<Grant>,
    pub expires_at: i64,
}

impl Storage {
    pub async fn start_session(
        &self,
        token_sha256: &str,
        s: &Session,
        at: i64,
    ) -> Result<(), StorageError> {
        let mut tx = self.write().await?;
        sqlx::query(
            "INSERT INTO sessions (token_sha256, tenant_id, actor, kind, access, created_at,
                 expires_at) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(token_sha256)
        .bind(&s.tenant_id)
        .bind(&s.actor)
        .bind(&s.kind)
        .bind(serde_json::to_string(&s.grants)?)
        .bind(at)
        .bind(s.expires_at)
        .execute(&mut *tx)
        .await?;
        Audit {
            tenant_id: &s.tenant_id,
            actor: &s.actor,
            action: "SESSION_STARTED",
            target: "session".into(),
            repository_id: None,
            metadata: json!({"kind": s.kind, "repositories": s.grants.len()}),
            at,
        }
        .record(&mut tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The session of a token, unless it expired by `now`.
    pub async fn session(
        &self,
        token_sha256: &str,
        now: i64,
    ) -> Result<Option<Session>, StorageError> {
        let row = sqlx::query(
            "SELECT tenant_id, actor, kind, access, expires_at FROM sessions
             WHERE token_sha256 = $1 AND expires_at > $2",
        )
        .bind(token_sha256)
        .bind(now)
        .fetch_optional(self.pool())
        .await?;
        row.map(|r| {
            Ok(Session {
                tenant_id: r.get(0),
                actor: r.get(1),
                kind: r.get(2),
                grants: serde_json::from_str(&r.get::<String, _>(3))?,
                expires_at: r.get(4),
            })
        })
        .transpose()
    }

    /// Ends a session; `false` when there was none.
    pub async fn end_session(&self, token_sha256: &str, at: i64) -> Result<bool, StorageError> {
        let mut tx = self.write().await?;
        let ended = sqlx::query(
            "DELETE FROM sessions WHERE token_sha256 = $1 RETURNING tenant_id, actor, kind",
        )
        .bind(token_sha256)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(r) = &ended {
            let (tenant, actor, kind): (String, String, String) = (r.get(0), r.get(1), r.get(2));
            Audit {
                tenant_id: &tenant,
                actor: &actor,
                action: "SESSION_ENDED",
                target: "session".into(),
                repository_id: None,
                metadata: json!({"kind": kind}),
                at,
            }
            .record(&mut tx)
            .await?;
        }

        tx.commit().await?;
        Ok(ended.is_some())
    }
}
