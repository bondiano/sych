//! Repository-level `.sych/config.yaml` and server-level `sych.yaml` settings
//! for the built-in database and application schema linter.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::config::{
    DbSchemaConfig, DbSchemaExclusion, DbSchemaRuleConfig, ModelExtractionSettings,
};
use super::model::SqlDialect;
use crate::config::{EnvName, Problems};

/// Repository-level schema linter settings inside `.sych/config.yaml`.
///
/// Off by default (`enabled: false`). Read strictly from the trusted target tip
/// so a change cannot weaken rules or hide violations with exclusions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct SchemaLintSettings {
    pub enabled: bool,
    /// Optional dialect hint for static SQL reconstruction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dialect: Option<SqlDialect>,
    /// Optional SQL schema/migration file globs; auto-discovered when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sql_paths: Vec<String>,
    /// Optional `ci.expected_reports[].id` carrying a `db_schema_snapshot`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_report: Option<String>,
    /// Optional `ci.expected_reports[].id` carrying a `db_model_manifest`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_ci_report: Option<String>,
    /// Optional server-side connection name from `sych.yaml` (`schema_lint.connections`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// Static application model extraction settings.
    #[serde(skip_serializing_if = "ModelExtractionSettings::is_default")]
    pub models: ModelExtractionSettings,
    /// Inline rule toggles, severity overrides and options.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, DbSchemaRuleConfig>,
    /// Inline exact exclusions with mandatory reasons.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclusions: Vec<DbSchemaExclusion>,
}

impl SchemaLintSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Converts inline repository settings into a [`DbSchemaConfig`].
    pub fn to_linter_config(&self) -> DbSchemaConfig {
        DbSchemaConfig {
            rules: self.rules.clone(),
            exclusions: self.exclusions.clone(),
            models: self.models.clone(),
        }
    }

    /// Validates settings inside `.sych/config.yaml`.
    pub(crate) fn validate(&self, p: &mut Problems<'_>) {
        self.to_linter_config().validate("schema_lint", p);
        if let Some(report) = &self.ci_report {
            p.check(
                !report.trim().is_empty(),
                "schema_lint.ci_report",
                "must not be empty",
            );
        }

        if let Some(report) = &self.model_ci_report {
            p.check(
                !report.trim().is_empty(),
                "schema_lint.model_ci_report",
                "must not be empty",
            );
        }

        if let Some(conn) = &self.connection {
            p.check(
                !conn.trim().is_empty(),
                "schema_lint.connection",
                "must not be empty",
            );
        }
    }
}

/// Default read-only database inspection timeout in seconds.
pub const DEFAULT_INSPECT_TIMEOUT_SECONDS: u32 = 30;

const fn default_timeout() -> u32 {
    DEFAULT_INSPECT_TIMEOUT_SECONDS
}

/// Trusted server-side read-only database connection for schema inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaDbConnection {
    /// Environment variable holding the DSN (`postgres://`, `mysql://`, `sqlite://`).
    pub dsn_env: EnvName,
    /// Allowed `owner/name` repositories; empty denies all repositories on a server.
    #[serde(default)]
    pub repositories: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

/// Server-level schema linter settings inside trusted `sych.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerSchemaLint {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub connections: BTreeMap<String, SchemaDbConnection>,
}

impl ServerSchemaLint {
    pub fn is_default(&self) -> bool {
        self.connections.is_empty()
    }

    /// Every environment variable referenced by configured DB connections.
    pub fn env_refs(&self) -> Vec<(String, &EnvName)> {
        self.connections
            .iter()
            .map(|(name, conn)| {
                (
                    format!("schema_lint.connections.{name}.dsn_env"),
                    &conn.dsn_env,
                )
            })
            .collect()
    }

    pub(crate) fn validate(&self, p: &mut Problems<'_>) {
        for (name, conn) in &self.connections {
            let base = format!("schema_lint.connections.{name}");
            p.check(
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                base.clone(),
                "connection name must be non-empty [A-Za-z0-9_-]",
            );
            p.check(
                conn.timeout_seconds > 0,
                format!("{base}.timeout_seconds"),
                "must be greater than zero",
            );
            for (i, repo) in conn.repositories.iter().enumerate() {
                p.check(
                    repo.split_once('/')
                        .is_some_and(|(o, n)| !o.is_empty() && !n.is_empty() && !n.contains('/')),
                    format!("{base}.repositories[{i}]"),
                    "must be `owner/name`",
                );
            }
        }
    }
}
