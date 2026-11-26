//! Model-versus-database consistency rules and unmapped model table warnings.

use std::collections::{BTreeMap, BTreeSet};

use super::RuleInput;
use super::constraints::{name_set, quote_list};
use crate::db_schema::manifest::{
    DbModelColumn, DbModelManifest, DbModelRelation, DbModelTable, DbModelTableRef,
};
use crate::db_schema::model::{
    DbForeignKey, DbIndex, DbSchemaFinding, DbSchemaObject, DbSchemaSnapshot, DbSourcePosition,
    DbTable,
};

pub const REQUIRED_FIELD_NULLABLE_COLUMN: &str = "required-field-nullable-column";
pub const UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX: &str = "unique-field-without-unique-index";
pub const RELATION_WITHOUT_FOREIGN_KEY: &str = "relation-without-foreign-key";

pub(super) struct ModelTablePair<'a> {
    pub schema: &'a str,
    pub model: &'a DbModelTable,
    pub db: &'a DbTable,
}

pub(super) fn matched_tables<'a>(input: &'a RuleInput<'a>) -> Vec<ModelTablePair<'a>> {
    let Some(manifest) = input.manifest else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for mt in &manifest.tables {
        let Some(schema) = input.snapshot.find_schema(mt.schema.as_deref()) else {
            continue;
        };
        if let Some(db) = schema.table(&mt.name) {
            out.push(ModelTablePair {
                schema: &schema.name,
                model: mt,
                db,
            });
        }
    }
    out
}

pub(super) fn source_of(table: &DbModelTable, col: &DbModelColumn) -> Option<DbSourcePosition> {
    col.source.clone().or_else(|| table.source.clone())
}

/// Human-readable warnings for model tables absent from the database or whose
/// schema is ambiguous when the database exposes multiple schemas.
pub fn unmapped_tables(manifest: &DbModelManifest, snapshot: &DbSchemaSnapshot) -> Vec<String> {
    let mut out = Vec::new();
    for mt in &manifest.tables {
        let schema = snapshot.find_schema(mt.schema.as_deref());
        match (schema, mt.schema.as_deref().filter(|s| !s.is_empty())) {
            (None, None) => out.push(format!(
                "manifest table \"{}\" has no schema and the database has {} schemas; set the table's schema in the manifest, so it is not compared",
                mt.name,
                snapshot.schemas.len()
            )),
            (None, Some(s)) => out.push(format!(
                "manifest table \"{s}\".\"{}\": the database has no schema \"{s}\"; the table is not compared",
                mt.name
            )),
            (Some(s), _) => {
                if s.table(&mt.name).is_none() {
                    out.push(format!(
                        "manifest table \"{}\" does not exist in schema \"{}\" of the database; the table is not compared",
                        mt.name, s.name
                    ));
                }
            }
        }
    }
    out
}

pub(super) fn check_required_field_nullable_column(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for mt in matched_tables(input) {
        let pk_cols = mt.db.primary_key_columns();
        for mc in &mt.model.columns {
            if !mc.required {
                continue;
            }

            let Some(dc) = mt.db.column(&mc.name) else {
                continue;
            };
            // Primary key columns cannot hold NULL even when SQLite reports INTEGER PRIMARY KEY as nullable.
            if !dc.nullable || pk_cols.contains(&dc.name.as_str()) {
                continue;
            }

            out.push(DbSchemaFinding {
                rule: REQUIRED_FIELD_NULLABLE_COLUMN.to_owned(),
                object: DbSchemaObject {
                    schema: mt.schema.to_owned(),
                    table: Some(mt.db.name.clone()),
                    column: Some(dc.name.clone()),
                    ..DbSchemaObject::default()
                },
                message: format!(
                    "model field for column \"{}\" of table \"{}\" is required, but the column is nullable",
                    dc.name, mt.db.name
                ),
                fix: format!(
                    "Make column \"{}\" of table \"{}\" NOT NULL: keep the field required in the model, \
                     then regenerate the migration and apply it so the database enforces what the model promises.",
                    dc.name, mt.db.name
                ),
                source: source_of(mt.model, mc),
            });
        }
    }
    out
}

struct ModelUnique {
    columns: Vec<String>,
    source: Option<DbSourcePosition>,
}

fn model_uniques(table: &DbModelTable) -> Vec<ModelUnique> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut add = |cols: Vec<String>, src: Option<DbSourcePosition>| {
        if cols.is_empty() {
            return;
        }

        let key = name_set(&cols).join("\0");
        if seen.insert(key) {
            out.push(ModelUnique {
                columns: cols,
                source: src,
            });
        }
    };
    for col in &table.columns {
        if col.unique {
            add(vec![col.name.clone()], source_of(table, col));
        }
    }
    for uniq in &table.uniques {
        add(uniq.columns.clone(), table.source.clone());
    }
    out
}

