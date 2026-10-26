//! The normalized SCM contract every provider adapter maps to (spec §19.1, §19.3): errors,
//! capabilities and what a webhook signals. Webhooks are signals; current refs come from the
//! API.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Revision of the HTTP → [`ScmErrorKind`] mapping, kept with every recorded error.
pub const ERROR_MAPPING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScmErrorKind {
    Unauthorized,
    UnsupportedCapability,
    NotFound,
    RateLimited,
    Transient,
    IncompleteDiff,
    InvalidAnchor,
    PublicationUnknown,
}

impl ScmErrorKind {
    /// Only rate limits and transient failures are retried without a change of inputs; an
    /// unknown publication outcome goes to the reconciler, never to a blind retry.
    pub const fn retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Transient)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct ScmError {
    pub kind: ScmErrorKind,
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

impl ScmError {
    pub fn new(kind: ScmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_seconds: None,
        }
    }
}

/// The call an HTTP status came from; 422 means different things per endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmCall {
    Read,
    /// Creating or updating a publication object; the outcome of a lost reply is unknown.
    Write,
    /// An inline review comment; 422 is a position GitHub does not accept.
    InlineComment,
}

/// Maps a provider HTTP status (mapping v1). 403 is not "unsupported": it is a missing
/// permission unless the provider says it is a rate limit; 422 is never retryable.
pub fn map_status(status: u16, rate_limited: bool, call: ScmCall) -> ScmErrorKind {
    match status {
        403 | 429 if rate_limited || status == 429 => ScmErrorKind::RateLimited,
        401 | 403 => ScmErrorKind::Unauthorized,
        404 | 410 => ScmErrorKind::NotFound,
        422 if call == ScmCall::InlineComment => ScmErrorKind::InvalidAnchor,
        422 | 400 | 409 | 415 => ScmErrorKind::UnsupportedCapability,
        _ if call != ScmCall::Read => ScmErrorKind::PublicationUnknown,
        _ => ScmErrorKind::Transient,
    }
}

/// What a connection can do, fixed when it is connected or updated (spec §19.1). A missing
/// capability gives a supported fallback or an explicit limitation, never a guess.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[allow(clippy::struct_excessive_bools)] // independent flags of one provider, not states
pub struct ProviderCapabilities {
    pub change_refs: bool,
    pub full_diff: bool,
    pub blobs: bool,
    pub ci_runs: bool,
    pub ci_artifacts: bool,
    pub own_comments: bool,
    pub summary_write: bool,
    pub inline_write: bool,
    pub actor_permissions: bool,
    pub reconciliation: bool,
    pub checks_write: bool,
    /// Files the provider diff API lists at most; more means the local Git fallback.
    pub max_diff_files: u32,
}

impl ProviderCapabilities {
    /// Names of the capabilities the review flow needs and this connection lacks.
    pub fn missing(&self, checks: bool) -> Vec<&'static str> {
        [
            ("change_refs", self.change_refs),
            ("full_diff", self.full_diff),
            ("blobs", self.blobs),
            ("ci_runs", self.ci_runs),
            ("ci_artifacts", self.ci_artifacts),
            ("own_comments", self.own_comments),
            ("summary_write", self.summary_write),
            ("inline_write", self.inline_write),
            ("actor_permissions", self.actor_permissions),
            ("reconciliation", self.reconciliation),
            ("checks_write", self.checks_write || !checks),
        ]
        .into_iter()
        .filter(|(_, has)| !has)
        .map(|(name, _)| name)
        .collect()
    }
}

/// A command written in a change comment; accepted only from an authorized actor (§22.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScmCommand {
    Rerun,
    Deep,
    Stop,
}

/// The normalized meaning of one webhook event (spec §19.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChangeSignal {
    /// Opened, reopened or marked ready: plan a review of the current state.
    Opened,
    /// New source commits: supersede the active run.
    SourcePush {
        head_sha: String,
    },
    /// The target branch or its base moved: refresh the integration context.
    TargetChanged,
    /// Converted to draft: no expensive analysis by default.
    Draft,
    /// Closed or merged: stop new analysis and unsent publication.
    Closed {
        merged: bool,
    },
    /// A CI run finished for `head_sha`; its binding and significance are checked later.
    CiCompleted {
        run_id: u64,
        head_sha: String,
    },
    Command {
        actor: String,
        command: ScmCommand,
    },
    /// A comment by the bot itself; never schedules work.
    BotComment,
}

impl ChangeSignal {
    /// Seconds equal signals coalesce over (spec §19.3).
    pub const fn debounce_seconds(&self) -> i64 {
        match self {
            Self::CiCompleted { .. } => 10,
            _ => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepositoryRef {
    /// The provider's stable numeric id, as text.
    pub external_id: String,
    pub full_name: String,
}

/// A verified, parsed webhook delivery. `signal` is `None` for events Sych records but does not
/// act on (ping, labels, edits that change nothing it reviews).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebhookEvent {
    pub event: String,
    pub action: Option<String>,
    pub repository: Option<RepositoryRef>,
    pub installation_id: Option<u64>,
    /// Changes the event concerns; a CI run may name several, or none for a fork.
    pub change_numbers: Vec<u64>,
    /// The source branch lives in another repository; it is read, never run (spec §19.6).
    pub fork: bool,
    pub sender: Option<String>,
    pub signal: Option<ChangeSignal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_mapping_v1() {
        use ScmCall::{InlineComment, Read, Write};
        assert_eq!(map_status(403, false, Read), ScmErrorKind::Unauthorized);
        assert_eq!(map_status(403, true, Read), ScmErrorKind::RateLimited);
        assert_eq!(map_status(429, false, Write), ScmErrorKind::RateLimited);
        assert_eq!(
            map_status(422, false, InlineComment),
            ScmErrorKind::InvalidAnchor
        );
        assert_eq!(
            map_status(422, false, Read),
            ScmErrorKind::UnsupportedCapability
        );
        assert!(!map_status(422, false, Write).retryable());
        assert_eq!(map_status(502, false, Read), ScmErrorKind::Transient);
        assert_eq!(
            map_status(502, false, Write),
            ScmErrorKind::PublicationUnknown
        );
        assert!(!ScmErrorKind::PublicationUnknown.retryable());
    }
}
