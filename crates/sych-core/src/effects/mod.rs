//! Effect ledger (spec §12, roadmap M3.3): a bounded base/head table of observable effects
//! of the changed code, built from the syntax facts of every language and the accepted
//! effect registry. It is not full-program dataflow: it records what the facts show, how it
//! matched across sides, and how far that match is resolved.

mod build;
mod scope;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use build::{bindings, ledger};

use crate::config::EffectKind;
use crate::semantic::facts_evidence_id;
use crate::{
    BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    GitObjectId, Language, Producer, RepoPath, SchemaVersion, SourceSide, SourceSnapshot,
    SourceSnapshotId, SyntaxKind, Timestamp, TrustClass, Validity, canonical_sha256,
};

/// How far an entry's match is backed by resolved receivers and links.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Resolved,
    /// Matched by name or position, or a move without a proven call edge.
    Partial,
    /// A receiver, callee or registration the snapshot does not show.
    DynamicUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectMatch {
    Preserved,
    /// Gone from its construct on head and found in another one.
    Moved,
    /// Kept, but its scope, guard, multiplicity or order differs.
    Changed,
    Removed,
    Added,
    Unresolved,
}

/// One place an effect happens on one side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Occurrence {
    pub code_ref: CodeRef,
    /// Enclosing function, method or component.
    pub symbol: Option<String>,
    /// The call or shape as the facts show it: receiver, arguments or hook; repository data.
    pub payload: String,
    pub guard: Option<String>,
    /// `transaction (*sql.Tx)`, `none: *sql.DB outside any transaction`, `unknown (..)` or
    /// `n/a`.
    pub transaction_scope: String,
    /// `caller`, `detached: go func..`, `with open(..)`, `effect cleanup`, ...
    pub lifetime_scope: String,
    pub failure_behavior: String,
    /// Whether the span intersects a changed line.
    pub in_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LedgerEntry {
    /// Stable within a snapshot: construct, kind and target.
    pub effect_id: String,
    pub kind: EffectKind,
    /// What is affected: the operation, wrapper symbol or registry symbol.
    pub target: String,
    /// The syntax shape the effect was read from.
    pub shape: SyntaxKind,
    pub language: Language,
    /// File and construct the entry belongs to, as the head names them when it can.
    pub path: RepoPath,
    pub symbol: Option<String>,
    /// Registry entry this call is bound to.
    pub registry_id: Option<String>,
    /// `durable_with_transaction` of the registry, only while its implementation is current.
    pub durable_with_transaction: Option<bool>,
    pub contract_refs: Vec<RepoPath>,
    /// Cardinality is the length of each side.
    pub base: Vec<Occurrence>,
    pub head: Vec<Occurrence>,
    #[serde(rename = "match")]
    pub matched: EffectMatch,
    pub resolution: Resolution,
    /// Why it matched so: scope, guard, multiplicity, order, move or what was not ruled out.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BindingStatus {
    /// No implementation ref changed: the accepted guarantee still describes the code.
    Current,
    /// An implementation ref changed in this diff: the guarantee is checked again, not carried.
    ImplementationChanged,
    /// The registry names no implementation; its guarantee is a bare claim.
    Unbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImplementationRef {
    pub path: RepoPath,
    pub base_oid: Option<GitObjectId>,
    pub head_oid: Option<GitObjectId>,
    pub changed: bool,
}

/// A registry entry bound to the implementation it describes in this snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegistryBinding {
    pub registry_id: String,
    pub symbol: String,
    pub kind: EffectKind,
    pub durable_with_transaction: Option<bool>,
    pub implementation: Vec<ImplementationRef>,
    pub contract_refs: Vec<RepoPath>,
    pub status: BindingStatus,
}

impl RegistryBinding {
    pub fn carries_guarantees(&self) -> bool {
        self.status == BindingStatus::Current
    }
}

/// A blob the ledger was read from: a later run reuses the ledger only while all match.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct LedgerDependency {
    pub side: SourceSide,
    pub path: RepoPath,
    pub blob_oid: GitObjectId,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectLedger {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: Option<SourceSnapshotId>,
    pub entries: Vec<LedgerEntry>,
    pub registry: Vec<RegistryBinding>,
    pub dependencies: Vec<LedgerDependency>,
    pub limitations: Vec<String>,
}

/// Packet gaps of the ledger that keep an obligation from `SUPPORTED`: an effect the snapshot
/// cannot resolve, and a registry guarantee whose implementation changed.
pub const LEDGER_GAPS: [&str; 2] = ["EFFECT_UNRESOLVED", "REGISTRY_RECHECK"];

impl EffectLedger {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.registry.is_empty()
    }

    pub fn evidence_id(&self) -> Option<EvidenceId> {
        let snapshot = self.source_snapshot_id.as_ref()?;
        // The ledger spans both sides and every language; the head/Go slot only seeds the id.
        Some(facts_evidence_id(
            "effect-ledger",
            snapshot,
            SourceSide::Head,
            Language::Go,
        ))
    }

    /// Derived static evidence: partial, since effects are what the facts show.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Option<Evidence>, serde_json::Error> {
        let Some(id) = self.evidence_id().filter(|_| !self.is_empty()) else {
            return Ok(None);
        };
        Ok(Some(Evidence {
            schema_version: SchemaVersion,
            id,
            kind: EvidenceKind::Static,
            source_snapshot_id: snapshot.id.clone(),
            producer: Producer {
                name: "sych-effect-ledger".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                config_hash: None,
            },
            binding: EvidenceBinding {
                kind: BindingKind::Source,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: None,
            },
            validity: Validity::Valid,
            completeness: Completeness::Partial,
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self
                .entries
                .iter()
                .flat_map(|e| e.base.iter().chain(&e.head))
                .map(|o| o.code_ref.clone())
                .collect(),
            limitations: self.limitations.clone(),
            observed_at,
        }))
    }
}
