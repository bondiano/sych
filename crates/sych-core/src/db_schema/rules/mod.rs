//! Rule registry, structured explanations and deterministic evaluation entry point.

mod catalog;
mod constraints;
mod db_rules;
mod index_shape;
mod model_constraints;
mod model_rules;
mod plain_tables;
mod required_column;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::{DbSchemaConfig, DbSchemaExclusion};
use super::manifest::DbModelManifest;
use super::model::{DbSchemaFinding, DbSchemaSnapshot, SqlDialect, sort_findings};
use crate::{SchemaVersion, Severity};

pub use catalog::ALL_RULES;
pub use db_rules::{
    DUPLICATE_INDEX, FK_WITHOUT_INDEX, NARROW_KEY, NULLABLE_BOOLEAN, PREFIX_REDUNDANT_INDEX,
    TABLE_WITHOUT_PRIMARY_KEY,
};
pub use model_constraints::{ENUM_NOT_ENFORCED, MAX_LENGTH_NOT_ENFORCED};
pub use model_rules::{
    RELATION_WITHOUT_FOREIGN_KEY, REQUIRED_FIELD_NULLABLE_COLUMN,
    UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX, unmapped_tables,
};
pub use plain_tables::PLAIN_TABLES;
pub use required_column::{
    REQUIRED_COLUMN, REQUIRED_COLUMN_MISSING, REQUIRED_COLUMN_NO_FOREIGN_KEY,
    REQUIRED_COLUMN_NO_LEADING_INDEX, REQUIRED_COLUMN_NULLABLE, REQUIRED_COLUMN_UNIQUE_INDEX,
};

/// Everything a rule check may inspect.
pub(super) struct RuleInput<'a> {
    pub snapshot: &'a DbSchemaSnapshot,
    pub manifest: Option<&'a DbModelManifest>,
    pub config: &'a DbSchemaConfig,
    pub options: Option<&'a BTreeMap<String, Value>>,
}

/// Static specification of one built-in database/model schema rule.
#[derive(Clone, Copy)]
pub struct RuleSpec {
    pub id: &'static str,
    pub dialects: &'static [SqlDialect],
    pub parent: Option<&'static str>,
    pub default_off: bool,
    pub requires_model: bool,
    pub default_severity: Severity,
    pub why: &'static str,
    pub fix: &'static str,
    pub exception: &'static str,
    pub(super) check: Option<fn(&RuleInput<'_>) -> Vec<DbSchemaFinding>>,
}

impl RuleSpec {
    /// Whether this rule applies to `dialect`.
    pub fn applies_to(&self, dialect: SqlDialect) -> bool {
        self.dialects.contains(&dialect)
    }

    /// Corresponding Sych review rule ID (`DB-<UPPER_KEBAB>`).
    pub fn review_rule_id(&self) -> String {
        review_rule_id(self.id)
    }

    /// Serializable summary for CLI `rules` and `GET /v1/schema/rules`.
    pub fn info(&self) -> DbSchemaRuleInfo {
        DbSchemaRuleInfo {
            id: self.id.to_owned(),
            review_rule_id: self.review_rule_id(),
            dialects: self.dialects.to_vec(),
            default_enabled: !self.default_off,
            requires_model: self.requires_model,
            default_severity: self.default_severity,
            parent: self.parent.map(str::to_owned),
            why: self.why.to_owned(),
            fix: self.fix.to_owned(),
            exception: self.exception.to_owned(),
        }
    }
}

/// Review-level rule ID for any schema linter rule (`DB-FK-WITHOUT-INDEX`, ...).
pub fn review_rule_id(rule_id: &str) -> String {
    format!("DB-{}", rule_id.to_ascii_uppercase())
}

/// Serializable description of one built-in schema rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DbSchemaRuleInfo {
    pub id: String,
    pub review_rule_id: String,
    pub dialects: Vec<SqlDialect>,
    pub default_enabled: bool,
    pub requires_model: bool,
    pub default_severity: Severity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub why: String,
    pub fix: String,
    pub exception: String,
}

/// Complete deterministic report of a schema check run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DbSchemaReport {
    pub schema_version: SchemaVersion,
    pub dialect: SqlDialect,
    pub findings: Vec<DbSchemaFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unmatched_exclusions: Vec<DbSchemaExclusion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl DbSchemaReport {
    /// Clean only when there are zero findings and zero stale exclusions.
    pub fn passed(&self) -> bool {
        self.findings.is_empty() && self.unmatched_exclusions.is_empty()
    }
}

/// Looks up one built-in rule by kebab-case id or `DB-*` review rule id.
pub fn lookup(id: &str) -> Option<&'static RuleSpec> {
    if let Some(rule) = ALL_RULES.iter().find(|r| r.id == id) {
        return Some(rule);
    }

    let stripped = id.strip_prefix("DB-")?.to_ascii_lowercase();
    ALL_RULES.iter().find(|r| r.id == stripped)
}

/// Rules skipped because they do not apply to `dialect`.
pub fn skipped_rules(dialect: SqlDialect) -> Vec<&'static str> {
    ALL_RULES
        .iter()
        .filter(|r| !r.applies_to(dialect))
        .map(|r| r.id)
        .collect()
}

/// Evaluates all enabled rules against `snapshot` and optional `manifest`,
/// returning findings before exclusions are applied.
pub fn run_raw(
    snapshot: &DbSchemaSnapshot,
    manifest: Option<&DbModelManifest>,
    config: &DbSchemaConfig,
) -> Vec<DbSchemaFinding> {
    let mut findings = Vec::new();
    for rule in ALL_RULES {
        if !rule.applies_to(snapshot.dialect) || !config.is_enabled(rule.id) {
            continue;
        }

        let Some(check) = rule.check else {
            continue;
        };
        let input = RuleInput {
            snapshot,
            manifest,
            config,
            options: config.options(rule.id),
        };
        findings.extend(check(&input));
    }
    sort_findings(&mut findings);
    findings
}

/// Full check evaluation: runs enabled rules, applies configured exclusions,
/// and reports stale exclusions, unmapped model tables and skipped dialect rules.
pub fn evaluate(
    snapshot: &DbSchemaSnapshot,
    manifest: Option<&DbModelManifest>,
    config: &DbSchemaConfig,
) -> DbSchemaReport {
    let raw = run_raw(snapshot, manifest, config);
    let (findings, unmatched_exclusions) = config.apply(raw);
    let warnings = manifest
        .map(|m| unmapped_tables(m, snapshot))
        .unwrap_or_default();
    let skipped = skipped_rules(snapshot.dialect)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut limitations = snapshot.limitations.clone();
    if let Some(m) = manifest {
        limitations.extend(m.limitations.iter().cloned());
    }

    DbSchemaReport {
        schema_version: SchemaVersion,
        dialect: snapshot.dialect,
        findings,
        unmatched_exclusions,
        warnings,
        skipped_rules: skipped,
        limitations,
    }
}
