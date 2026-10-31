//! Checkpoints of routed tasks: an accepted answer and everything its call
//! depended on. A successor reuses one only when its own task has exactly the same
//! dependencies, so the model saw the same input under the same contract and config; the
//! answer is then validated again against the successor's own context.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::Role;
use crate::{RunId, SchemaVersion, Sha256Digest, canonical_sha256};

/// What a routed task's answer depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDeps {
    pub template: String,
    pub schema_name: String,
    pub role: Role,
    pub chain: Vec<String>,
    /// Digest of the messages as sent, after redaction: every span, evidence, gap and search
    /// result the model was shown, so a negative answer carries the scope it was given.
    pub messages_sha256: Sha256Digest,
    pub schema_sha256: Sha256Digest,
    /// Digest of the trusted model config: profiles, models and their settings.
    pub models_revision: String,
}

/// One accepted answer under the digest of its dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub schema_version: SchemaVersion,
    pub key: Sha256Digest,
    pub deps: CheckpointDeps,
    /// The profile that answered.
    pub profile: String,
    /// The run whose model call produced the answer.
    pub run_id: RunId,
    pub output: serde_json::Value,
}

impl CheckpointDeps {
    pub fn key(&self) -> Option<Sha256Digest> {
        canonical_sha256(self).ok()
    }
}

/// Checkpoints a successor may reuse, by key; one whose key is not the digest of its
/// dependencies is dropped.
pub fn reusable(checkpoints: Vec<Checkpoint>) -> BTreeMap<Sha256Digest, Checkpoint> {
    checkpoints
        .into_iter()
        .filter(|c| c.deps.key().as_ref() == Some(&c.key))
        .map(|c| (c.key.clone(), c))
        .collect()
}
