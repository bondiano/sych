//! Independent status dimensions (INV-02): execution, coverage, recommendation,
//! publication and relevance never share one field.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Run execution state (spec §8.1). Only `Completed` runs have a saved result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionState {
    Queued,
    Running,
    WaitingEvidence,
    RetryWait,
    Finalizing,
    Completed,
    Cancelled,
    Superseded,
    Failed,
}

impl ExecutionState {
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Superseded | Self::Failed
        )
    }
}

/// How the run reached FINALIZING; recorded inside the immutable result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionOutcome {
    /// Every planned task ran.
    PlanExecuted,
    /// A budget or size limit stopped the plan; skipped work is listed as gaps.
    LimitReached,
    /// Retryable failures exhausted their retries; an honest partial result is kept.
    RetriesExhausted,
    /// Substantive analysis was impossible, for example the diff could not be read.
    InputUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Coverage {
    CompleteForScope,
    Partial,
    Unavailable,
}

/// Computed by the reducer; never a model verdict or SCM approval (spec §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Recommendation {
    AttentionRequired,
    NeedsEvidence,
    NoBlockingFindingsInScope,
}

/// Outbox delivery state of one publication operation (spec §18.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PublicationState {
    Pending,
    Publishing,
    Confirmed,
    Uncertain,
    Failed,
    Superseded,
}

/// Mutable relevance of an immutable result, kept outside the result (spec §6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Relevance {
    Current,
    Stale,
}
