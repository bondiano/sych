//! Pure deterministic database and application schema linter.
//!
//! Compares a normalized database schema snapshot (PostgreSQL, MySQL or SQLite)
//! against structural database rules and an optional application model manifest,
//! with exact exclusions, stale-exclusion detection, idempotent baseline
//! generation and review-pipeline `DB-*` findings.

mod baseline;
mod config;
mod facts;
mod findings;
mod manifest;
mod model;
pub mod rules;
mod settings;

pub use baseline::{BASELINE_REASON, add_baseline};
pub use config::{
    DEFAULT_SCHEMA_LINT_FILE, DbSchemaConfig, DbSchemaExclusion, DbSchemaRuleConfig,
    ModelExtractionSettings, ModelExtractorKind, ModelNamingOverrides, parse_object,
};
pub use facts::{
    DbSchemaChange, DbSchemaFactFinding, DbSchemaFacts, FixedDbSchemaFinding, diff_findings,
    finding_key,
};
pub use findings::{STALE_EXCLUSION_RULE, findings, gaps};
pub use manifest::{
    DbModelColumn, DbModelManifest, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique,
    MODEL_MANIFEST_VERSION, ModelManifestError,
};
pub use model::{
    DbCheck, DbColumn, DbColumnType, DbForeignKey, DbFunction, DbIndex, DbIndexPart, DbSchema,
    DbSchemaFinding, DbSchemaObject, DbSchemaSnapshot, DbSchemaSource, DbSourcePosition, DbTable,
    DbTrigger, DbView, SqlDialect, sort_findings,
};
pub use rules::{ALL_RULES, DbSchemaReport, DbSchemaRuleInfo, RuleSpec, evaluate, review_rule_id};
pub use settings::{
    DEFAULT_INSPECT_TIMEOUT_SECONDS, SchemaDbConnection, SchemaLintSettings, ServerSchemaLint,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_model;
