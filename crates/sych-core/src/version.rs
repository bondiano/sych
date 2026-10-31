//! `schema_version` handling for external JSON documents.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// Major schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// Supported major version marker; deserializing any other value fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(into = "u32")]
pub struct SchemaVersion;

impl From<SchemaVersion> for u32 {
    fn from(_: SchemaVersion) -> Self {
        SCHEMA_VERSION
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match u64::deserialize(d)? {
            v if v == u64::from(SCHEMA_VERSION) => Ok(Self),
            v => Err(D::Error::custom(format!(
                "unsupported schema_version {v}, supported {SCHEMA_VERSION}"
            ))),
        }
    }
}

impl JsonSchema for SchemaVersion {
    fn schema_name() -> Cow<'static, str> {
        "SchemaVersion".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "type": "integer", "const": SCHEMA_VERSION })
    }
}

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("malformed JSON document: {0}")]
    Malformed(serde_json::Error),
    #[error("document has no integer schema_version")]
    MissingVersion,
    #[error("unsupported schema_version {found}, supported {SCHEMA_VERSION}")]
    UnsupportedVersion { found: u64 },
    #[error("document does not match schema_version {SCHEMA_VERSION}: {0}")]
    Invalid(serde_json::Error),
}

#[derive(Deserialize)]
struct VersionProbe {
    schema_version: Option<u64>,
}

/// Decodes a versioned document, reporting an unknown major version before field errors.
pub fn decode<T: DeserializeOwned>(json: &str) -> Result<T, DecodeError> {
    let probe: VersionProbe = serde_json::from_str(json).map_err(DecodeError::Malformed)?;
    match probe.schema_version {
        None => Err(DecodeError::MissingVersion),
        Some(v) if v != u64::from(SCHEMA_VERSION) => {
            Err(DecodeError::UnsupportedVersion { found: v })
        }
        Some(_) => serde_json::from_str(json).map_err(DecodeError::Invalid),
    }
}
