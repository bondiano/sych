//! Static Go GORM model extractor using `tree-sitter-go`.

use std::collections::{BTreeMap, BTreeSet};

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::naming::{default_column_name, default_table_name};
use super::source::SourceTree;

const GO_SCALARS: &[&str] = &[
    "string",
    "bool",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "float32",
    "float64",
    "byte",
    "rune",
    "Time",
    "UUID",
    "Decimal",
    "NullString",
    "NullInt64",
    "NullInt32",
    "NullBool",
    "NullTime",
    "NullFloat64",
    "DeletedAt",
    "RawMessage",
];

struct ParsedStruct {
    file: String,
    line: u32,
    pkg: String,
    name: String,
    fields: Vec<ParsedField>,
    is_gorm: bool,
}

struct ParsedField {
    line: u32,
    name: Option<String>,
    type_text: String,
    tag_raw: String,
}

/// Extracts GORM model tables from Go files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["go"], &settings.paths);
    let mut structs = Vec::new();
    let mut table_methods: BTreeMap<(String, String), String> = BTreeMap::new();

    for file in &files {
        if file.ends_with("_test.go") {
            continue;
        }

        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("gorm") && !src.contains("TableName()") {
            continue;
        }

        parse_go_file(file, &src, &mut structs, &mut table_methods);
    }

    let struct_names: BTreeMap<String, String> = structs
        .iter()
        .map(|s| {
            let tbl = table_methods
                .get(&(s.pkg.clone(), s.name.clone()))
                .cloned()
                .unwrap_or_else(|| {
                    default_table_name(&s.name, Some(&s.pkg), &settings.naming, &settings.tables)
                });
            (s.name.clone(), tbl)
        })
        .collect();

    let mut tables = Vec::new();
    let mut join_tables: BTreeMap<String, DbModelTable> = BTreeMap::new();

    for st in &structs {
        if settings.exclude.iter().any(|e| e == &st.name) {
            continue;
        }

        let has_method = table_methods.contains_key(&(st.pkg.clone(), st.name.clone()));
        let explicit = settings.tables.contains_key(&st.name)
            || settings
                .tables
                .contains_key(&format!("{}.{}", st.pkg, st.name));
        if !st.is_gorm && !has_method && !explicit {
            continue;
        }

        let Some(table_name) = struct_names.get(&st.name) else {
            continue;
        };
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

        let mut composite_uniques: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut col_names: BTreeSet<String> = BTreeSet::new();

        for field in &st.fields {
            process_gorm_field(
                st,
                field,
                settings,
                &struct_names,
                &mut table,
                &mut composite_uniques,
                &mut col_names,
                &mut join_tables,
            );
        }

        for cols in composite_uniques.into_values() {
            if cols.len() == 1 {
                if let Some(c) = table.columns.iter_mut().find(|c| c.name == cols[0]) {
                    c.unique = true;
                }
            } else if cols.len() > 1 {
                table.uniques.push(DbModelUnique { columns: cols });
            }
        }

        if !table.columns.is_empty() || !table.relations.is_empty() || !table.primary_key.is_empty()
        {
            tables.push(table);
        }
    }

    tables.extend(join_tables.into_values());
    tables
}

#[allow(clippy::too_many_arguments)]
fn process_gorm_field(
    st: &ParsedStruct,
    field: &ParsedField,
    settings: &ModelExtractionSettings,
    struct_names: &BTreeMap<String, String>,
    table: &mut DbModelTable,
    composite_uniques: &mut BTreeMap<String, Vec<String>>,
    col_names: &mut BTreeSet<String>,
    join_tables: &mut BTreeMap<String, DbModelTable>,
) {
    if field.name.is_none() {
        handle_gorm_embedded(st, field, settings, table, col_names);
        return;
    }

    let Some(fname) = field.name.as_deref() else {
        return;
    };
    if !fname.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        return;
    }

    let gorm_tag = extract_struct_tag(&field.tag_raw, "gorm");
    let gorm_parts = parse_gorm_tag(gorm_tag.as_deref().unwrap_or(""));
    if gorm_parts.skip {
        return;
    }

    let target_base = strip_go_type(&field.type_text);
    if let Some(m2m) = &gorm_parts.many2many {
        record_gorm_m2m(
            st,
            field,
            m2m,
            target_base,
            struct_names,
            table,
            join_tables,
        );
        return;
    }

    let is_slice = field.type_text.trim().starts_with("[]");
    if !is_slice && struct_names.contains_key(target_base) && !is_scalar_go_type(&field.type_text) {
        record_gorm_relation(
            fname,
            target_base,
            &gorm_parts,
            settings,
            struct_names,
            col_names,
            table,
        );
        return;
    }

    if is_slice && !field.type_text.trim().starts_with("[]byte") {
        return;
    }

    let validate_tag = extract_struct_tag(&field.tag_raw, "validate")
        .or_else(|| extract_struct_tag(&field.tag_raw, "binding"));
    let col_name = gorm_parts
        .column
        .clone()
        .unwrap_or_else(|| default_column_name(fname, &settings.naming));
    col_names.insert(col_name.clone());

    if gorm_parts.primary_key {
        table.primary_key.push(col_name.clone());
    }

    let val_info = parse_validate_tag(validate_tag.as_deref().unwrap_or(""));
    if let Some(uq_name) = &gorm_parts.unique_index_name {
        composite_uniques
            .entry(uq_name.clone())
            .or_default()
            .push(col_name.clone());
    }

    table.columns.push(DbModelColumn {
        name: col_name,
        required: gorm_parts.not_null || gorm_parts.primary_key || val_info.required,
        unique: gorm_parts.unique,
        max_length: gorm_parts.size.or(val_info.max_length),
        enum_values: val_info.oneof,
        source: Some(DbSourcePosition {
            file: st.file.clone(),
            line: field.line,
        }),
    });
}

