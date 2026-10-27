//! Obligations: concrete questions a review must answer (spec §5.5).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{CodeRef, EvidenceId, EvidenceKind, ObligationId, ReviewUnitId, Timestamp};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Three-valued rule applicability (spec §11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Applicability {
    Applies,
    DoesNotApply,
    Unknown,
}

/// Final assessment. Each variant carries the fields the spec requires for it;
/// pending work is an execution state, not one of these answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObligationStatus {
    Supported,
    Violated,
    Unknown {
        reason_code: String,
        next_evidence_needed: String,
    },
    NotApplicable {
        /// Checkable basis of non-applicability.
        basis: String,
    },
}

/// How a migration obligation is judged (spec §13.4): the plan alone, or an observed
/// transition of a real environment, which needs evidence of that environment's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvaluationMode {
    PlanReview,
    ObservedTransition,
}

/// One concrete question with its rule, scope, acceptance criterion and assessment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Obligation {
    pub id: ObligationId,
    pub rule_id: String,
    pub rule_version: u32,
    pub question: String,
    pub required: bool,
    pub risk_level: RiskLevel,
    pub review_unit_ids: Vec<ReviewUnitId>,
    pub code_refs: Vec<CodeRef>,
    pub applicability: Applicability,
    pub acceptance_criterion: String,
    pub acceptable_evidence_kinds: Vec<EvidenceKind>,
    #[serde(flatten)]
    pub status: ObligationStatus,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    #[serde(default)]
    pub limitations: Vec<String>,
    /// Scoped exceptions in force; they lower the risk decision and never rewrite `status`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waivers: Vec<Waiver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_mode: Option<EvaluationMode>,
}

/// A rule exception in force for one obligation (spec §11.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Waiver {
    pub paths: Vec<String>,
    pub reason: String,
    pub owner: String,
    pub author: String,
    pub expires_at: Option<Timestamp>,
    /// Completion condition when there is no fixed expiry.
    pub until: Option<String>,
}
