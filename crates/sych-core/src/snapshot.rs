//! Source snapshot and code coordinates (spec §5.3, §6.1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    GitObjectId, RepoPath, SchemaVersion, Sha256Digest, SourceSnapshotId, Timestamp,
    canonical_sha256,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceSide {
    DiffBase,
    Head,
    TargetTip,
}

/// How `diff_base` was chosen; never an arbitrary commit (spec §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComparisonMode {
    /// `diff_base = merge-base(target_tip, head)`.
    MergeBase,
    /// `diff_base` was given explicitly for a two-dot comparison.
    ExplicitDiffBase,
}

/// Immutable source positions of one review. `target_tip` and `diff_base` are separate fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceSnapshot {
    pub schema_version: SchemaVersion,
    pub id: SourceSnapshotId,
    /// Opaque repository identity, not a host path.
    pub repository: String,
    pub target_ref: Option<String>,
    pub target_tip: GitObjectId,
    pub diff_base: GitObjectId,
    pub head: GitObjectId,
    pub comparison: ComparisonMode,
    pub source_manifest_hash: Sha256Digest,
    pub diff_manifest_hash: Sha256Digest,
    pub created_at: Timestamp,
}

/// Exact span inside a pinned blob; resolved by the harness, never by a model (INV-06).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CodeRef {
    pub snapshot_id: SourceSnapshotId,
    pub side: SourceSide,
    pub path: RepoPath,
    pub blob_oid: GitObjectId,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive.
    pub end_line: u32,
    pub content_sha256: Sha256Digest,
}

#[derive(Serialize)]
struct SnapshotIdentity<'a> {
    repository: &'a str,
    target_tip: &'a GitObjectId,
    diff_base: &'a GitObjectId,
    head: &'a GitObjectId,
    comparison: ComparisonMode,
    source_manifest_hash: &'a Sha256Digest,
    diff_manifest_hash: &'a Sha256Digest,
}

impl SourceSnapshot {
    /// Content identity (spec §6.1): repository, the three positions, comparison mode and both
    /// manifests (the diff manifest records the diff method). `target_ref`, `id` and
    /// `created_at` are excluded, so repeating the same inputs yields the same snapshot.
    pub fn identity(&self) -> Result<SourceSnapshotId, serde_json::Error> {
        canonical_sha256(&SnapshotIdentity {
            repository: &self.repository,
            target_tip: &self.target_tip,
            diff_base: &self.diff_base,
            head: &self.head,
            comparison: self.comparison,
            source_manifest_hash: &self.source_manifest_hash,
            diff_manifest_hash: &self.diff_manifest_hash,
        })
        .map(SourceSnapshotId)
    }
}
