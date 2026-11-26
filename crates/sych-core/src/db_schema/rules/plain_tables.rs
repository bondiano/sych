//! `plain-tables` (default off): reports database views, triggers and functions
//! that live outside the application model.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::RuleInput;
use crate::db_schema::model::{DbSchemaFinding, DbSchemaObject};

pub const PLAIN_TABLES: &str = "plain-tables";

struct PlainTablesOptions {
    exclude: BTreeSet<String>,
    exclude_prefixes: Vec<String>,
    exclude_extensions: bool,
}

impl PlainTablesOptions {
    fn from_map(map: Option<&BTreeMap<String, Value>>) -> Self {
        let Some(map) = map else {
            return Self {
                exclude: BTreeSet::new(),
                exclude_prefixes: Vec::new(),
                exclude_extensions: false,
            };
        };
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
        Self {
            exclude: list("exclude").into_iter().collect(),
            exclude_prefixes: list("exclude_prefixes"),
            exclude_extensions: map
                .get("exclude_extensions")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    fn is_excluded(&self, name: &str, extension_owned: bool) -> bool {
        (self.exclude_extensions && extension_owned)
            || self.exclude.contains(name)
            || self.exclude_prefixes.iter().any(|p| name.starts_with(p))
    }
}

pub(super) fn check_plain_tables(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let opts = PlainTablesOptions::from_map(input.options);
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for func in &schema.functions {
            if opts.is_excluded(&func.name, func.extension_owned) {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: PLAIN_TABLES.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    function: Some(func.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "function \"{}\" exists in schema \"{}\" but is not part of the model",
                    func.name, schema.name
                ),
                fix: format!(
                    "Move the logic of function \"{}\" into application code and drop it, or exclude it with a written reason (use rules.plain-tables.options for extension and migration-tool objects).",
                    func.name
                ),
                source: None,
            });
        }

        for view in &schema.views {
            if opts.is_excluded(&view.name, view.extension_owned) {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: PLAIN_TABLES.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    view: Some(view.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "view \"{}\" exists in schema \"{}\" but is not part of the model",
                    view.name, schema.name
                ),
                fix: format!(
                    "Move the logic of view \"{}\" into application code and drop it, or exclude it with a written reason (use rules.plain-tables.options for extension and migration-tool objects).",
                    view.name
                ),
                source: None,
            });
        }

        for trigger in &schema.triggers {
            if opts.is_excluded(&trigger.name, trigger.extension_owned) {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: PLAIN_TABLES.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    table: Some(trigger.table.clone()),
                    trigger: Some(trigger.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "trigger \"{}\" on table \"{}\" exists but is not part of the model",
                    trigger.name, trigger.table
                ),
                fix: format!(
                    "Move the logic of trigger \"{}\" into application code and drop it, or exclude it with a written reason (use rules.plain-tables.options for extension and migration-tool objects).",
                    trigger.name
                ),
                source: None,
            });
        }
    }
    out
}
