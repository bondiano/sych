//! Model-versus-database value-constraint rules (`max-length-not-enforced` and `enum-not-enforced`).

use super::RuleInput;
use super::constraints::{check_enum_values, check_max_length, describe_set, name_set};
use super::model_rules::{matched_tables, source_of};
use crate::db_schema::model::{
    DbColumn, DbColumnType, DbSchemaFinding, DbSchemaObject, DbTable, SqlDialect,
};

pub const MAX_LENGTH_NOT_ENFORCED: &str = "max-length-not-enforced";
pub const ENUM_NOT_ENFORCED: &str = "enum-not-enforced";

pub(super) fn check_max_length_not_enforced(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for mt in matched_tables(input) {
        for mc in &mt.model.columns {
            let Some(limit) = mc.max_length.filter(|n| *n > 0) else {
                continue;
            };
            let Some(dc) = mt.db.column(&mc.name) else {
                continue;
            };
            if let Err(detail) = length_enforcement(input.snapshot.dialect, dc, mt.db, limit) {
                out.push(DbSchemaFinding {
                    rule: MAX_LENGTH_NOT_ENFORCED.to_owned(),
                    object: DbSchemaObject {
                        schema: mt.schema.to_owned(),
                        table: Some(mt.db.name.clone()),
                        column: Some(dc.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "model limits column \"{}\" of table \"{}\" to {limit} characters, but the database does not enforce it: {detail}",
                        dc.name, mt.db.name
                    ),
                    fix: format!(
                        "Limit column \"{}\" of table \"{}\" to {limit} characters: keep the maximum length in the model, \
                         then regenerate the migration so the column gets a length-limited type or a CHECK on its length, and apply it.",
                        dc.name, mt.db.name
                    ),
                    source: source_of(mt.model, mc),
                });
            }
        }
    }
    out
}

fn length_enforcement(
    dialect: SqlDialect,
    col: &DbColumn,
    table: &DbTable,
    limit: u32,
) -> Result<(), String> {
    let mut detail = "neither the column type nor a CHECK constraint limits its length".to_owned();
    // SQLite accepts VARCHAR(n) syntax without enforcing n.
    if dialect != SqlDialect::Sqlite
        && let DbColumnType::String {
            char_max_len: Some(size),
            is_char_type: true,
            ..
        } = &col.data_type
        && *size > 0
    {
        if *size <= limit {
            return Ok(());
        }

        detail = format!("the column type allows {size} characters");
    }

    for ck in &table.checks {
        if let Some(n) = check_max_length(&ck.expression, &col.name) {
            if n <= limit {
                return Ok(());
            }

            detail = format!("a CHECK constraint allows {n} characters");
        }
    }

    Err(detail)
}

pub(super) fn check_enum_not_enforced(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for mt in matched_tables(input) {
        for mc in &mt.model.columns {
            if mc.enum_values.is_empty() {
                continue;
            }

            let Some(dc) = mt.db.column(&mc.name) else {
                continue;
            };
            if let Err(detail) = enum_enforcement(dc, mt.db, &mc.enum_values) {
                let want = describe_set(&mc.enum_values);
                out.push(DbSchemaFinding {
                    rule: ENUM_NOT_ENFORCED.to_owned(),
                    object: DbSchemaObject {
                        schema: mt.schema.to_owned(),
                        table: Some(mt.db.name.clone()),
                        column: Some(dc.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "model restricts column \"{}\" of table \"{}\" to the values {want}, but the database does not enforce that: {detail}",
                        dc.name, mt.db.name
                    ),
                    fix: format!(
                        "Restrict column \"{}\" of table \"{}\" to the values {want}: keep the enum in the model, \
                         then regenerate the migration so the column becomes a native enum or gets a CHECK listing exactly these values, and apply it.",
                        dc.name, mt.db.name
                    ),
                    source: source_of(mt.model, mc),
                });
            }
        }
    }
    out
}

fn enum_enforcement(col: &DbColumn, table: &DbTable, want_raw: &[String]) -> Result<(), String> {
    let mut detail =
        "the column is neither a native enum nor covered by a CHECK listing values".to_owned();
    let want = name_set(want_raw);
    if let DbColumnType::Enum { values, .. } = &col.data_type {
        if name_set(values) == want {
            return Ok(());
        }

        return Err(format!(
            "the column's enum type allows {}",
            describe_set(values)
        ));
    }

    for ck in &table.checks {
        if let Some(vals) = check_enum_values(&ck.expression, &col.name) {
            if name_set(&vals) == want {
                return Ok(());
            }

            detail = format!("a CHECK constraint allows {}", describe_set(&vals));
        }
    }

    Err(detail)
}
