//! Diagnostics of pinned linters Sych runs itself on the head's changed files (ruff, oxlint,
//! actionlint, zizmor). Unlike imported CI reports they are derived from the pinned snapshot,
//! but they are still observations: the linter's rule set, and the project configuration it
//! read, decide what is reported.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    BackendStatus, BindingKind, CiDiagnostic, Completeness, Evidence, EvidenceBinding, EvidenceId,
    EvidenceKind, Producer, SchemaVersion, SourceSide, SourceSnapshot, SourceSnapshotId, Timestamp,
    TrustClass, Validity, canonical_sha256, derived_uuid,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LintFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    pub side: SourceSide,
    /// Pinned linter name and version; `config_hash` covers the arguments Sych passed.
    pub producer: Producer,
    pub status: BackendStatus,
    /// Linted files of this side.
    pub files: Vec<String>,
    /// Project configuration files the linter may read; empty means its defaults.
    #[serde(default)]
    pub config: Vec<String>,
    /// Findings with a `CodeRef` when their line resolves on this side.
    pub diagnostics: Vec<CiDiagnostic>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

impl LintFacts {
    pub fn evidence_id(&self) -> EvidenceId {
        let side = serde_json::to_value(self.side).unwrap_or_default();
        EvidenceId(derived_uuid(&[
            "lint",
            &self.producer.name,
            &self.source_snapshot_id.0.to_string(),
            side.as_str().unwrap_or_default(),
        ]))
    }

    /// Derived static evidence, partial: rules and configuration bound what it shows.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        let validity = match self.status {
            BackendStatus::Ready => Validity::Valid,
            BackendStatus::Unsupported => Validity::Unavailable,
            BackendStatus::Failed => Validity::Failed,
        };
        Ok(Evidence {
            schema_version: SchemaVersion,
            id: self.evidence_id(),
            kind: EvidenceKind::Static,
            source_snapshot_id: self.source_snapshot_id.clone(),
            producer: self.producer.clone(),
            binding: EvidenceBinding {
                kind: BindingKind::Source,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: None,
            },
            validity,
            completeness: Completeness::Partial,
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self
                .diagnostics
                .iter()
                .filter_map(|d| d.code_ref.clone())
                .collect(),
            limitations: self.limitations.clone(),
            observed_at,
        })
    }
}
