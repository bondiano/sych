//! Evidence records.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    CodeRef, EvidenceId, GitObjectId, SchemaVersion, Sha256Digest, SourceSnapshotId, Timestamp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Source,
    Semantic,
    Static,
    Test,
    Coverage,
    Contract,
    Infrastructure,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Validity {
    Valid,
    Stale,
    Unavailable,
    Malformed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Completeness {
    CompleteForQuery,
    Partial,
    Unknown,
}

/// Provenance class. `AuthenticatedCi` proves origin, not oracle independence from the author.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrustClass {
    TrustedPolicy,
    SourceObservation,
    DerivedLocal,
    AuthenticatedCi,
    AuthorClaim,
    HumanAttestation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Producer {
    pub name: String,
    pub version: String,
    pub config_hash: Option<Sha256Digest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    Source,
    Ci,
    Environment,
}

/// What the evidence was observed against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceBinding {
    pub kind: BindingKind,
    pub head_oid: Option<GitObjectId>,
    pub target_tip_oid: Option<GitObjectId>,
    pub tested_oid: Option<GitObjectId>,
    pub environment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Evidence {
    pub schema_version: SchemaVersion,
    pub id: EvidenceId,
    pub kind: EvidenceKind,
    pub source_snapshot_id: SourceSnapshotId,
    pub producer: Producer,
    pub binding: EvidenceBinding,
    pub validity: Validity,
    pub completeness: Completeness,
    pub trust_class: TrustClass,
    pub artifact_sha256: Sha256Digest,
    #[serde(default)]
    pub code_refs: Vec<CodeRef>,
    #[serde(default)]
    pub limitations: Vec<String>,
    pub observed_at: Timestamp,
}