fn handle_gorm_embedded(
    st: &ParsedStruct,
    field: &ParsedField,
    settings: &ModelExtractionSettings,
    table: &mut DbModelTable,
    col_names: &mut BTreeSet<String>,
) {
    let embedded = field.type_text.trim_start_matches('*');
    if embedded == "gorm.Model" || embedded.ends_with(".Model") {
        let id = default_column_name("ID", &settings.naming);
        table.primary_key.push(id.clone());
        col_names.insert(id.clone());
        table.columns.push(DbModelColumn {
            name: id,
            required: true,
            unique: false,
            max_length: None,
            enum_values: Vec::new(),
            source: Some(DbSourcePosition {
                file: st.file.clone(),
                line: field.line,
            }),
        });
    }
}

fn record_gorm_m2m(
    st: &ParsedStruct,
    field: &ParsedField,
    m2m: &str,
    target_base: &str,
    struct_names: &BTreeMap<String, String>,
    table: &DbModelTable,
    join_tables: &mut BTreeMap<String, DbModelTable>,
) {
    if let Some(target_tbl) = struct_names.get(target_base) {
        join_tables.entry(m2m.to_owned()).or_insert_with(|| {
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
                    table.table_ref(),
                    DbModelTableRef {
                        schema: None,
                        table: target_tbl.clone(),
                    },
                ],
                uniques: Vec::new(),
                relations: Vec::new(),
            };
            jt.set_qualified_name(m2m);
            jt
        });
    }
}

fn record_gorm_relation(
    fname: &str,
    target_base: &str,
    gorm_parts: &GormTag,
    settings: &ModelExtractionSettings,
    struct_names: &BTreeMap<String, String>,
    col_names: &BTreeSet<String>,
    table: &mut DbModelTable,
) {
    let target_tbl = &struct_names[target_base];
    let fk_field = gorm_parts
        .foreign_key
        .clone()
        .unwrap_or_else(|| format!("{fname}ID"));
    let fk_col = default_column_name(&fk_field, &settings.naming);
    let ref_col = gorm_parts.references.as_deref().map_or_else(
        || default_column_name("ID", &settings.naming),
        |r| default_column_name(r, &settings.naming),
    );
    if gorm_parts.foreign_key.is_some() || col_names.contains(&fk_col) {
        table.relations.push(DbModelRelation {
            columns: vec![fk_col],
            references: DbModelTableRef {
                schema: None,
                table: target_tbl.clone(),
            },
            ref_columns: vec![ref_col],
        });
    }
}

fn parse_go_file(
    file: &str,
    src: &str,
    structs: &mut Vec<ParsedStruct>,
    table_methods: &mut BTreeMap<(String, String), String>,
) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .is_err()
    {
        return;
    }

    let Some(tree) = parser.parse(src, None) else {
        return;
    };
    let root = tree.root_node();
    let mut pkg = String::new();
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "package_clause" => {
                if let Some(id) = child.named_child(0) {
                    node_text(id, src).clone_into(&mut pkg);
                }
            }
            "type_declaration" => {
                let mut tc = child.walk();
                for spec in child.named_children(&mut tc) {
                    if spec.kind() != "type_spec" {
                        continue;
                    }

                    let Some(name_node) = spec.child_by_field_name("name") else {
                        continue;
                    };
                    let Some(type_node) = spec.child_by_field_name("type") else {
                        continue;
                    };
                    if type_node.kind() != "struct_type" {
                        continue;
                    }

                    let parsed =
                        parse_go_struct(file, &pkg, node_text(name_node, src), type_node, src);
                    structs.push(parsed);
                }
            }
            "method_declaration" => {
                if let Some((recv, tbl)) = parse_table_name_method(child, src) {
                    table_methods.insert((pkg.clone(), recv), tbl);
                }
            }
            _ => {}
        }
    }
}