pub(super) fn check_unique_field_without_unique_index(
    input: &RuleInput<'_>,
) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for mt in matched_tables(input) {
        for uniq in model_uniques(mt.model) {
            if uniquely_indexed(mt.db, &uniq.columns) {
                continue;
            }

            let obj = DbSchemaObject {
                schema: mt.schema.to_owned(),
                table: Some(mt.db.name.clone()),
                column: Some(uniq.columns.join(",")),
                ..DbSchemaObject::default()
            };
            let (message, fix) = if uniq.columns.len() == 1 {
                let col = &uniq.columns[0];
                (
                    format!(
                        "model field for column \"{col}\" of table \"{}\" is unique, but no unique index or primary key covers the column",
                        mt.db.name
                    ),
                    format!(
                        "Add a unique index on column \"{col}\" of table \"{}\": keep the field unique in the model, \
                         then regenerate the migration and apply it (remove duplicate rows first).",
                        mt.db.name
                    ),
                )
            } else {
                let cols = quote_list(&uniq.columns);
                (
                    format!(
                        "model declares columns {cols} of table \"{}\" unique together, but no unique index or primary key covers them",
                        mt.db.name
                    ),
                    format!(
                        "Add a unique index on columns {cols} of table \"{}\": keep the composite uniqueness in the model, \
                         then regenerate the migration and apply it (remove duplicate rows first).",
                        mt.db.name
                    ),
                )
            };
            out.push(DbSchemaFinding {
                rule: UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX.to_owned(),
                object: obj,
                message,
                fix,
                source: uniq.source,
            });
        }
    }
    out
}

/// Reports whether the primary key or a total, non-prefix unique index consists
/// solely of columns from `cols` (a narrower unique key also forbids duplicates
/// of the full combination).
fn uniquely_indexed(table: &DbTable, cols: &[String]) -> bool {
    if table
        .primary_key
        .as_ref()
        .is_some_and(|pk| within_columns(pk, cols))
    {
        return true;
    }

    table
        .indexes
        .iter()
        .any(|idx| idx.unique && !idx.is_partial() && within_columns(idx, cols))
}

fn within_columns(idx: &DbIndex, cols: &[String]) -> bool {
    !idx.parts.is_empty()
        && idx.parts.iter().all(|part| {
            part.column
                .as_deref()
                .is_some_and(|name| cols.iter().any(|c| c == name))
                && part.prefix_length.is_none_or(|len| len == 0)
        })
}

pub(super) fn check_relation_without_foreign_key(input: &RuleInput<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    for mt in matched_tables(input) {
        for rel in &mt.model.relations {
            if let Some(finding) = relation_finding(input.snapshot, &mt, rel) {
                out.push(finding);
            }
        }
        out.extend(join_table_findings(&mt));
    }
    out
}

fn relation_finding(
    snapshot: &DbSchemaSnapshot,
    mt: &ModelTablePair<'_>,
    rel: &DbModelRelation,
) -> Option<DbSchemaFinding> {
    let mut nullable_extra: Option<Vec<String>> = None;
    for fk in &mt.db.foreign_keys {
        let Some(nullable) = cover_relation(snapshot, mt.schema, mt.db, fk, rel) else {
            continue;
        };
        if nullable.is_empty() {
            return None;
        }

        if nullable_extra.is_none() {
            nullable_extra = Some(nullable);
        }
    }

    let obj = DbSchemaObject {
        schema: mt.schema.to_owned(),
        table: Some(mt.db.name.clone()),
        column: Some(rel.columns.join(",")),
        ..DbSchemaObject::default()
    };
    let (message, fix) = match nullable_extra {
        Some(nullable) => (
            format!(
                "model relates {} of table \"{}\" to table \"{}\", but the foreign key that links them \
                 also holds nullable {}, and under MATCH SIMPLE, the default, a row with it NULL skips the check",
                columns_phrase(&rel.columns),
                mt.db.name,
                rel.references.table,
                columns_phrase(&nullable)
            ),
            format!(
                "Make {} of table \"{}\" NOT NULL so the foreign key checks every row: mark the field \
                 required in the model, then regenerate the migration and apply it (fill the NULLs first).",
                columns_phrase(&nullable),
                mt.db.name
            ),
        ),
        None => (
            format!(
                "model relates {} of table \"{}\" to table \"{}\", but no foreign key does that",
                columns_phrase(&rel.columns),
                mt.db.name,
                rel.references.table
            ),
            format!(
                "Add a foreign key from {} of table \"{}\" to table \"{}\": declare the relation in the model, \
                 then regenerate the migration and apply it (repair orphaned rows first).",
                columns_phrase(&rel.columns),
                mt.db.name,
                rel.references.table
            ),
        ),
    };
    Some(DbSchemaFinding {
        rule: RELATION_WITHOUT_FOREIGN_KEY.to_owned(),
        object: obj,
        message,
        fix,
        source: mt.model.source.clone(),
    })
}

