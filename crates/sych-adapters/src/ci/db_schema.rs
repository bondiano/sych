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
    facts
        .limitations
        .extend(snapshot.limitations.iter().cloned());
    facts.db_schema_snapshot = Some(Box::new(snapshot));
    Ok(())
}

/// Parses a normalized application model manifest artifact.
pub fn parse_manifest(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let manifest = DbModelManifest::parse(text).map_err(|e| malformed(facts, &e))?;
    facts
        .limitations
        .extend(manifest.limitations.iter().cloned());
    facts.db_model_manifest = Some(Box::new(manifest));
    Ok(())
}
