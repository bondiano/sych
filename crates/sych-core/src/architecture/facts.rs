//! What architecture review found in one project, as the checks computed it: each finding with
//! what the diff base says of it, the base's findings head no longer has, and how complete
//! each check is. No model writes these; a model's explanation of them is a finding of its own.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Change, Check, Limits};
use crate::{
    BackendStatus, BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId,
    EvidenceKind, Language, Producer, SchemaVersion, SourceSnapshot, SourceSnapshotId, Timestamp,
    TrustClass, Validity, canonical_sha256, derived_uuid,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchitectureFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    /// The pinned indexer and its version; `config_hash` covers the limits.
    pub producer: Producer,
    pub language: Language,
    /// The project's directory, `""` for the repository's root.
    pub project: String,
    pub status: BackendStatus,
    /// The diff base was checked: without it no finding is new or old.
    pub base_checked: bool,
    pub limits: Limits,
    /// Per check: what it proves. A circle found stands; an index that cannot see some code
    /// does not prove that no other one is there.
    pub coverage: Vec<CheckCoverage>,
    pub findings: Vec<ArchitectureFinding>,
    /// Findings of the diff base that head no longer has.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<FixedFinding>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckCoverage {
    pub check: Check,
    pub completeness: Completeness,
    /// Why it is partial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArchitectureFinding {
    pub check: Check,
    /// The members' names, the field's, the module's or the path: never a line.
    pub key: String,
    pub message: String,
    pub notes: Vec<String>,
    /// The shape of what was found, drawn in box-drawing characters.
    pub picture: Vec<String>,
    /// Members of the circle, places the field is known, functions shown, directory members
    /// or lines.
    pub size: usize,
    /// Against the diff base; `None` when it was not checked.
    pub change: Option<Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_size: Option<usize>,
    /// Where it is reported, on head; a finding in a file the change does not touch has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_ref: Option<CodeRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixedFinding {
    pub check: Check,
    pub key: String,
    pub message: String,
}

impl ArchitectureFacts {
    pub fn evidence_id(&self) -> EvidenceId {
        EvidenceId(derived_uuid(&[
            "architecture",
            &self.producer.name,
            &self.source_snapshot_id.0.to_string(),
            &self.project,
        ]))
    }

    /// Derived static evidence, complete only where every check is.
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
        let complete = self
            .coverage
            .iter()
            .all(|c| c.completeness == Completeness::CompleteForQuery);
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
            completeness: if complete {
                Completeness::CompleteForQuery
            } else {
                Completeness::Partial
            },
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self
                .findings
                .iter()
                .filter_map(|f| f.code_ref.clone())
                .collect(),
            limitations: self.limitations.clone(),
            observed_at,
        })
    }

    /// Whether a check of this project is partial, and why.
    pub fn partial(&self, check: Check) -> Option<&str> {
        self.coverage
            .iter()
            .find(|c| c.check == check && c.completeness != Completeness::CompleteForQuery)
            .map(|c| c.reason.as_deref().unwrap_or("partial"))
    }
}
