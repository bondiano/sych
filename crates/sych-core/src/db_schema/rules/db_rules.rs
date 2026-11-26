//! Database-only structural schema rules that require no application model.

use std::ptr;

use super::RuleInput;
use super::index_shape::{IndexShape, index_columns};
use crate::db_schema::model::{
    DbColumnType, DbForeignKey, DbIndex, DbSchema, DbSchemaFinding, DbSchemaObject, DbTable,
};

pub const TABLE_WITHOUT_PRIMARY_KEY: &str = "table-without-primary-key";
pub const FK_WITHOUT_INDEX: &str = "fk-without-index";
pub const DUPLICATE_INDEX: &str = "duplicate-index";
pub const PREFIX_REDUNDANT_INDEX: &str = "prefix-redundant-index";
pub const NULLABLE_BOOLEAN: &str = "nullable-boolean";
pub const NARROW_KEY: &str = "narrow-key";

pub(super) fn check_table_without_primary_key(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            if table
                .primary_key
                .as_ref()
                .is_some_and(|pk| !pk.parts.is_empty())
            {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: TABLE_WITHOUT_PRIMARY_KEY.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    table: Some(table.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!("table \"{}\" has no primary key", table.name),
                fix: format!(
                    "Declare a primary key for the model or migration that defines table \"{}\" \
                     (for example an id column marked as PRIMARY KEY), then regenerate the schema.",
                    table.name
                ),
                source: table.source.clone(),
            });
        }
    }
    out
}

pub(super) fn check_fk_without_index(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            for fk in &table.foreign_keys {
                if fk.columns.is_empty() || fk_indexed(table, fk) {
                    continue;
                }

                let quoted: Vec<String> = fk.columns.iter().map(|c| format!("\"{c}\"")).collect();
                let cols = quoted.join(", ");
                let (plural, lead) = if quoted.len() > 1 {
                    (
                        format!("columns {cols} are"),
                        format!("whose leading columns are {cols}"),
                    )
                } else {
                    (
                        format!("column {cols} is"),
                        format!("whose leading column is {cols}"),
                    )
                };
                out.push(DbSchemaFinding {
                    rule: FK_WITHOUT_INDEX.to_owned(),
                    object: DbSchemaObject {
                        schema: schema.name.clone(),
                        table: Some(table.name.clone()),
                        foreign_key: Some(fk.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "foreign key \"{}\" on table \"{}\" has no supporting index: {plural} not indexed",
                        fk.name, table.name
                    ),
                    fix: format!(
                        "Add an index on table \"{}\" {lead} (for example an index declaration in the model or migration), then regenerate the schema.",
                        table.name
                    ),
                    source: fk.source.clone().or_else(|| table.source.clone()),
                });
            }
        }
    }
    out
}

/// Reports whether the foreign key columns form the leading prefix (in any
/// permutation) of the primary key or a non-partial index.
fn fk_indexed(table: &DbTable, fk: &DbForeignKey) -> bool {
    if table
        .primary_key
        .as_ref()
        .is_some_and(|pk| covers_prefix(pk, &fk.columns))
    {
        return true;
    }

    table
        .indexes
        .iter()
        .filter(|idx| !idx.is_partial())
        .any(|idx| covers_prefix(idx, &fk.columns))
}

fn covers_prefix(idx: &DbIndex, cols: &[String]) -> bool {
    if idx.parts.len() < cols.len() {
        return false;
    }

    let mut leading = Vec::with_capacity(cols.len());
    for part in &idx.parts[..cols.len()] {
        let Some(col) = part.column.as_deref() else {
            return false;
        };
        if part.prefix_length.is_some() {
            return false;
        }

        leading.push(col);
    }
    cols.iter().all(|c| leading.contains(&c.as_str()))
}

pub(super) fn check_duplicate_index(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            out.extend(duplicate_indexes(schema, table));
        }
    }
    out
}

