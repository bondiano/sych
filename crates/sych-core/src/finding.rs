//! Candidates (unverified hypotheses) and findings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AttemptId, CandidateId, CodeRef, EvidenceId, FindingId, ObligationId, Sha256Digest};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateOrigin {
    Model,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationStatus {
    Pending,
    Confirmed,
    Rejected,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Candidate {
    pub id: CandidateId,
    pub origin: CandidateOrigin,
    pub category: FindingCategory,
    /// The proposer's guess; the verifier sets the finding's severity.
    pub proposed_severity: Severity,
    pub claim: String,
    pub causal_chain: Vec<String>,
    pub anchors: Vec<CodeRef>,
    pub consequences: Vec<String>,
    pub conditions: Vec<String>,
    pub refutation: String,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    pub obligation_ids: Vec<ObligationId>,
    pub verification_status: VerificationStatus,
    /// Why the candidate is rejected or unresolved, when it is.
    #[serde(default)]
    pub limitations: Vec<String>,
    /// The verifier's answers about it, one per round, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verdicts: Vec<VerdictRecord>,
}

/// One verifier answer about a candidate and what the engine made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerdictRecord {
    /// 1 for the first call; 2 for the call with the answers to the context asks.
    pub round: u32,
    /// The verdict the verifier gave; `PENDING` when it gave none, or more than one.
    pub proposed: VerificationStatus,
    /// The status the engine accepted: lower than proposed when the basis was not admissible.
    pub status: VerificationStatus,
    pub explanation: String,
    /// Wrappers, guards, recovery paths or intentional changes the verifier looked for.
    #[serde(default)]
    pub alternatives_checked: Vec<String>,
    /// The verdict's anchors that resolved in the shown spans.
    #[serde(default)]
    pub anchors: Vec<CodeRef>,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    /// What the engine did not accept in the verdict.
    #[serde(default)]
    pub engine_problems: Vec<String>,
    /// The accepted model attempt; none when a stored checkpoint answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
}

/// The full new text of exactly the HEAD lines of `code`, which a comment can offer as a
/// one-click change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuggestedChange {
    pub code: CodeRef,
    pub replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FindingCategory {
    Defect,
    RuleViolation,
    Improvement,
    Nit,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// Kind of support; independent of severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Support {
    ExecutedCounterexample,
    StaticTrace,
    SemanticArgument,
    ExplicitRule,
    Preference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lifecycle {
    Open,
    Resolved,
    Dismissed,
    Obsolete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Finding {
    pub id: FindingId,
    pub category: FindingCategory,
    pub severity: Severity,
    pub support: Support,
    pub title: String,
    pub explanation: String,
    pub triggering_conditions: Vec<String>,
    pub consequence: String,
    pub anchors: Vec<CodeRef>,
    pub evidence_ids: Vec<EvidenceId>,
    pub rule_id: Option<String>,
    #[serde(default)]
    pub obligation_ids: Vec<ObligationId>,
    pub suggested_action: String,
    pub validation_expectation: String,
    pub fingerprint: Sha256Digest,
    pub predecessor_finding_id: Option<FindingId>,
    pub lifecycle: Lifecycle,
    pub scope: String,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_change: Option<SuggestedChange>,
    /// Less severe findings on overlapping lines, kept inside this one with their claim,
    /// conditions, consequence and anchors; their rule and evidence ids join this finding's. One
    /// comment per location, no claim lost.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub also: Vec<AlsoFinding>,
}

/// A confirmed finding merged into an overlapping, at least as severe one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AlsoFinding {
    pub category: FindingCategory,
    pub severity: Severity,
    pub title: String,
    pub explanation: String,
    pub triggering_conditions: Vec<String>,
    pub consequence: String,
    pub anchors: Vec<CodeRef>,
}
