//! `required-column` family: enforces that a scoping column (such as `tenant_id`)
//! exists, is NOT NULL, has a foreign key, leads an index, and leads every unique index.

use std::collections::BTreeMap;

use serde_json::Value;

use super::RuleInput;
use crate::db_schema::model::{DbIndex, DbSchemaFinding, DbSchemaObject, DbTable};

pub const REQUIRED_COLUMN: &str = "required-column";
pub const REQUIRED_COLUMN_MISSING: &str = "required-column-missing";
pub const REQUIRED_COLUMN_NULLABLE: &str = "required-column-nullable";
pub const REQUIRED_COLUMN_NO_FOREIGN_KEY: &str = "required-column-no-foreign-key";
pub const REQUIRED_COLUMN_NO_LEADING_INDEX: &str = "required-column-no-leading-index";
pub const REQUIRED_COLUMN_UNIQUE_INDEX: &str = "required-column-unique-index";

struct RequiredColumnOptions {
    column: String,
    tables: Vec<String>,
    exclude_tables: Vec<String>,
}

impl RequiredColumnOptions {
    fn from_map(map: Option<&BTreeMap<String, Value>>) -> Option<Self> {
        let map = map?;
        let column = map
            .get("column")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?
            .to_owned();
        let list = |key: &str| -> Vec<String> {
            map.get(key)
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Self {
            column,
            tables: list("tables"),
            exclude_tables: list("exclude_tables"),
        })
    }

    fn applies(&self, table: &str) -> bool {
        if self.exclude_tables.iter().any(|t| t == table) {
            return false;
        }

        self.tables.is_empty() || self.tables.iter().any(|t| t == table)
    }
}

fn leads_with(idx: &DbIndex, column: &str) -> bool {
    idx.parts.first().is_some_and(|p| {
        p.column.as_deref() == Some(column) && p.prefix_length.is_none_or(|len| len == 0)
    })
}

fn has_leading_index(table: &DbTable, column: &str) -> bool {
    table
        .primary_key
        .as_ref()
        .is_some_and(|pk| leads_with(pk, column))
        || table
            .indexes
            .iter()
            .any(|idx| !idx.is_partial() && leads_with(idx, column))
}

fn has_foreign_key(table: &DbTable, column: &str) -> bool {
    table
        .foreign_keys
        .iter()
        .any(|fk| fk.columns.iter().any(|c| c == column))
}

fn run_for_tables(
    input: &RuleInput<'_>,
    check: impl Fn(&RequiredColumnOptions, &str, &DbTable) -> Vec<DbSchemaFinding>,
) -> Vec<DbSchemaFinding> {
    let Some(opts) = RequiredColumnOptions::from_map(input.config.options(REQUIRED_COLUMN)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            if opts.applies(&table.name) {
                out.extend(check(&opts, &schema.name, table));
            }
        }
    }
    out
}

pub(super) fn check_missing(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    run_for_tables(input, |o, schema, table| {
        if table.column(&o.column).is_some() {
            return Vec::new();
        }

        vec![DbSchemaFinding {
            rule: REQUIRED_COLUMN_MISSING.to_owned(),
            object: DbSchemaObject {
                schema: schema.to_owned(),
                table: Some(table.name.clone()),
                column: Some(o.column.clone()),
                ..DbSchemaObject::default()
            },
            message: format!("table \"{}\" has no column \"{}\"", table.name, o.column),
            fix: format!(
                "Add a \"{}\" field to the model or migration that defines table \"{}\" and regenerate the schema.",
                o.column, table.name
            ),
            source: table.source.clone(),
        }]
    })
}

pub(super) fn check_nullable(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    run_for_tables(input, |o, schema, table| {
        let Some(col) = table.column(&o.column) else {
            return Vec::new();
        };
        if !col.nullable {
            return Vec::new();
        }

        vec![DbSchemaFinding {
            rule: REQUIRED_COLUMN_NULLABLE.to_owned(),
            object: DbSchemaObject {
                schema: schema.to_owned(),
                table: Some(table.name.clone()),
                column: Some(o.column.clone()),
                ..DbSchemaObject::default()
            },
            message: format!("column \"{}\".\"{}\" is nullable", table.name, o.column),
            fix: format!(
                "Make the \"{}\" field of table \"{}\" required (NOT NULL) and regenerate the schema.",
                o.column, table.name
            ),
            source: col.source.clone().or_else(|| table.source.clone()),
        }]
    })
}

pub(super) fn check_no_foreign_key(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    run_for_tables(input, |o, schema, table| {
        let Some(col) = table.column(&o.column) else {
            return Vec::new();
        };
        if has_foreign_key(table, &col.name) {
            return Vec::new();
        }

        vec![DbSchemaFinding {
            rule: REQUIRED_COLUMN_NO_FOREIGN_KEY.to_owned(),
            object: DbSchemaObject {
                schema: schema.to_owned(),
                table: Some(table.name.clone()),
                column: Some(o.column.clone()),
                ..DbSchemaObject::default()
            },
            message: format!(
                "column \"{}\".\"{}\" has no foreign key",
                table.name, o.column
            ),
            fix: format!(
                "Declare a foreign key relation from column \"{}\" of table \"{}\" to the table it refers to, and regenerate the schema.",
                o.column, table.name
            ),
            source: col.source.clone().or_else(|| table.source.clone()),
        }]
    })
}

pub(super) fn check_no_leading_index(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    run_for_tables(input, |o, schema, table| {
        let Some(col) = table.column(&o.column) else {
            return Vec::new();
        };
        if has_leading_index(table, &col.name) {
            return Vec::new();
        }

        vec![DbSchemaFinding {
            rule: REQUIRED_COLUMN_NO_LEADING_INDEX.to_owned(),
            object: DbSchemaObject {
                schema: schema.to_owned(),
                table: Some(table.name.clone()),
                column: Some(o.column.clone()),
                ..DbSchemaObject::default()
            },
            message: format!(
                "column \"{}\".\"{}\" is not the first column of any index",
                table.name, o.column
            ),
            fix: format!(
                "Add an index on table \"{}\" that starts with \"{}\" and regenerate the schema.",
                table.name, o.column
            ),
            source: col.source.clone().or_else(|| table.source.clone()),
        }]
    })
}

pub(super) fn check_unique_index(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    run_for_tables(input, |o, schema, table| {
        if table.column(&o.column).is_none() {
            return Vec::new();
        }

        let mut out = Vec::new();
        for idx in &table.indexes {
            if idx.unique && !leads_with(idx, &o.column) {
                out.push(DbSchemaFinding {
                    rule: REQUIRED_COLUMN_UNIQUE_INDEX.to_owned(),
                    object: DbSchemaObject {
                        schema: schema.to_owned(),
                        table: Some(table.name.clone()),
                        index: Some(idx.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "unique index \"{}\" on \"{}\" does not start with \"{}\"",
                        idx.name, table.name, o.column
                    ),
                    fix: format!(
                        "Declare the unique index \"{}\" of table \"{}\" with \"{}\" as its first column, then regenerate the schema.",
                        idx.name, table.name, o.column
                    ),
                    source: idx.source.clone().or_else(|| table.source.clone()),
                });
            }
        }
        out
    })
}
