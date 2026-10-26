//! JSON Schemas generated from the DTOs, the single source of truth (spec §25.4).

use schemars::{JsonSchema, Schema, schema_for};

use crate::broker::ContextPackets;

use crate::{
    DiffManifest, Evidence, EvidenceManifest, ExternalInputManifest, PolicySnapshot, ReviewResult,
    RunFacts, SemanticFacts, SourceSnapshot,
};

fn with_name<T: JsonSchema>(name: &'static str) -> (&'static str, Schema) {
    (name, schema_for!(T))
}

/// `(file stem, schema)` for every external versioned document.
pub fn document_schemas() -> [(&'static str, Schema); 10] {
    [
        with_name::<SourceSnapshot>("source-snapshot"),
        with_name::<DiffManifest>("diff-manifest"),
        with_name::<PolicySnapshot>("policy-snapshot"),
        with_name::<Evidence>("evidence"),
        with_name::<ExternalInputManifest>("external-input-manifest"),
        with_name::<EvidenceManifest>("evidence-manifest"),
        with_name::<ReviewResult>("review-result"),
        with_name::<SemanticFacts>("semantic-facts"),
        with_name::<ContextPackets>("context-packets"),
        with_name::<RunFacts>("run-facts"),
    ]
}
