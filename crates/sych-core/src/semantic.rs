//! Semantic facts from a language server for one side of a snapshot (spec §5.4, §9.4).
//!
//! An answered query is never a proof of global absence: callers via macros, dynamic
//! dispatch or unloaded dependencies may be missing, so answered edges stay `PARTIAL`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    BindingKind, CodeRef, Completeness, Evidence, EvidenceBinding, EvidenceId, EvidenceKind,
    PathClass, Producer, SchemaVersion, SourceSide, SourceSnapshot, SourceSnapshotId, Timestamp,
    TrustClass, Validity, canonical_sha256, derived_uuid,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Rust,
    Typescript,
    Go,
    /// ty, plus tree-sitter syntax facts.
    Python,
}

/// The semantic language of a source path and its LSP language id; `None` for files no
/// semantic backend reads (manifests, docs, data).
pub fn source_language(path: &str) -> Option<(Language, &'static str)> {
    let ext = path.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => (Language::Rust, "rust"),
        "ts" | "mts" | "cts" => (Language::Typescript, "typescript"),
        "tsx" => (Language::Typescript, "typescriptreact"),
        "js" | "mjs" | "cjs" => (Language::Typescript, "javascript"),
        "jsx" => (Language::Typescript, "javascriptreact"),
        "go" => (Language::Go, "go"),
        "py" | "pyi" => (Language::Python, "python"),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QueryStatus {
    Answered,
    /// The server does not advertise the capability, or no backend fits the project.
    Unsupported,
    /// The query does not apply to this kind of symbol, e.g. callers of a struct.
    NotApplicable,
    Failed,
}

/// One query's result: resolved spans inside the snapshot, locations outside it, and what
/// could not be mapped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeSet {
    pub status: QueryStatus,
    pub completeness: Completeness,
    #[serde(default)]
    pub refs: Vec<CodeRef>,
    /// Dependency or toolchain locations, home directory shortened to `~`.
    #[serde(default)]
    pub external: Vec<String>,
    /// Locations that could not become a `CodeRef`, with the reason.
    #[serde(default)]
    pub unresolved: Vec<String>,
}

impl EdgeSet {
    pub fn unsupported() -> Self {
        Self::empty(QueryStatus::Unsupported)
    }

    pub fn not_applicable() -> Self {
        Self::empty(QueryStatus::NotApplicable)
    }

    pub fn failed(reason: String) -> Self {
        Self {
            unresolved: vec![reason],
            ..Self::empty(QueryStatus::Failed)
        }
    }

    fn empty(status: QueryStatus) -> Self {
        Self {
            status,
            completeness: Completeness::Unknown,
            refs: Vec::new(),
            external: Vec::new(),
            unresolved: Vec::new(),
        }
    }
}

/// A changed construct (function, method, type, …) and its static neighbourhood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SymbolFacts {
    pub name: String,
    /// LSP `SymbolKind` name, e.g. `function`, `method`, `struct`.
    pub kind: String,
    /// Enclosing symbols, outermost first.
    #[serde(default)]
    pub container: Vec<String>,
    pub definition: CodeRef,
    pub path_class: PathClass,
    pub references: EdgeSet,
    /// References that read or write the symbol, from the server's highlight kinds; not
    /// applicable to callables.
    pub readers: EdgeSet,
    pub writers: EdgeSet,
    pub callers: EdgeSet,
    /// Whole functions or components that contain the call sites in `callers`.
    #[serde(default)]
    pub calling_constructs: Vec<CodeRef>,
    /// Whole constructs this one calls (wrappers, helpers); calls into dependencies are `external`.
    pub callees: EdgeSet,
    /// Whole definitions of the types the construct names, found through the server.
    pub type_definitions: EdgeSet,
    pub implementations: EdgeSet,
}

/// A server diagnostic on a changed file. `message` is repository-derived data, not an
/// instruction, and compiling without errors is not proof of runtime correctness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagnosticFact {
    pub code_ref: CodeRef,
    /// `error`, `warning`, `information` or `hint`.
    pub severity: Option<String>,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BackendStatus {
    Ready,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticFacts {
    pub schema_version: SchemaVersion,
    pub source_snapshot_id: SourceSnapshotId,
    pub side: SourceSide,
    pub language: Language,
    pub status: BackendStatus,
    /// Server name and version as reported by the server; config hash of what Sych sent.
    pub producer: Option<Producer>,
    /// Negotiated LSP position encoding.
    pub position_encoding: Option<String>,
    /// Toolchain, lockfile and server settings the answers depend on.
    #[serde(default)]
    pub build_configuration: BTreeMap<String, String>,
    /// Changed files of this language on this side that were queried.
    #[serde(default)]
    pub query_scope: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<SymbolFacts>,
    /// Pull diagnostics (`textDocument/diagnostic`) of the queried files.
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticFact>,
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// Stable id of a facts document for one snapshot, side and language.
pub(crate) fn facts_evidence_id(
    kind: &str,
    snapshot: &SourceSnapshotId,
    side: SourceSide,
    language: Language,
) -> EvidenceId {
    let wire = |v: serde_json::Value| v.as_str().unwrap_or_default().to_owned();
    EvidenceId(derived_uuid(&[
        kind,
        &snapshot.0.to_string(),
        &wire(serde_json::to_value(side).unwrap_or_default()),
        &wire(serde_json::to_value(language).unwrap_or_default()),
    ]))
}

impl SemanticFacts {
    /// Stable per snapshot, side and language, so a rerun cites the same evidence.
    pub fn evidence_id(&self) -> EvidenceId {
        facts_evidence_id(
            "semantic",
            &self.source_snapshot_id,
            self.side,
            self.language,
        )
    }

    /// Derived evidence produced in this run from the pinned snapshot; the artifact is this
    /// document and its code refs are the definitions of the queried constructs.
    pub fn to_evidence(
        &self,
        snapshot: &SourceSnapshot,
        observed_at: Timestamp,
    ) -> Result<Evidence, serde_json::Error> {
        let (validity, completeness) = match self.status {
            BackendStatus::Ready => (Validity::Valid, Completeness::Partial),
            BackendStatus::Unsupported => (Validity::Unavailable, Completeness::Unknown),
            BackendStatus::Failed => (Validity::Failed, Completeness::Unknown),
        };
        Ok(Evidence {
            schema_version: SchemaVersion,
            id: self.evidence_id(),
            kind: EvidenceKind::Semantic,
            source_snapshot_id: self.source_snapshot_id.clone(),
            producer: self.producer.clone().unwrap_or_else(|| Producer {
                name: "sych-semantic".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                config_hash: None,
            }),
            binding: EvidenceBinding {
                kind: BindingKind::Source,
                head_oid: Some(snapshot.head.clone()),
                target_tip_oid: Some(snapshot.target_tip.clone()),
                tested_oid: None,
                environment_id: None,
            },
            validity,
            completeness,
            trust_class: TrustClass::DerivedLocal,
            artifact_sha256: canonical_sha256(self)?,
            code_refs: self.symbols.iter().map(|s| s.definition.clone()).collect(),
            limitations: self.limitations.clone(),
            observed_at,
        })
    }
}
