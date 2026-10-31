//! Scope and the immutable review result.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{BillingKind, Enforcement};
use crate::{
    Candidate, Coverage, EvidenceManifestId, ExecutionOutcome, Finding, MicroUsd, Obligation,
    ObligationId, PolicySnapshotId, Recommendation, RepoPath, ResultId, ReviewUnitId, RiskLevel,
    RunId, SchemaVersion, SourceSnapshotId, Timestamp,
};

/// Related changes reviewed together around one contract or behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewUnit {
    pub id: ReviewUnitId,
    pub title: String,
    /// Where each file is reported: its new path, or old path when deleted. A renamed file's
    /// old path is still part of its identity in the diff manifest.
    pub paths: Vec<RepoPath>,
    /// Routing hints from the planner; not evidence of a defect.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_hints: Vec<RiskHint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RiskHint {
    pub level: RiskLevel,
    /// Stable code such as `MIGRATION` or `CI_CONFIG`.
    pub code: String,
    pub path: RepoPath,
}

/// A path left out of reading or review, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PathGap {
    pub path: RepoPath,
    pub reason: String,
}

/// Fixed before discovery; may only grow through a new revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScopeManifest {
    pub revision: u32,
    pub units: Vec<ReviewUnit>,
    pub required_obligation_ids: Vec<ObligationId>,
    pub requested_profiles: Vec<String>,
    pub excluded: Vec<PathGap>,
}

/// Cost with explicit unknown part; an unknown charge is never recorded as zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CostSummary {
    /// Money the providers reported, or the price card computed, for settled API attempts.
    pub known: MicroUsd,
    /// List-price estimate of the attempts with complete usage; not a charge.
    pub reference_estimate: MicroUsd,
    pub unknown_cost_attempts: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSummary {
    pub model_attempts: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Already part of `output_tokens`; never added to it.
    pub reasoning_tokens: u64,
    /// Dispatched attempts whose tokens were not reported; the token sums leave them out.
    pub unknown_usage_attempts: u32,
    pub cost: CostSummary,
    pub routes: Vec<RouteUsage>,
}

/// One model route of the run: how it bills and which caps are hard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RouteUsage {
    pub profile: String,
    pub model: String,
    pub billing: BillingKind,
    pub enforcement: Enforcement,
    pub attempts: u32,
    /// Subscription attempts settled with no reported money: they spent included quota, which
    /// is not free. Remaining quota and its reset are not reported by any connected transport.
    pub quota_attempts: u32,
    pub fixed_fee: Option<FeeShare>,
}

/// A subscription fee for one calendar month (UTC); charged to the monthly pool once, not per
/// request or run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FeeShare {
    /// `YYYY-MM`.
    pub period: String,
    pub amount: MicroUsd,
    pub allocated: MicroUsd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    pub engine_version: String,
    /// Model adapters used, e.g. `fake`; a fake adapter proves the harness, not model quality.
    pub model_adapters: Vec<String>,
}

/// The language of every human-readable text a review writes (`review.language`): model
/// prose and the static text of published comments. Code, paths, ids and enum values stay as
/// they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReviewLanguage {
    #[default]
    En,
    Ru,
}

impl ReviewLanguage {
    /// The instruction every model prompt carries.
    pub const fn instruction(self) -> &'static str {
        match self {
            Self::En => {
                "Write every human-readable field (titles, claims, explanations, questions, \
                 rationales, limitations and suggested actions) in English. Keep code, \
                 identifiers, file paths, ids and enum values exactly as they are."
            }
            Self::Ru => {
                "Write every human-readable field (titles, claims, explanations, questions, \
                 rationales, limitations and suggested actions) in Russian. Keep code, \
                 identifiers, file paths, ids and enum values exactly as they are, untranslated."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReviewResult {
    pub schema_version: SchemaVersion,
    pub result_id: ResultId,
    pub run_id: RunId,
    pub review_revision: u64,
    pub parent_result_id: Option<ResultId>,
    pub source_snapshot_id: SourceSnapshotId,
    pub policy_snapshot_id: PolicySnapshotId,
    pub evidence_manifest_id: EvidenceManifestId,
    pub scope_manifest: ScopeManifest,
    pub execution_outcome: ExecutionOutcome,
    pub coverage: Coverage,
    pub recommendation: Recommendation,
    pub findings: Vec<Finding>,
    pub obligations: Vec<Obligation>,
    pub unresolved_questions: Vec<String>,
    pub excluded_or_unreviewed: Vec<PathGap>,
    /// Internal by default.
    #[serde(default)]
    pub rejected_candidates: Vec<Candidate>,
    pub usage_summary: UsageSummary,
    pub created_at: Timestamp,
    pub provenance: Provenance,
    /// The language of the result's prose and of its publication.
    #[serde(default)]
    pub language: ReviewLanguage,
}
