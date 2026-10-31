//! Input freeze and evidence finalization: external inputs are pinned before
//! analysis; derived evidence on the pinned snapshot is added at finalization.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BindingKind, Evidence, EvidenceId, EvidenceKind, EvidenceManifestId, PolicySnapshotId,
    SchemaVersion, Sha256Digest, SourceSnapshotId, Timestamp, Validity, canonical_sha256,
    derived_uuid,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FreezeError {
    #[error("evidence {0} belongs to another source snapshot")]
    WrongSnapshot(EvidenceId),
    #[error("evidence {0} is derived from the snapshot, not an external input")]
    NotExternal(EvidenceId),
    /// Arrived after the input freeze; it plans a successor generation instead.
    #[error("external evidence {0} was not frozen before analysis")]
    LateExternalInput(EvidenceId),
    #[error("cannot hash manifest: {0}")]
    Hash(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalInput {
    pub evidence_id: EvidenceId,
    pub kind: EvidenceKind,
    pub artifact_sha256: Sha256Digest,
}

/// Pinned CI/contract/environment inputs of one run, frozen before analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExternalInputManifest {
    pub schema_version: SchemaVersion,
    /// Hash of snapshot, policy and inputs; `frozen_at` is excluded.
    pub id: Sha256Digest,
    pub source_snapshot_id: SourceSnapshotId,
    pub policy_snapshot_id: PolicySnapshotId,
    /// Sorted by evidence id.
    pub inputs: Vec<ExternalInput>,
    pub frozen_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EvidenceOrigin {
    /// Listed in the frozen external input manifest.
    ExternalInput,
    /// Produced in this run from the pinned snapshot.
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceRecord {
    pub evidence_id: EvidenceId,
    pub kind: EvidenceKind,
    pub origin: EvidenceOrigin,
    pub validity: Validity,
    pub artifact_sha256: Sha256Digest,
}

/// Every input and derived evidence the result used; frozen at finalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceManifest {
    pub schema_version: SchemaVersion,
    pub id: EvidenceManifestId,
    pub source_snapshot_id: SourceSnapshotId,
    pub policy_snapshot_id: PolicySnapshotId,
    pub external_input_manifest_id: Sha256Digest,
    /// Sorted by evidence id.
    pub evidence: Vec<EvidenceRecord>,
    pub frozen_at: Timestamp,
}

fn is_external(ev: &Evidence) -> bool {
    ev.binding.kind != BindingKind::Source
}

fn hash<T: Serialize>(value: &T) -> Result<Sha256Digest, FreezeError> {
    canonical_sha256(value).map_err(|e| FreezeError::Hash(e.to_string()))
}

impl ExternalInputManifest {
    pub fn freeze(
        source_snapshot_id: &SourceSnapshotId,
        policy_snapshot_id: &PolicySnapshotId,
        external: &[Evidence],
        at: Timestamp,
    ) -> Result<Self, FreezeError> {
        let mut inputs = Vec::with_capacity(external.len());
        for ev in external {
            if &ev.source_snapshot_id != source_snapshot_id {
                return Err(FreezeError::WrongSnapshot(ev.id));
            }

            if !is_external(ev) {
                return Err(FreezeError::NotExternal(ev.id));
            }

            inputs.push(ExternalInput {
                evidence_id: ev.id,
                kind: ev.kind,
                artifact_sha256: ev.artifact_sha256.clone(),
            });
        }
        inputs.sort_by_key(|i| i.evidence_id);
        Ok(Self {
            schema_version: SchemaVersion,
            id: hash(&(source_snapshot_id, policy_snapshot_id, &inputs))?,
            source_snapshot_id: source_snapshot_id.clone(),
            policy_snapshot_id: policy_snapshot_id.clone(),
            inputs,
            frozen_at: at,
        })
    }
}

impl EvidenceManifest {
    /// External evidence must match a frozen input by id and digest.
    pub fn freeze(
        external: &ExternalInputManifest,
        evidence: &[Evidence],
        at: Timestamp,
    ) -> Result<Self, FreezeError> {
        let mut records = Vec::with_capacity(evidence.len());
        for ev in evidence {
            if ev.source_snapshot_id != external.source_snapshot_id {
                return Err(FreezeError::WrongSnapshot(ev.id));
            }

            let origin = if is_external(ev) {
                let frozen = external
                    .inputs
                    .iter()
                    .any(|i| i.evidence_id == ev.id && i.artifact_sha256 == ev.artifact_sha256);
                if !frozen {
                    return Err(FreezeError::LateExternalInput(ev.id));
                }

                EvidenceOrigin::ExternalInput
            } else {
                EvidenceOrigin::Derived
            };
            records.push(EvidenceRecord {
                evidence_id: ev.id,
                kind: ev.kind,
                origin,
                validity: ev.validity,
                artifact_sha256: ev.artifact_sha256.clone(),
            });
        }
        records.sort_by_key(|r| r.evidence_id);
        let content = hash(&(&external.id, &records))?;
        Ok(Self {
            schema_version: SchemaVersion,
            id: EvidenceManifestId(derived_uuid(&["evidence_manifest", &content.to_string()])),
            source_snapshot_id: external.source_snapshot_id.clone(),
            policy_snapshot_id: external.policy_snapshot_id.clone(),
            external_input_manifest_id: external.id.clone(),
            evidence: records,
            frozen_at: at,
        })
    }
}