fn duplicate_indexes(schema: &DbSchema, table: &DbTable) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    let mut idxs: Vec<&DbIndex> = table.indexes.iter().collect();
    // Name order, so the alphabetically first index of a duplicate group is kept.
    idxs.sort_by(|a, b| a.name.cmp(&b.name));

    let pk = IndexShape::primary_key(table);
    let mut kept: Vec<(&DbIndex, IndexShape)> = Vec::new();

    for idx in idxs {
        let shape = IndexShape::of(idx);
        let kind = if idx.unique { "unique index" } else { "index" };
        if pk.as_ref().is_some_and(|p| shape.same_as(p)) {
            out.push(DbSchemaFinding {
                rule: DUPLICATE_INDEX.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    table: Some(table.name.clone()),
                    index: Some(idx.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "{kind} \"{}\" on table \"{}\" duplicates the primary key: both are keyed on ({})",
                    idx.name,
                    table.name,
                    index_columns(idx).join(", ")
                ),
                fix: format!(
                    "Remove \"{}\" from the model or migration (it adds write cost and no query benefit), then regenerate the schema.",
                    idx.name
                ),
                source: idx.source.clone().or_else(|| table.source.clone()),
            });
            continue;
        }

        if let Some((orig, _)) = kept
            .iter()
            .find(|(k, kshape)| k.unique == idx.unique && kshape.same_as(&shape))
        {
            out.push(DbSchemaFinding {
                rule: DUPLICATE_INDEX.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    table: Some(table.name.clone()),
                    index: Some(idx.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "{kind} \"{}\" on table \"{}\" duplicates index \"{}\": both are keyed on ({})",
                    idx.name,
                    table.name,
                    orig.name,
                    index_columns(idx).join(", ")
                ),
                fix: format!(
                    "Remove \"{}\" from the model or migration (it adds write cost and no query benefit), then regenerate the schema.",
                    idx.name
                ),
                source: idx.source.clone().or_else(|| table.source.clone()),
            });
            continue;
        }

        kept.push((idx, shape));
    }

    out
}

pub(super) fn check_prefix_redundant_index(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            out.extend(prefix_redundant_indexes(schema, table));
        }
    }
    out
}

struct Coverer<'a> {
    idx: Option<&'a DbIndex>,
    desc: String,
    shape: IndexShape,
    unique: bool,
}

fn prefix_redundant_indexes(schema: &DbSchema, table: &DbTable) -> Vec<DbSchemaFinding> {
    let mut coverers = Vec::new();
    if let Some(pk) = IndexShape::primary_key(table) {
        coverers.push(Coverer {
            idx: None,
            desc: "the primary key".to_owned(),
            shape: pk,
            unique: true,
        });
    }

    for idx in &table.indexes {
        coverers.push(Coverer {
            idx: Some(idx),
            desc: format!("index \"{}\"", idx.name),
            shape: IndexShape::of(idx),
            unique: idx.unique,
        });
    }

    let mut out = Vec::new();
    for idx in &table.indexes {
        let shape = IndexShape::of(idx);
        if idx.unique || !shape.is_btree() || !shape.include.is_empty() || shape.parts.is_empty() {
            continue;
        }

        for coverer in &coverers {
            if coverer.idx.is_some_and(|c| ptr::eq(c, idx))
                || !coverer.shape.is_btree()
                || coverer.shape.predicate != shape.predicate
                || !shape.is_prefix_of(&coverer.shape)
            {
                continue;
            }

            // Same keys and equally non-unique: reported by duplicate-index instead.
            if shape.parts.len() == coverer.shape.parts.len()
                && !coverer.unique
                && shape.include == coverer.shape.include
            {
                continue;
            }

            // Same keys as the primary key: reported by duplicate-index instead.
            if shape.parts.len() == coverer.shape.parts.len() && coverer.idx.is_none() {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: PREFIX_REDUNDANT_INDEX.to_owned(),
                object: DbSchemaObject {
                    schema: schema.name.clone(),
                    table: Some(table.name.clone()),
                    index: Some(idx.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "index \"{}\" on table \"{}\" is redundant: its keys ({}) are the leading keys of {}",
                    idx.name,
                    table.name,
                    index_columns(idx).join(", "),
                    coverer.desc
                ),
                fix: format!(
                    "Remove \"{}\" from the model or migration ({} already serves its queries), then regenerate the schema.",
                    idx.name, coverer.desc
                ),
                source: idx.source.clone().or_else(|| table.source.clone()),
            });
            break;
        }
    }
    out
}

