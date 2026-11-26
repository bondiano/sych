//! Configuration, exact object exclusions and review settings for the database
//! and application schema linter.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{DbSchemaFinding, DbSchemaObject};
use super::rules;
use crate::config::{ConfigError, Problems, RuleSeverity, parse_yaml};

/// Default standalone configuration file path in a repository.
pub const DEFAULT_SCHEMA_LINT_FILE: &str = ".sych/schema-lint.yaml";

/// Supported built-in static application model extractors.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ModelExtractorKind {
    /// Detect all supported ORMs present in the repository automatically.
    Auto,
    /// Disable static model extraction.
    None,
    Gorm,
    Bun,
    Ent,
    Prisma,
    Drizzle,
    Sqlalchemy,
    Django,
    Diesel,
    Seaorm,
}

impl ModelExtractorKind {
    /// All concrete built-in extractors (excluding `Auto` and `None`).
    pub const ALL_CONCRETE: &'static [Self] = &[
        Self::Gorm,
        Self::Bun,
        Self::Ent,
        Self::Prisma,
        Self::Drizzle,
        Self::Sqlalchemy,
        Self::Django,
        Self::Diesel,
        Self::Seaorm,
    ];

    /// Stable lowercase identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Gorm => "gorm",
            Self::Bun => "bun",
            Self::Ent => "ent",
            Self::Prisma => "prisma",
            Self::Drizzle => "drizzle",
            Self::Sqlalchemy => "sqlalchemy",
            Self::Django => "django",
            Self::Diesel => "diesel",
            Self::Seaorm => "seaorm",
        }
    }
}

/// Naming overrides for static ORM extractors when naming is configured at runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ModelNamingOverrides {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub table_prefix: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub singular_table: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_lower_case: bool,
}

impl ModelNamingOverrides {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Model extraction settings in `.sych/config.yaml` or standalone config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ModelExtractionSettings {
    /// Which built-in static extractors run (`[auto]` by default).
    pub extractors: Vec<ModelExtractorKind>,
    /// Optional path globs limiting where model files are scanned.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(skip_serializing_if = "ModelNamingOverrides::is_default")]
    pub naming: ModelNamingOverrides,
    /// Struct/model name (or `pkg.Name`) -> explicit physical table name override.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tables: BTreeMap<String, String>,
    /// Struct/model names to skip during static extraction.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

impl Default for ModelExtractionSettings {
    fn default() -> Self {
        Self {
            extractors: vec![ModelExtractorKind::Auto],
            paths: Vec::new(),
            naming: ModelNamingOverrides::default(),
            tables: BTreeMap::new(),
            exclude: Vec::new(),
        }
    }
}

impl ModelExtractionSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Per-rule toggle, severity override and rule-specific options.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DbSchemaRuleConfig {
    /// `None` uses the rule's default (`true`, except `default_off` rules like `plain-tables`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Optional severity override for review findings (`low`, `medium`, `high`, `critical`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<RuleSeverity>,
    /// Rule-specific options (for example `required-column` and `plain-tables`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, Value>,
}

/// Exact exclusion silencing one rule on one database object, with a mandatory reason.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct DbSchemaExclusion {
    pub rule: String,
    pub object: String,
    pub reason: String,
}

/// Standalone schema linter configuration (`.sych/schema-lint.yaml` or `--config`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct DbSchemaConfig {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, DbSchemaRuleConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<DbSchemaExclusion>,
    #[serde(skip_serializing_if = "ModelExtractionSettings::is_default")]
    pub models: ModelExtractionSettings,
}

impl DbSchemaConfig {
    /// Parses and validates a YAML configuration document.
    pub fn parse_yaml(origin: &str, text: &str) -> Result<Self, Vec<ConfigError>> {
        if text.trim().is_empty() {
            return Ok(Self::default());
        }

        let cfg: Self = parse_yaml(origin, text).map_err(|e| vec![e])?;
        let mut problems = Problems::new(origin);
        cfg.validate("", &mut problems);
        if problems.errors.is_empty() {
            Ok(cfg)
        } else {
            Err(problems.errors)
        }
    }

