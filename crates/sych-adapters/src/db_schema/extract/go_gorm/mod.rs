//! Static Go GORM model extractor using `tree-sitter-go`.

mod parse;

use std::collections::{BTreeMap, BTreeSet};

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};

use super::naming::{default_column_name, default_table_name};
use super::source::SourceTree;
use super::table_ref_from_qualified;
use parse::{
    GormTag, ParsedField, ParsedStruct, is_scalar_go_type, is_slice_go_type, parse_go_file,
    parse_gorm_tag, parse_validate_tag, strip_go_wrappers,
};

pub(in crate::db_schema::extract) use parse::{extract_struct_tag, node_text, split_go_type};

/// Unique Go struct key: `(package_dir, struct_name)`.
type StructKey = (String, String);

struct PendingRelation {
    fname: String,
    col_prefix: String,
    target_table: String,
    foreign_key: Option<String>,
    references: Option<String>,
}

struct ExtractContext<'a> {
    settings: &'a ModelExtractionSettings,
    struct_tables: &'a BTreeMap<StructKey, String>,
    struct_defs: &'a BTreeMap<StructKey, &'a ParsedStruct>,
}

impl ExtractContext<'_> {
    /// Resolves a Go type reference (`Type` or `pkg.Type`) from `current_dir` to its
    /// [`StructKey`], preferring an exact directory/package match and falling back
    /// to an unambiguous bare-name match across packages.
    fn resolve_struct_key(&self, current_dir: &str, type_text: &str) -> Option<StructKey> {
        let (qual_pkg, base) = split_go_type(type_text);
        if let Some(pkg) = qual_pkg {
            let mut matches = self
                .struct_defs
                .iter()
                .filter(|(_, s)| s.pkg == pkg && s.name == base)
                .map(|(k, _)| k.clone());
            let first = matches.next()?;
            if matches.next().is_none() {
                return Some(first);
            }

            return None;
        }

        let local = (current_dir.to_owned(), base.to_owned());
        if self.struct_defs.contains_key(&local) {
            return Some(local);
        }

        let mut matches = self
            .struct_defs
            .keys()
            .filter(|(_, name)| name == base)
            .cloned();
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }

        Some(first)
    }

    fn resolve_target_table(&self, current_dir: &str, type_text: &str) -> Option<String> {
        let key = self.resolve_struct_key(current_dir, type_text)?;
        self.struct_tables.get(&key).cloned()
    }
}

/// Extracts GORM model tables from Go files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["go"], &settings.paths);
    let mut structs = Vec::new();
    let mut table_methods: BTreeMap<StructKey, Option<String>> = BTreeMap::new();

    for file in &files {
        if file.ends_with("_test.go") {
            continue;
        }

        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("struct") && !src.contains("TableName") {
            continue;
        }

        parse_go_file(file, &src, &mut structs, &mut table_methods);
    }

    let struct_tables: BTreeMap<StructKey, String> = structs
        .iter()
        .map(|s| {
            let key = (s.dir.clone(), s.name.clone());
            let tbl = table_methods
                .get(&key)
                .and_then(Option::as_ref)
                .cloned()
                .unwrap_or_else(|| {
                    default_table_name(&s.name, Some(&s.pkg), &settings.naming, &settings.tables)
                });
            (key, tbl)
        })
        .collect();
    let struct_defs: BTreeMap<StructKey, &ParsedStruct> = structs
        .iter()
        .map(|s| ((s.dir.clone(), s.name.clone()), s))
        .collect();
    let ctx = ExtractContext {
        settings,
        struct_tables: &struct_tables,
        struct_defs: &struct_defs,
    };

    let mut tables = Vec::new();
    let mut join_tables: BTreeMap<String, DbModelTable> = BTreeMap::new();

    for st in &structs {
        if settings
            .exclude
            .iter()
            .any(|e| e == &st.name || e == &format!("{}.{}", st.pkg, st.name))
        {
            continue;
        }

        let key = (st.dir.clone(), st.name.clone());
        let has_method = table_methods.contains_key(&key);
        let explicit = settings.tables.contains_key(&st.name)
            || settings
                .tables
                .contains_key(&format!("{}.{}", st.pkg, st.name));
        if !st.is_gorm && !has_method && !explicit {
            continue;
        }

        let Some(table_name) = struct_tables.get(&key) else {
            continue;
        };
        if let Some((table, st_joins)) = extract_struct_table(&ctx, st, table_name) {
            tables.push(table);
            for (m2m_name, jt) in st_joins {
                join_tables.entry(m2m_name).or_insert(jt);
            }
        }
    }

    tables.extend(join_tables.into_values());
    tables
}

