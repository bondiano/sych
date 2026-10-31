//! Review requests from the API and the CLI: the change state is read from
//! the provider, the request becomes an explicit manual generation, and nothing is published
//! unless both the request and the connection allow it.

use sych_core::config::CheckRequest;
use sych_core::schedule::{MAX_RETRIES, RunInputs, Trigger};
use sych_core::scm::{ScmError, ScmErrorKind};
use sych_storage::{RunHandle, StorageError};

use super::engine::{Engine, scm_state};

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Scm(#[from] ScmError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl Engine {
    /// A manual generation of PR `number`: `deep` asks for the deep profile, `publish` for an
    /// advisory publication (only where the connection publishes).
    #[allow(clippy::too_many_arguments)] // one review request
    pub async fn request_review(
        &self,
        connection: &str,
        full_name: &str,
        number: u64,
        actor: &str,
        publish: bool,
        deep: bool,
        checks: Vec<CheckRequest>,
    ) -> Result<RunHandle, RequestError> {
        let conn = self
            .conns
            .get(connection)
            .ok_or_else(|| RequestError::NotFound(format!("no connection `{connection}`")))?;
        if !conn.config.allows(full_name) {
            return Err(RequestError::NotFound(format!(
                "{full_name} is not bound to `{connection}`"
            )));
        }

        let now = (self.now)();
        let installation = conn.gh.installation(full_name, now.as_second()).await?;
        let token = conn
            .gh
            .token(installation.id, full_name, now.as_second())
            .await?;
        let repo = conn.gh.repository(full_name, &token).await?;
        let change = self
            .app
            .storage
            .ensure_change(&conn.binding, &repo, number)
            .await?;
        self.manual(change, actor, publish, deep, checks).await
    }

    /// A manual generation of a known change with the same inputs as now on the provider.
    pub async fn manual(
        &self,
        change_id: i64,
        actor: &str,
        publish: bool,
        deep: bool,
        targeted: Vec<CheckRequest>,
    ) -> Result<RunHandle, RequestError> {
        let storage = &self.app.storage;
        let change = storage.change(change_id).await?;
        let conn = self
            .conns
            .get(&change.connection_id)
            .ok_or_else(|| RequestError::NotFound("connection is not configured".into()))?;
        let token = self.token(conn, &change).await?;
        let pull = conn
            .gh
            .pull(&change.full_name, change.number, &token)
            .await?;
        let state = scm_state(&pull, &change.full_name);
        storage.sync_change(change_id, &state, (self.now)()).await?;
        if !state.open {
            return Err(RequestError::Refused("the change is closed".into()));
        }

        let inputs = RunInputs {
            head_sha: state.head_sha,
            base_sha: state.base_sha,
            base_ref: state.base_ref,
            fork: state.fork,
            trigger: Trigger::Manual {
                actor: actor.into(),
                deep,
            },
            publish: publish && conn.config.publish,
            checks: publish && conn.config.publish && conn.config.checks,
            targeted,
        };
        let deadline = (self.now)().as_second() + self.job_seconds();
        Ok(storage
            .generate(change_id, &inputs, deadline, MAX_RETRIES + 1, (self.now)())
            .await?)
    }
}

impl RequestError {
    /// Whether the provider was only unavailable for now.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Scm(e) if e.kind.retryable() || e.kind == ScmErrorKind::PublicationUnknown)
    }
}