    /// Validates rule ids, exclusions and object syntax.
    pub(crate) fn validate(&self, prefix: &str, p: &mut Problems<'_>) {
        let key = |suffix: &str| {
            if prefix.is_empty() {
                suffix.to_owned()
            } else {
                format!("{prefix}.{suffix}")
            }
        };
        for (id, rc) in &self.rules {
            if rules::lookup(id).is_none() {
                p.push(key(&format!("rules.{id}")), &format!("unknown rule `{id}`"));
                continue;
            }

            validate_rule_options(id, &rc.options, &key(&format!("rules.{id}.options")), p);
        }

        for (i, ex) in self.exclusions.iter().enumerate() {
            let where_at = key(&format!("exclusions[{i}]"));
            if ex.rule.trim().is_empty() {
                p.push(format!("{where_at}.rule"), "rule is required");
            } else if rules::lookup(&ex.rule).is_none() {
                p.push(
                    format!("{where_at}.rule"),
                    &format!("unknown rule `{}`", ex.rule),
                );
            }

            if ex.object.trim().is_empty() {
                p.push(format!("{where_at}.object"), "object is required");
            } else if let Err(msg) = parse_object(&ex.object) {
                p.push(format!("{where_at}.object"), &msg);
            }

            if ex.reason.trim().is_empty() {
                p.push(
                    format!("{where_at}.reason"),
                    "reason is required and must not be empty",
                );
            }
        }
    }

    /// Reports whether `rule_id` runs under this configuration.
    pub fn is_enabled(&self, rule_id: &str) -> bool {
        let Some(rule) = rules::lookup(rule_id) else {
            return false;
        };
        if let Some(parent) = rule.parent
            && !self.is_enabled(parent)
        {
            return false;
        }

        if let Some(rc) = self.rules.get(rule_id)
            && let Some(enabled) = rc.enabled
        {
            return enabled;
        }

        !rule.default_off
    }

    /// Rule-specific options for `rule_id`, if any.
    pub fn options(&self, rule_id: &str) -> Option<&BTreeMap<String, Value>> {
        self.rules.get(rule_id).map(|rc| &rc.options)
    }

    /// Configured severity override for `rule_id`, if any.
    pub fn severity_override(&self, rule_id: &str) -> Option<RuleSeverity> {
        self.rules.get(rule_id).and_then(|rc| rc.severity)
    }

    /// Filters excluded findings and returns `(kept, unmatched_exclusions)`.
    ///
    /// Exclusions of disabled rules are never reported as unmatched because
    /// their rule did not run.
    pub fn apply(
        &self,
        findings: Vec<DbSchemaFinding>,
    ) -> (Vec<DbSchemaFinding>, Vec<DbSchemaExclusion>) {
        let parsed: Vec<Option<DbSchemaObject>> = self
            .exclusions
            .iter()
            .map(|e| parse_object(&e.object).ok())
            .collect();
        let mut used = vec![false; self.exclusions.len()];
        let mut kept = Vec::new();
        for finding in findings {
            let mut excluded = false;
            for (i, ex) in self.exclusions.iter().enumerate() {
                if ex.rule == finding.rule && parsed[i].as_ref() == Some(&finding.object) {
                    used[i] = true;
                    excluded = true;
                }
            }
            if !excluded {
                kept.push(finding);
            }
        }

        let mut unmatched = Vec::new();
        for (i, ex) in self.exclusions.iter().enumerate() {
            if !used[i] && self.is_enabled(&ex.rule) {
                unmatched.push(ex.clone());
            }
        }

        (kept, unmatched)
    }

    /// Merges `overlay` onto `self`: rules and exclusions combine, overlay keys win.
    pub fn merge_from(&mut self, overlay: &Self) {
        for (id, rc) in &overlay.rules {
            let entry = self.rules.entry(id.clone()).or_default();
            if rc.enabled.is_some() {
                entry.enabled = rc.enabled;
            }

            if rc.severity.is_some() {
                entry.severity = rc.severity;
            }

            for (k, v) in &rc.options {
                entry.options.insert(k.clone(), v.clone());
            }
        }
        self.exclusions.extend(overlay.exclusions.iter().cloned());
        if !overlay.models.is_default() {
            self.models = overlay.models.clone();
        }
    }
}

