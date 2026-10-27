//! `ContextBroker` (spec §9): bounded, addressable evidence packets instead of a context string.
//!
//! A packet lists exact `CodeRef`s with why each was selected, what was left out and why, and
//! how complete every query behind it was. Raw source is not stored in the packet; it is read
//! back through the snapshot reader, which checks the original hash.

mod build;
mod limited;
mod neighbours;
mod observations;
mod ops;
mod select;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use build::{BrokerInput, Limits, build_packets};
pub use limited::{context_gaps, context_limited};
pub use observations::{UNRESOLVED_IMPORT, unseen_spans};
pub use ops::{OpResult, OpStatus, Operation, Ops, SymbolRef};

use crate::{
    CodeRef, Completeness, EvidenceId, ObligationId, PolicySnapshotId, QueryStatus, ReviewUnitId,
    RunId, SchemaVersion, SourceSide, SourceSnapshotId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Purpose {
    Discovery,
    Verification,
    TourFollowup,
}

/// What a context request is bound to (spec §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextRequest {
    pub run_id: RunId,
    pub snapshot_id: SourceSnapshotId,
    pub policy_snapshot_id: PolicySnapshotId,
    pub review_unit_ids: Vec<ReviewUnitId>,
    pub obligation_id: Option<ObligationId>,
    pub question: String,
    pub purpose: Purpose,
    pub seed_refs: Vec<CodeRef>,
    pub requested_operations: Vec<String>,
    /// Input tokens the packet may use.
    pub remaining_budget: u32,
    pub access_scope_id: String,
}

/// Required evidence first, then evidence that could refute a hypothesis, then the rest.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    Required,
    Refuting,
    Supporting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Fragment {
    /// Short content-derived id that links refer to.
    pub id: String,
    pub code_ref: CodeRef,
    pub priority: Priority,
    /// Why it was selected; merged fragments keep every reason.
    pub reason_selected: Vec<String>,
    pub evidence_ids: Vec<EvidenceId>,
    pub completeness: Completeness,
    /// Upper bound on input tokens of the span text.
    pub tokens: u32,
    /// Always `none` so far: spans are never summarised. Secret values are redacted when the
    /// prompt is rendered (`Prompt::messages`), which marks the block it changed.
    pub transformation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    CalledBy,
    Calls,
    UsesType,
    ReferencedBy,
    ReadBy,
    WrittenBy,
    ImplementedBy,
    TestedBy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub relation: Relation,
}

/// One semantic query behind the packet and how complete its answer was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QueryRecord {
    pub operation: String,
    pub symbol: String,
    pub side: SourceSide,
    pub status: QueryStatus,
    pub completeness: Completeness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Excluded {
    pub code_ref: CodeRef,
    pub priority: Priority,
    /// `REQUIRED_OVER_BUDGET`, `BUDGET` or `LIMIT_MAX_SOURCE_FILES`.
    pub reason: String,
}

/// Sufficiency of the requested sources, never the truth of an obligation (spec §9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PacketResolution {
    Satisfied,
    Partial,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PacketUsage {
    pub tokens: u32,
    pub files: u32,
    /// Semantic context items selected into the packet; LSP requests happen earlier.
    pub semantic_items: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidencePacket {
    pub schema_version: SchemaVersion,
    pub request: ContextRequest,
    pub fragments: Vec<Fragment>,
    pub links: Vec<Link>,
    pub queries: Vec<QueryRecord>,
    pub excluded: Vec<Excluded>,
    pub gaps: Vec<String>,
    pub usage: PacketUsage,
    pub resolution: PacketResolution,
}

/// Every packet of a run, as one versioned stored document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPackets {
    pub schema_version: SchemaVersion,
    pub packets: Vec<EvidencePacket>,
}

/// The reason code that forces an obligation to stay `UNKNOWN` (spec §9.3).
pub const REQUIRED_OVER_BUDGET: &str = "REQUIRED_OVER_BUDGET";

/// Gaps of changed code whose behaviour the snapshot does not show: code under a cfg the
/// analysed build skips, a macro that is not expanded, a `Debug` impl defined elsewhere,
/// reflection, `getattr`, monkey patching or import-time registration, and a Python import
/// neither the snapshot nor the standard library has. None of them lets an obligation be
/// `SUPPORTED`; nor do the ledger's `LEDGER_GAPS`.
pub const UNSEEN_GAPS: [&str; 5] = [
    "CFG_INACTIVE",
    "MACRO_UNEXPANDED",
    "DEBUG_UNSEEN_TYPE",
    "DYNAMIC_UNRESOLVED",
    "IMPORT_UNRESOLVED",
];