fn extract_struct_table(
    ctx: &ExtractContext<'_>,
    st: &ParsedStruct,
    table_name: &str,
) -> Option<(DbModelTable, Vec<(String, DbModelTable)>)> {
    let mut table = DbModelTable {
        schema: None,
        name: String::new(),
        source: Some(DbSourcePosition {
            file: st.file.clone(),
            line: st.line,
        }),
        columns: Vec::new(),
        primary_key: Vec::new(),
        join_table_for: Vec::new(),
        uniques: Vec::new(),
        relations: Vec::new(),
    };
    table.set_qualified_name(table_name);

    let mut state = TableBuildState {
        table: &mut table,
        composite_uniques: BTreeMap::new(),
        col_names: BTreeSet::new(),
        field_to_col: BTreeMap::new(),
        pending_relations: Vec::new(),
        join_tables: Vec::new(),
    };
    let mut visited = BTreeSet::from([(st.dir.clone(), st.name.clone())]);

    // Pass 1: extract all scalar/embedded columns, composite uniques, M2M join tables and
    // queue candidate relations so field declaration order never affects FK resolution.
    for field in &st.fields {
        process_gorm_field(ctx, st, field, "", &mut visited, &mut state);
    }

    // Pass 2: resolve queued belongs-to relations against columns actually present on this
    // table. Requiring a matching local column avoids misclassifying GORM has-one relations
    // (whose `foreignKey:` column lives on the target table) as local FKs.
    for pending in state.pending_relations {
        let fk_field = pending
            .foreign_key
            .clone()
            .unwrap_or_else(|| format!("{}ID", pending.fname));
        let fk_base = default_column_name(&fk_field, &ctx.settings.naming);
        let prefixed_fk = format!("{}{fk_base}", pending.col_prefix);
        let fk_col = state
            .field_to_col
            .get(&(pending.col_prefix.clone(), fk_field.clone()))
            .cloned()
            .or_else(|| {
                state
                    .col_names
                    .contains(&prefixed_fk)
                    .then_some(prefixed_fk)
            })
            .or_else(|| {
                pending.foreign_key.as_ref().and_then(|_| {
                    state
                        .field_to_col
                        .get(&(String::new(), fk_field.clone()))
                        .cloned()
                        .or_else(|| state.col_names.contains(&fk_base).then_some(fk_base))
                })
            });
        if let Some(fk_col) = fk_col {
            let ref_col = pending.references.as_deref().map_or_else(
                || default_column_name("ID", &ctx.settings.naming),
                |r| default_column_name(r, &ctx.settings.naming),
            );
            state.table.relations.push(DbModelRelation {
                columns: vec![fk_col],
                references: table_ref_from_qualified(&pending.target_table),
                ref_columns: vec![ref_col],
            });
        }
    }

    for cols in state.composite_uniques.into_values() {
        if cols.len() == 1 {
            if let Some(c) = state.table.columns.iter_mut().find(|c| c.name == cols[0]) {
                c.unique = true;
            }
        } else if cols.len() > 1 {
            state.table.uniques.push(DbModelUnique { columns: cols });
        }
    }

    let joins = state.join_tables;
    if table.columns.is_empty() && table.relations.is_empty() && table.primary_key.is_empty() {
        return None;
    }

    Some((table, joins))
}