pub(super) fn check_nullable_boolean(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            for col in &table.columns {
                if !col.nullable || !matches!(col.data_type, DbColumnType::Boolean) {
                    continue;
                }

                out.push(DbSchemaFinding {
                    rule: NULLABLE_BOOLEAN.to_owned(),
                    object: DbSchemaObject {
                        schema: schema.name.clone(),
                        table: Some(table.name.clone()),
                        column: Some(col.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "boolean column \"{}\" on table \"{}\" is nullable, so it has three states: true, false and NULL",
                        col.name, table.name
                    ),
                    fix: format!(
                        "Make column \"{}\" NOT NULL with an explicit default, backfill existing NULL rows, then regenerate the schema.",
                        col.name
                    ),
                    source: col.source.clone().or_else(|| table.source.clone()),
                });
            }
        }
    }
    out
}

pub(super) fn check_narrow_key(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for schema in &input.snapshot.schemas {
        for table in &schema.tables {
            if let Some(pk) = &table.primary_key
                && pk.parts.len() == 1
                && let Some(col_name) = pk.parts[0].column.as_deref()
                && let Some(col) = table.column(col_name)
                && let Some((bits, raw)) = narrow_integer(&col.data_type)
            {
                out.push(DbSchemaFinding {
                    rule: NARROW_KEY.to_owned(),
                    object: DbSchemaObject {
                        schema: schema.name.clone(),
                        table: Some(table.name.clone()),
                        column: Some(col.name.clone()),
                        ..DbSchemaObject::default()
                    },
                    message: format!(
                        "primary key column \"{}\" on table \"{}\" is {raw} ({bits} bits), narrower than 64 bits",
                        col.name, table.name
                    ),
                    fix: format!(
                        "Declare the primary key column \"{}\" as a 64-bit integer (bigint), also on the foreign keys that reference it, then regenerate the schema.",
                        col.name
                    ),
                    source: col.source.clone().or_else(|| table.source.clone()),
                });
            }

            for fk in &table.foreign_keys {
                if fk.columns.len() != 1 {
                    continue;
                }

                let Some(col) = table.column(&fk.columns[0]) else {
                    continue;
                };
                if let Some((bits, raw)) = narrow_integer(&col.data_type) {
                    out.push(DbSchemaFinding {
                        rule: NARROW_KEY.to_owned(),
                        object: DbSchemaObject {
                            schema: schema.name.clone(),
                            table: Some(table.name.clone()),
                            column: Some(col.name.clone()),
                            foreign_key: Some(fk.name.clone()),
                            ..DbSchemaObject::default()
                        },
                        message: format!(
                            "foreign key \"{}\" column \"{}\" on table \"{}\" is {raw} ({bits} bits), narrower than 64 bits",
                            fk.name, col.name, table.name
                        ),
                        fix: format!(
                            "Declare column \"{}\" as a 64-bit integer (bigint), matching the key it references, then regenerate the schema.",
                            col.name
                        ),
                        source: col
                            .source
                            .clone()
                            .or_else(|| fk.source.clone())
                            .or_else(|| table.source.clone()),
                    });
                }
            }
        }
    }
    out
}

fn narrow_integer(data_type: &DbColumnType) -> Option<(u8, &str)> {
    match data_type {
        DbColumnType::Integer {
            raw,
            bits: Some(bits),
        } if *bits < 64 => Some((*bits, raw.as_str())),
        _ => None,
    }
}
