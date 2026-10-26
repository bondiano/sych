//! Trusted policy identity (spec §5.2, §11, INV-12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{GitObjectId, PolicySnapshotId, SchemaVersion, Sha256Digest, Timestamp};

/// Where active rules came from. The reviewed head is never a policy source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicySource {
    /// Engine defaults only; no repository policy was accepted.
    EngineDefaults,
    /// Policy read from an accepted ref pinned to a commit.
    TrustedRef {
        ref_name: String,
        commit: GitObjectId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PolicySnapshot {
    pub schema_version: SchemaVersion,
    pub id: PolicySnapshotId,
    pub source: PolicySource,
    pub rules_hash: Sha256Digest,
    pub config_hash: Sha256Digest,
    pub authorization_policy_version: u32,
    pub created_at: Timestamp,
}
