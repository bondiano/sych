//! Versioned application model manifest describing physical table, column,
//! uniqueness and relation promises independently of any specific ORM.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::model::DbSourcePosition;
use crate::SchemaVersion;

/// Supported manifest format major version.
pub const MODEL_MANIFEST_VERSION: u32 = 1;

/// Errors returned while decoding or validating a model manifest document.
#[derive(Debug, Error)]
pub enum ModelManifestError {
    #[error("manifest is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("manifest version {found} is not supported; expected version {MODEL_MANIFEST_VERSION}")]
    UnsupportedVersion { found: i64 },
    #[error("invalid model manifest:\n  {}", .0.join("\n  "))]
    Validation(Vec<String>),
}

/// Application model described with physical database names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelManifest {
    #[serde(alias = "version")]
    pub schema_version: SchemaVersion,
    /// Built-in extractor or adapter name that produced this manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    pub tables: Vec<DbModelTable>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl DbModelManifest {
    /// Creates an empty manifest produced by `extractor`.
    pub fn new(extractor: Option<String>) -> Self {
        Self {
            schema_version: SchemaVersion,
            extractor,
            tables: Vec::new(),
            limitations: Vec::new(),
        }
    }

    /// Parses and validates a JSON model manifest document.
    pub fn parse(input: &str) -> Result<Self, ModelManifestError> {
        let raw: Value = serde_json::from_str(input)?;
        if let Some(obj) = raw.as_object() {
            for key in ["schema_version", "version"] {
                if let Some(v) = obj.get(key).and_then(Value::as_i64)
                    && v != i64::from(MODEL_MANIFEST_VERSION)
                {
                    return Err(ModelManifestError::UnsupportedVersion { found: v });
                }
            }
        }

        let manifest: Self = serde_json::from_value(raw)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates all structural invariants that types alone do not enforce.
    pub fn validate(&self) -> Result<(), ModelManifestError> {
        let mut problems = Vec::new();
        for (ti, table) in self.tables.iter().enumerate() {
            let base = format!("/tables/{ti}");
            if table.name.trim().is_empty() {
                problems.push(format!("at {base}/name: table name must not be empty"));
            }

            if let Some(src) = &table.source {
                validate_pos(src, &format!("{base}/source"), &mut problems);
            }

            if !table.primary_key.is_empty() {
                validate_column_names(
                    &table.primary_key,
                    &format!("{base}/primary_key"),
                    &mut problems,
                );
            }

            if !table.join_table_for.is_empty() && table.join_table_for.len() != 2 {
                problems.push(format!(
                    "at {base}/join_table_for: expected 2 referenced tables, found {}",
                    table.join_table_for.len()
                ));
            }

            for (ji, tref) in table.join_table_for.iter().enumerate() {
                if tref.table.trim().is_empty() {
                    problems.push(format!(
                        "at {base}/join_table_for/{ji}/table: table name must not be empty"
                    ));
                }
            }

            for (ci, col) in table.columns.iter().enumerate() {
                let cpath = format!("{base}/columns/{ci}");
                if col.name.trim().is_empty() {
                    problems.push(format!("at {cpath}/name: column name must not be empty"));
                }

                if col.max_length == Some(0) {
                    problems.push(format!("at {cpath}/max_length: must be at least 1"));
                }

                if let Some(src) = &col.source {
                    validate_pos(src, &format!("{cpath}/source"), &mut problems);
                }

                if !col.enum_values.is_empty() {
                    let unique: BTreeSet<&str> =
                        col.enum_values.iter().map(String::as_str).collect();
                    if unique.len() != col.enum_values.len() {
                        problems.push(format!("at {cpath}/enum: enum values must be unique"));
                    }
                }
            }

            for (ui, uniq) in table.uniques.iter().enumerate() {
                validate_column_names(
                    &uniq.columns,
                    &format!("{base}/uniques/{ui}/columns"),
                    &mut problems,
                );
            }

            for (ri, rel) in table.relations.iter().enumerate() {
                let rpath = format!("{base}/relations/{ri}");
                validate_column_names(&rel.columns, &format!("{rpath}/columns"), &mut problems);
                if rel.references.table.trim().is_empty() {
                    problems.push(format!(
                        "at {rpath}/references/table: referenced table name must not be empty"
                    ));
                }

                if !rel.ref_columns.is_empty() {
                    validate_column_names(
                        &rel.ref_columns,
                        &format!("{rpath}/ref_columns"),
                        &mut problems,
                    );
                    if rel.ref_columns.len() != rel.columns.len() {
                        problems.push(format!(
                            "at {rpath}/ref_columns: {} referenced columns for {} columns",
                            rel.ref_columns.len(),
                            rel.columns.len()
                        ));
                    }
                }
            }
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ModelManifestError::Validation(problems))
        }
    }
}

fn validate_pos(pos: &DbSourcePosition, path: &str, problems: &mut Vec<String>) {
    if pos.file.trim().is_empty() {
        problems.push(format!("at {path}/file: file path must not be empty"));
    }

    if pos.line == 0 {
        problems.push(format!("at {path}/line: line must be at least 1"));
    }
}

fn validate_column_names(cols: &[String], path: &str, problems: &mut Vec<String>) {
    if cols.is_empty() {
        problems.push(format!("at {path}: must contain at least one column"));
        return;
    }

    let mut seen = BTreeSet::new();
    for (i, col) in cols.iter().enumerate() {
        if col.trim().is_empty() {
            problems.push(format!("at {path}/{i}: column name must not be empty"));
        } else if !seen.insert(col.as_str()) {
            problems.push(format!("at {path}/{i}: duplicate column `{col}`"));
        }
    }
}

/// One model entity mapped to a physical database table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
    #[serde(default)]
    pub columns: Vec<DbModelColumn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub primary_key: Vec<String>,
    /// Set on a many-to-many join table: the two tables it joins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub join_table_for: Vec<DbModelTableRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uniques: Vec<DbModelUnique>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<DbModelRelation>,
}

impl DbModelTable {
    /// Reference pointing to this table.
    pub fn table_ref(&self) -> DbModelTableRef {
        DbModelTableRef {
            schema: self.schema.clone(),
            table: self.name.clone(),
        }
    }

    /// Splits a `schema.table` physical name into `(schema, name)`.
    pub fn set_qualified_name(&mut self, qualified: &str) {
        if let Some((schema, table)) = qualified.split_once('.') {
            self.schema = Some(schema.to_owned());
            table.clone_into(&mut self.name);
        } else {
            qualified.clone_into(&mut self.name);
        }
    }
}

/// Reference to a table in a relation or join table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelTableRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub table: String,
}

/// Composite (or single-column) uniqueness promised by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelUnique {
    pub columns: Vec<String>,
}

/// Model relation stored as a foreign key on this table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelRelation {
    pub columns: Vec<String>,
    pub references: DbModelTableRef,
    /// Referenced columns, one per `columns` entry; empty means referenced table's key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ref_columns: Vec<String>,
}

/// One model field mapped to a database column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DbModelColumn {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u32>,
    #[serde(rename = "enum", default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DbSourcePosition>,
}