fn validate_rule_options(
    rule_id: &str,
    options: &BTreeMap<String, Value>,
    path: &str,
    p: &mut Problems<'_>,
) {
    if options.is_empty() {
        return;
    }

    match rule_id {
        rules::REQUIRED_COLUMN => {
            for (k, v) in options {
                match k.as_str() {
                    "column" => {
                        p.check(
                            v.as_str().is_some_and(|s| !s.trim().is_empty()),
                            format!("{path}.column"),
                            "expected a non-empty string column name",
                        );
                    }
                    "tables" | "exclude_tables" => {
                        p.check(
                            v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
                            format!("{path}.{k}"),
                            "expected a list of table names",
                        );
                    }
                    other => p.push(
                        format!("{path}.{other}"),
                        &format!("unknown option `{other}` for rule `{rule_id}`"),
                    ),
                }
            }
        }
        rules::PLAIN_TABLES => {
            for (k, v) in options {
                match k.as_str() {
                    "exclude" | "exclude_prefixes" => {
                        p.check(
                            v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
                            format!("{path}.{k}"),
                            "expected a list of strings",
                        );
                    }
                    "exclude_extensions" => {
                        p.check(
                            v.is_boolean(),
                            format!("{path}.exclude_extensions"),
                            "expected a boolean",
                        );
                    }
                    other => p.push(
                        format!("{path}.{other}"),
                        &format!("unknown option `{other}` for rule `{rule_id}`"),
                    ),
                }
            }
        }
        _ => {
            for k in options.keys() {
                p.push(
                    format!("{path}.{k}"),
                    &format!("rule `{rule_id}` accepts no options"),
                );
            }
        }
    }
}

/// Parses an exclusion object string into a [`DbSchemaObject`].
///
/// Exact formats (no wildcards; schema and table names must not contain dots):
/// - `schema.table`
/// - `schema.table.column:NAME`
/// - `schema.table.index:NAME`
/// - `schema.table.foreign_key:NAME`
/// - `schema.table.trigger:NAME`
/// - `schema.view:NAME`
/// - `schema.function:NAME`
pub fn parse_object(raw: &str) -> Result<DbSchemaObject, String> {
    let parts: Vec<&str> = raw.splitn(3, '.').collect();
    if parts.len() == 2
        && !parts[0].is_empty()
        && let Some((kind, name)) = parts[1].split_once(':')
        && !name.is_empty()
    {
        match kind {
            "view" => {
                return Ok(DbSchemaObject {
                    schema: parts[0].to_owned(),
                    view: Some(name.to_owned()),
                    ..DbSchemaObject::default()
                });
            }
            "function" => {
                return Ok(DbSchemaObject {
                    schema: parts[0].to_owned(),
                    function: Some(name.to_owned()),
                    ..DbSchemaObject::default()
                });
            }
            _ => {}
        }
    }

    if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!(
            "object `{raw}`: want schema.table (optionally .column:NAME, .index:NAME, \
             .foreign_key:NAME or .trigger:NAME), schema.view:NAME or schema.function:NAME"
        ));
    }

    let mut obj = DbSchemaObject {
        schema: parts[0].to_owned(),
        table: Some(parts[1].to_owned()),
        ..DbSchemaObject::default()
    };
    if parts.len() == 3 {
        let Some((kind, name)) = parts[2].split_once(':').filter(|(_, n)| !n.is_empty()) else {
            return Err(format!(
                "object `{raw}`: sub-object must be column:NAME, index:NAME, foreign_key:NAME or trigger:NAME"
            ));
        };
        match kind {
            "column" => obj.column = Some(name.to_owned()),
            "index" => obj.index = Some(name.to_owned()),
            "foreign_key" => obj.foreign_key = Some(name.to_owned()),
            "trigger" => obj.trigger = Some(name.to_owned()),
            other => {
                return Err(format!("object `{raw}`: unknown sub-object kind `{other}`"));
            }
        }
    }

    Ok(obj)
}