fn columns_phrase(cols: &[String]) -> String {
    if cols.len() == 1 {
        format!("column \"{}\"", cols[0])
    } else {
        format!("columns {}", quote_list(cols))
    }
}

fn refers_to(fk: &DbForeignKey, target: &DbModelTableRef) -> bool {
    if fk.ref_table != target.table {
        return false;
    }

    match target.schema.as_deref().filter(|s| !s.is_empty()) {
        None => true,
        Some(want) => fk.ref_schema.as_deref().is_none_or(|got| got == want),
    }
}

/// Checks whether `fk` covers `rel`, returning the extra FK columns outside `rel`
/// that are nullable (and thus weaken MATCH SIMPLE).
fn cover_relation(
    snapshot: &DbSchemaSnapshot,
    current_schema: &str,
    table: &DbTable,
    fk: &DbForeignKey,
    rel: &DbModelRelation,
) -> Option<Vec<String>> {
    if !refers_to(fk, &rel.references) || fk.ref_columns.len() != fk.columns.len() {
        return None;
    }

    let mut refs: BTreeMap<&str, &str> = BTreeMap::new();
    let mut extra: Vec<&str> = Vec::new();
    for (col, ref_col) in fk.columns.iter().zip(&fk.ref_columns) {
        refs.insert(col.as_str(), ref_col.as_str());
        if !rel.columns.iter().any(|c| c == col) {
            extra.push(col.as_str());
        }
    }

    if fk.columns.len().saturating_sub(extra.len()) != rel.columns.len() {
        return None;
    }

    let want_refs: Vec<String> = if !rel.ref_columns.is_empty() {
        rel.ref_columns.clone()
    } else if !extra.is_empty() {
        let ref_schema_name = fk.ref_schema.as_deref().unwrap_or(current_schema);
        let ref_table = snapshot
            .find_schema(Some(ref_schema_name))
            .and_then(|s| s.table(&fk.ref_table))?;
        let pk: Vec<String> = ref_table
            .primary_key_columns()
            .into_iter()
            .map(str::to_owned)
            .collect();
        if pk.len() != rel.columns.len() {
            return None;
        }

        pk
    } else {
        Vec::new()
    };

    if !want_refs.is_empty() {
        for (col, want) in rel.columns.iter().zip(&want_refs) {
            if refs.get(col.as_str()).copied() != Some(want.as_str()) {
                return None;
            }
        }
    }

    let pk_cols = table.primary_key_columns();
    let mut nullable = Vec::new();
    for col_name in extra {
        let is_nullable = table
            .column(col_name)
            .is_none_or(|c| c.nullable && !pk_cols.contains(&c.name.as_str()));
        if is_nullable {
            nullable.push(col_name.to_owned());
        }
    }

    Some(nullable)
}

fn join_table_findings(mt: &ModelTablePair<'_>) -> Vec<DbSchemaFinding> {
    let mut out = Vec::new();
    let mut used = vec![false; mt.db.foreign_keys.len()];
    let joined: Vec<String> = mt
        .model
        .join_table_for
        .iter()
        .map(|r| r.table.clone())
        .collect();
    for target in &mt.model.join_table_for {
        let matched = mt
            .db
            .foreign_keys
            .iter()
            .enumerate()
            .find(|(i, fk)| !used[*i] && refers_to(fk, target))
            .map(|(i, _)| i);
        if let Some(idx) = matched {
            used[idx] = true;
            continue;
        }

        out.push(DbSchemaFinding {
            rule: RELATION_WITHOUT_FOREIGN_KEY.to_owned(),
            object: DbSchemaObject {
                schema: mt.schema.to_owned(),
                table: Some(mt.db.name.clone()),
                ..DbSchemaObject::default()
            },
            message: format!(
                "table \"{}\" is the join table for {} in the model, but has no foreign key to table \"{}\"",
                mt.db.name,
                quote_list(&joined),
                target.table
            ),
            fix: format!(
                "Add a foreign key from table \"{}\" to table \"{}\": declare the many-to-many relation in the model \
                 so its join table references both sides, then regenerate the migration and apply it.",
                mt.db.name, target.table
            ),
            source: mt.model.source.clone(),
        });
    }
    out
}