struct TableBuildState<'a> {
    table: &'a mut DbModelTable,
    composite_uniques: BTreeMap<String, Vec<String>>,
    col_names: BTreeSet<String>,
    /// Maps `(col_prefix, go_field_name)` to physical column name for `foreignKey:` resolution.
    field_to_col: BTreeMap<(String, String), String>,
    pending_relations: Vec<PendingRelation>,
    join_tables: Vec<(String, DbModelTable)>,
}

fn process_gorm_field(
    ctx: &ExtractContext<'_>,
    st: &ParsedStruct,
    field: &ParsedField,
    col_prefix: &str,
    visited: &mut BTreeSet<StructKey>,
    state: &mut TableBuildState<'_>,
) {
    let gorm_tag = extract_struct_tag(&field.tag_raw, "gorm");
    let gorm_parts = parse_gorm_tag(gorm_tag.as_deref().unwrap_or(""));
    if gorm_parts.skip {
        return;
    }

    // Anonymous embeds and named `gorm:"embedded"` / `gorm:"embeddedPrefix:..."` fields
    // flatten their columns into the owning table instead of becoming relations.
    let Some(fname) = field.name.as_deref() else {
        let next_prefix = combine_prefix(col_prefix, gorm_parts.embedded_prefix.as_deref());
        flatten_gorm_embedded(ctx, st, field, &next_prefix, visited, state);
        return;
    };
    if !fname.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return;
    }

    if gorm_parts.embedded {
        let next_prefix = combine_prefix(col_prefix, gorm_parts.embedded_prefix.as_deref());
        flatten_gorm_embedded(ctx, st, field, &next_prefix, visited, state);
        return;
    }

    if let Some(m2m) = &gorm_parts.many2many {
        if let Some(target_tbl) = ctx.resolve_target_table(&st.dir, &field.type_text) {
            let mut jt = DbModelTable {
                schema: None,
                name: String::new(),
                source: Some(DbSourcePosition {
                    file: st.file.clone(),
                    line: field.line,
                }),
                columns: Vec::new(),
                primary_key: Vec::new(),
                join_table_for: vec![
                    state.table.table_ref(),
                    table_ref_from_qualified(&target_tbl),
                ],
                uniques: Vec::new(),
                relations: Vec::new(),
            };
            jt.set_qualified_name(m2m);
            state.join_tables.push((m2m.clone(), jt));
        }

        return;
    }

    let is_slice = is_slice_go_type(&field.type_text);
    if !is_slice
        && !is_scalar_go_type(&field.type_text)
        && let Some(target_table) = ctx.resolve_target_table(&st.dir, &field.type_text)
    {
        state.pending_relations.push(PendingRelation {
            fname: fname.to_owned(),
            col_prefix: col_prefix.to_owned(),
            target_table,
            foreign_key: gorm_parts.foreign_key,
            references: gorm_parts.references,
        });
        return;
    }

    if is_slice
        && !field
            .type_text
            .trim()
            .trim_start_matches('*')
            .starts_with("[]byte")
    {
        return;
    }

    push_scalar_column(ctx, st, field, fname, col_prefix, &gorm_parts, state);
}

