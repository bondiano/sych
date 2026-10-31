//! Identifiers and value formats.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use jiff::Timestamp;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("invalid git object id `{0}`: expected `sha1:<40 hex>` or `sha256:<64 hex>`")]
    GitObjectId(String),
    #[error("invalid sha256 digest `{0}`: expected `sha256:<64 lowercase hex>`")]
    Sha256(String),
    #[error("invalid repository path `{0}`: {1}")]
    RepoPath(String, &'static str),
}

macro_rules! uuid_id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[derive(Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            /// New time-ordered `UUIDv7` identifier.
            pub fn new_v7() -> Self {
                Self(uuid::Uuid::now_v7())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    )*};
}

uuid_id!(
    RunId,
    ResultId,
    EvidenceId,
    EvidenceManifestId,
    ObligationId,
    ReviewUnitId,
    CandidateId,
    FindingId,
    /// One Sych-initiated model request; the idempotency key of its ledger entry.
    AttemptId,
    /// One structured model task: its first attempt and its repair share one conversation.
    TaskId,
    /// One immutable tour of one result revision; regeneration makes a new one.
    TourId,
);

fn is_lower_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Hash algorithm of a Git repository's object format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitHashAlgorithm {
    Sha1,
    Sha256,
}

impl GitHashAlgorithm {
    pub const fn hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }

    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
}

/// Git object id stored with its algorithm, serialized as `sha1:<hex>` or `sha256:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GitObjectId {
    algorithm: GitHashAlgorithm,
    hex: String,
}

impl GitObjectId {
    pub fn new(algorithm: GitHashAlgorithm, hex: &str) -> Result<Self, FormatError> {
        if is_lower_hex(hex, algorithm.hex_len()) {
            Ok(Self {
                algorithm,
                hex: hex.to_owned(),
            })
        } else {
            Err(FormatError::GitObjectId(format!(
                "{}:{hex}",
                algorithm.prefix()
            )))
        }
    }

    pub fn algorithm(&self) -> GitHashAlgorithm {
        self.algorithm
    }

    pub fn hex(&self) -> &str {
        &self.hex
    }
}

impl FromStr for GitObjectId {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (algorithm, hex) = match s.split_once(':') {
            Some(("sha1", hex)) => (GitHashAlgorithm::Sha1, hex),
            Some(("sha256", hex)) => (GitHashAlgorithm::Sha256, hex),
            _ => return Err(FormatError::GitObjectId(s.to_owned())),
        };
        Self::new(algorithm, hex)
    }
}

impl TryFrom<String> for GitObjectId {
    type Error = FormatError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<GitObjectId> for String {
    fn from(id: GitObjectId) -> Self {
        id.to_string()
    }
}

impl fmt::Display for GitObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm.prefix(), self.hex)
    }
}

impl JsonSchema for GitObjectId {
    fn schema_name() -> Cow<'static, str> {
        "GitObjectId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^(sha1:[0-9a-f]{40}|sha256:[0-9a-f]{64})$"
        })
    }
}

/// SHA-256 digest of an artifact or canonical manifest, serialized as `sha256:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn from_bytes(digest: [u8; 32]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        Self(
            digest
                .iter()
                .flat_map(|b| [HEX[usize::from(b >> 4)], HEX[usize::from(b & 0xf)]])
                .map(char::from)
                .collect(),
        )
    }

    pub fn hex(&self) -> &str {
        &self.0
    }
}

impl FromStr for Sha256Digest {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.strip_prefix("sha256:") {
            Some(hex) if is_lower_hex(hex, 64) => Ok(Self(hex.to_owned())),
            _ => Err(FormatError::Sha256(s.to_owned())),
        }
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = FormatError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Sha256Digest> for String {
    fn from(d: Sha256Digest) -> Self {
        d.to_string()
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.0)
    }
}

impl JsonSchema for Sha256Digest {
    fn schema_name() -> Cow<'static, str> {
        "Sha256Digest".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "pattern": "^sha256:[0-9a-f]{64}$" })
    }
}

/// Content-derived identity of a [`crate::SourceSnapshot`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SourceSnapshotId(pub Sha256Digest);

/// Content-derived identity of a [`crate::PolicySnapshot`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct PolicySnapshotId(pub Sha256Digest);

/// Money as whole micro-USD; never a float.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct MicroUsd(pub u64);

/// Path inside a snapshot: relative, `/`-separated, without `.`/`..` or empty segments.
/// It names a tree entry and is never joined to a host path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoPath(String);

impl RepoPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for RepoPath {
    type Err = FormatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let reason = if s.is_empty() {
            Some("empty")
        } else if s.starts_with('/') {
            Some("absolute")
        } else if s.contains('\0') {
            Some("contains NUL")
        } else if s.split('/').any(|seg| matches!(seg, "" | "." | "..")) {
            Some("empty, `.` or `..` segment")
        } else {
            None
        };
        match reason {
            Some(r) => Err(FormatError::RepoPath(s.to_owned(), r)),
            None => Ok(Self(s.to_owned())),
        }
    }
}

impl TryFrom<String> for RepoPath {
    type Error = FormatError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<RepoPath> for String {
    fn from(p: RepoPath) -> Self {
        p.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl JsonSchema for RepoPath {
    fn schema_name() -> Cow<'static, str> {
        "RepoPath".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "string", "minLength": 1 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_object_id_keeps_algorithm() {
        let sha1 = format!("sha1:{}", "a".repeat(40));
        let sha256 = format!("sha256:{}", "b".repeat(64));
        assert_eq!(
            sha1.parse::<GitObjectId>().map(|id| id.algorithm()),
            Ok(GitHashAlgorithm::Sha1)
        );
        assert_eq!(
            sha256.parse::<GitObjectId>().map(|id| id.to_string()),
            Ok(sha256)
        );
        for bad in [
            "a".repeat(40),
            format!("sha1:{}", "a".repeat(64)),
            format!("sha1:{}", "A".repeat(40)),
        ] {
            assert!(bad.parse::<GitObjectId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn repo_path_rejects_escapes() {
        assert!("src/main.rs".parse::<RepoPath>().is_ok());
        assert!("docs/Привет мир.md".parse::<RepoPath>().is_ok());
        assert!(
            "a\\b".parse::<RepoPath>().is_ok(),
            "backslash is a valid Git filename byte"
        );
        for bad in ["", "/etc/passwd", "../x", "a/./b", "a//b", "a/", "a\0b"] {
            assert!(bad.parse::<RepoPath>().is_err(), "{bad}");
        }
    }

    #[test]
    fn sha256_digest_round_trips() {
        let d = Sha256Digest::from_bytes([0xab; 32]);
        assert_eq!(d.to_string().parse::<Sha256Digest>(), Ok(d));
        assert!("sha256:abc".parse::<Sha256Digest>().is_err());
    }
}
