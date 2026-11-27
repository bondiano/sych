//! Source snapshot and code coordinates.

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

/// How `diff_base` was chosen; never an arbitrary commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComparisonMode {
    /// `diff_base = merge-base(target_tip, head)`.
    MergeBase,
    /// `diff_base` was given explicitly for a two-dot comparison.
    ExplicitDiffBase,
}

/// Where the reviewed head's content came from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SnapshotHeadKind {
    #[default]
    Committed,
    Staged,
    Worktree,
}

impl SnapshotHeadKind {
    pub fn is_committed(&self) -> bool {
        *self == Self::Committed
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "SnapshotHeadKind::is_committed")]
    pub head_kind: SnapshotHeadKind,
    pub comparison: ComparisonMode,
    pub source_manifest_hash: Sha256Digest,
    pub diff_manifest_hash: Sha256Digest,
    pub created_at: Timestamp,
}

/// Exact span inside a pinned blob; resolved by the harness, never by a model.
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
    #[serde(skip_serializing_if = "SnapshotHeadKind::is_committed")]
    head_kind: SnapshotHeadKind,
    comparison: ComparisonMode,
    source_manifest_hash: &'a Sha256Digest,
    diff_manifest_hash: &'a Sha256Digest,
}

impl SourceSnapshot {
    /// Content identity: repository, three positions, uncommitted head kind, comparison mode
    /// and both manifests. Repeating the same inputs yields the same snapshot ID regardless of
    /// `target_ref`, `head_ref` or `created_at`.
    pub fn identity(&self) -> Result<SourceSnapshotId, serde_json::Error> {
        canonical_sha256(&SnapshotIdentity {
            repository: &self.repository,
            target_tip: &self.target_tip,
            diff_base: &self.diff_base,
            head: &self.head,
            head_kind: self.head_kind,
            comparison: self.comparison,
            source_manifest_hash: &self.source_manifest_hash,
            diff_manifest_hash: &self.diff_manifest_hash,
        })
        .map(SourceSnapshotId)
    }
}