fn push_scalar_column(
    ctx: &ExtractContext<'_>,
    st: &ParsedStruct,
    field: &ParsedField,
    fname: &str,
    col_prefix: &str,
    gorm_parts: &GormTag,
    state: &mut TableBuildState<'_>,
) {
    let validate_tag = extract_struct_tag(&field.tag_raw, "validate")
        .or_else(|| extract_struct_tag(&field.tag_raw, "binding"));
    let base_name = gorm_parts
        .column
        .clone()
        .unwrap_or_else(|| default_column_name(fname, &ctx.settings.naming));
    let col_name = format!("{col_prefix}{base_name}");
    state
        .field_to_col
        .insert((col_prefix.to_owned(), fname.to_owned()), col_name.clone());

    if gorm_parts.primary_key && !state.table.primary_key.contains(&col_name) {
        state.table.primary_key.push(col_name.clone());
    }

    let val_info = parse_validate_tag(validate_tag.as_deref().unwrap_or(""));
    if let Some(uq_name) = &gorm_parts.unique_index_name {
        state
            .composite_uniques
            .entry(uq_name.clone())
            .or_default()
            .push(col_name.clone());
    }

    let new_col = DbModelColumn {
        name: col_name.clone(),
        required: gorm_parts.not_null || gorm_parts.primary_key || val_info.required,
        unique: gorm_parts.unique,
        max_length: gorm_parts.size.or(val_info.max_length),
        enum_values: val_info.oneof,
        source: Some(DbSourcePosition {
            file: st.file.clone(),
            line: field.line,
        }),
    };
    upsert_column(state, new_col);
}

fn flatten_gorm_embedded(
    ctx: &ExtractContext<'_>,
    st: &ParsedStruct,
    field: &ParsedField,
    col_prefix: &str,
    visited: &mut BTreeSet<StructKey>,
    state: &mut TableBuildState<'_>,
) {
    let embedded = strip_go_wrappers(&field.type_text);
    if embedded == "gorm.Model" {
        let src_pos = Some(DbSourcePosition {
            file: st.file.clone(),
            line: field.line,
        });
        let id_col = format!(
            "{col_prefix}{}",
            default_column_name("ID", &ctx.settings.naming)
        );
        if !state.table.primary_key.contains(&id_col) {
            state.table.primary_key.push(id_col.clone());
        }

        for (go_name, required) in [
            ("ID", true),
            ("CreatedAt", true),
            ("UpdatedAt", true),
            ("DeletedAt", false),
        ] {
            let name = format!(
                "{col_prefix}{}",
                default_column_name(go_name, &ctx.settings.naming)
            );
            state
                .field_to_col
                .insert((col_prefix.to_owned(), go_name.to_owned()), name.clone());
            upsert_column(
                state,
                DbModelColumn {
                    name,
                    required,
                    unique: false,
                    max_length: None,
                    enum_values: Vec::new(),
                    source: src_pos.clone(),
                },
            );
        }
        return;
    }

    let Some(target_key) = ctx.resolve_struct_key(&st.dir, embedded) else {
        return;
    };
    if !visited.insert(target_key.clone()) {
        return;
    }

    if let Some(target_struct) = ctx.struct_defs.get(&target_key) {
        for sub in &target_struct.fields {
            process_gorm_field(ctx, target_struct, sub, col_prefix, visited, state);
        }
    }

    visited.remove(&target_key);
}

fn combine_prefix(outer: &str, inner: Option<&str>) -> String {
    match inner {
        Some(extra) if !extra.is_empty() => format!("{outer}{extra}"),
        _ => outer.to_owned(),
    }
}

/// Inserts or merges a column by name using a commutative (field-order-independent)
/// policy: `required` and `unique` are unioned, `max_length` keeps the tighter
/// limit, and `enum_values` are merged and sorted deterministically.
fn upsert_column(state: &mut TableBuildState<'_>, col: DbModelColumn) {
    state.col_names.insert(col.name.clone());
    if let Some(existing) = state.table.columns.iter_mut().find(|c| c.name == col.name) {
        existing.required |= col.required;
        existing.unique |= col.unique;
        existing.max_length = match (existing.max_length, col.max_length) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if !col.enum_values.is_empty() {
            let mut merged: BTreeSet<String> = existing.enum_values.drain(..).collect();
            merged.extend(col.enum_values);
            existing.enum_values = merged.into_iter().collect();
        }

        if existing.source.is_none() {
            existing.source = col.source;
        }
    } else {
        state.table.columns.push(col);
    }
}
