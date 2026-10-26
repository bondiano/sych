//! Candidates (unverified hypotheses) and findings (spec §5.6).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CandidateId, CodeRef, EvidenceId, FindingId, ObligationId, Sha256Digest};

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
}