fn parse_go_struct(
    file: &str,
    pkg: &str,
    name: &str,
    struct_node: Node<'_>,
    src: &str,
) -> ParsedStruct {
    let line = u32::try_from(struct_node.start_position().row + 1).unwrap_or(u32::MAX);
    let mut fields = Vec::new();
    let mut is_gorm = false;
    let mut cursor = struct_node.walk();
    for child in struct_node.named_children(&mut cursor) {
        if child.kind() != "field_declaration_list" {
            continue;
        }

        let mut fc = child.walk();
        for fnode in child.named_children(&mut fc) {
            if fnode.kind() != "field_declaration" {
                continue;
            }

            let fline = u32::try_from(fnode.start_position().row + 1).unwrap_or(u32::MAX);
            let type_text = fnode
                .child_by_field_name("type")
                .map(|n| node_text(n, src).to_owned())
                .unwrap_or_default();
            let tag_raw = fnode
                .child_by_field_name("tag")
                .map(|n| node_text(n, src).trim_matches('`').to_owned())
                .unwrap_or_default();
            if tag_raw.contains("gorm:\"") || type_text.trim_start_matches('*') == "gorm.Model" {
                is_gorm = true;
            }

            let name_node = fnode.child_by_field_name("name");
            fields.push(ParsedField {
                line: fline,
                name: name_node.map(|n| node_text(n, src).to_owned()),
                type_text,
                tag_raw,
            });
        }
    }
    ParsedStruct {
        file: file.to_owned(),
        line,
        pkg: pkg.to_owned(),
        name: name.to_owned(),
        fields,
        is_gorm,
    }
}

fn parse_table_name_method(node: Node<'_>, src: &str) -> Option<(String, String)> {
    let name = node_text(node.child_by_field_name("name")?, src);
    if name != "TableName" {
        return None;
    }

    let recv = node_text(node.child_by_field_name("receiver")?, src);
    let recv_type = recv
        .trim_matches(['(', ')'])
        .split_whitespace()
        .last()?
        .trim_start_matches('*')
        .to_owned();
    let body = node_text(node.child_by_field_name("body")?, src);
    let ret_pos = body.find("return")?;
    let rest = &body[ret_pos + "return".len()..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some((recv_type, rest[q1..q1 + q2].to_owned()))
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct GormTag {
    skip: bool,
    column: Option<String>,
    primary_key: bool,
    not_null: bool,
    unique: bool,
    unique_index_name: Option<String>,
    size: Option<u32>,
    foreign_key: Option<String>,
    references: Option<String>,
    many2many: Option<String>,
}

fn parse_gorm_tag(tag: &str) -> GormTag {
    let mut out = GormTag::default();
    for raw in tag.split(';') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }

        if part == "-" {
            out.skip = true;
            return out;
        }

        let (k, v) = part
            .split_once(':')
            .map_or((part, None), |(k, v)| (k, Some(v)));
        match k.trim().to_ascii_lowercase().as_str() {
            "column" => out.column = v.map(|s| s.trim().to_owned()),
            "primarykey" | "primary_key" => out.primary_key = true,
            "not null" | "notnull" => out.not_null = true,
            "unique" => out.unique = true,
            "uniqueindex" | "unique_index" => {
                if let Some(name) = v.map(str::trim).filter(|s| !s.is_empty()) {
                    let clean = name.split(',').next().unwrap_or(name).trim();
                    out.unique_index_name = Some(clean.to_owned());
                } else {
                    out.unique = true;
                }
            }
            "size" => out.size = v.and_then(|s| s.trim().parse::<u32>().ok()),
            "type" => {
                if let Some(ty) = v {
                    let lower = ty.to_ascii_lowercase();
                    if (lower.starts_with("varchar(") || lower.starts_with("char("))
                        && let Some(open) = lower.find('(')
                        && let Some(close) = lower[open + 1..].find(')')
                        && let Ok(n) = lower[open + 1..open + 1 + close].trim().parse::<u32>()
                    {
                        out.size = Some(n);
                    }
                }
            }
            "foreignkey" | "foreign_key" => out.foreign_key = v.map(|s| s.trim().to_owned()),
            "references" => out.references = v.map(|s| s.trim().to_owned()),
            "many2many" => out.many2many = v.map(|s| s.trim().to_owned()),
            _ => {}
        }
    }
    out
}

#[derive(Default)]
struct ValidateTag {
    required: bool,
    max_length: Option<u32>,
    oneof: Vec<String>,
}

fn parse_validate_tag(tag: &str) -> ValidateTag {
    let mut out = ValidateTag::default();
    for part in tag.split(',') {
        let p = part.trim();
        if p == "required" {
            out.required = true;
        } else if let Some(val) = p.strip_prefix("max=") {
            out.max_length = val.trim().parse::<u32>().ok();
        } else if let Some(vals) = p.strip_prefix("oneof=") {
            out.oneof = vals.split_whitespace().map(str::to_owned).collect();
        }
    }
    out
}

pub(super) fn extract_struct_tag(raw: &str, key: &str) -> Option<String> {
    let pat = format!("{key}:\"");
    let start = raw.find(&pat)? + pat.len();
    let rest = &raw[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn strip_go_type(ty: &str) -> &str {
    let s = ty.trim().trim_start_matches("[]").trim_start_matches('*');
    s.rsplit('.').next().unwrap_or(s)
}

fn is_scalar_go_type(ty: &str) -> bool {
    GO_SCALARS.contains(&strip_go_type(ty))
}

pub(super) fn node_text<'a>(node: Node<'_>, src: &'a str) -> &'a str {
    src.get(node.byte_range()).unwrap_or_default()
}
