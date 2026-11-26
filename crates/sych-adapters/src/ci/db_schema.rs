//! Import of `DbSchemaSnapshot` and `DbModelManifest` JSON artifacts produced in CI.

use sych_core::CiFacts;
use sych_core::db_schema::{DbModelManifest, DbSchemaSnapshot, DbSchemaSource};

use super::malformed;
use crate::ci::CiError;

/// Parses a normalized database schema snapshot artifact.
pub fn parse_snapshot(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut snapshot: DbSchemaSnapshot =
        serde_json::from_str(text).map_err(|e| malformed(facts, &e))?;
    snapshot.source = DbSchemaSource::CiSnapshot;
    if !snapshot.limitations.is_empty() {
        facts.limitations.push(format!(
            "ARTIFACT_LIMITATIONS: database schema snapshot reports {} limitation(s)",
            snapshot.limitations.len()
        ));
    }

    facts.db_schema_snapshot = Some(Box::new(snapshot));
    Ok(())
}

/// Parses a normalized application model manifest artifact.
pub fn parse_manifest(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let manifest = DbModelManifest::parse(text).map_err(|e| malformed(facts, &e))?;
    if !manifest.limitations.is_empty() {
        facts.limitations.push(format!(
            "ARTIFACT_LIMITATIONS: model manifest reports {} limitation(s)",
            manifest.limitations.len()
        ));
    }

    facts.db_model_manifest = Some(Box::new(manifest));
    Ok(())
}
