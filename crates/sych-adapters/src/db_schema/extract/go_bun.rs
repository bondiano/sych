//! Static Go Bun ORM model extractor using `tree-sitter-go`.

use std::collections::BTreeMap;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbSourcePosition, ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::go_gorm::{extract_struct_tag, node_text, split_go_type};
use super::naming::{default_column_name, default_table_name};
use super::source::SourceTree;
use super::{dir_of, table_ref_from_qualified};

type StructKey = (String, String);

struct BunStruct {
    file: String,
    dir: String,
    line: u32,
    pkg: String,
    name: String,
    explicit_table: Option<String>,
    fields: Vec<BunField>,
}

struct BunField {
    line: u32,
    name: Option<String>,
    type_text: String,
    bun_tag: String,
}

struct BunRegistry<'a> {
    structs: &'a [BunStruct],
    table_map: &'a BTreeMap<StructKey, String>,
}

impl BunRegistry<'_> {
    /// Resolves a Go type reference (`Type` or `pkg.Type`) from `current_dir`
    /// to its physical table name, preferring exact package/directory matches
    /// and falling back only when a bare name is unambiguous across packages.
    fn resolve_target_table(&self, current_dir: &str, type_text: &str) -> Option<String> {
        let (qual_pkg, base) = split_go_type(type_text);
        if let Some(pkg) = qual_pkg {
            let mut matches = self
                .structs
                .iter()
                .filter(|s| s.pkg == pkg && s.name == base)
                .filter_map(|s| self.table_map.get(&(s.dir.clone(), s.name.clone())));
            let first = matches.next()?;
            if matches.next().is_none() {
                return Some(first.clone());
            }

            return None;
        }

        let local = (current_dir.to_owned(), base.to_owned());
        if let Some(tbl) = self.table_map.get(&local) {
            return Some(tbl.clone());
        }

        let mut matches = self
            .table_map
            .iter()
            .filter(|((_, name), _)| name == base)
            .map(|(_, tbl)| tbl);
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }

        Some(first.clone())
    }
}

/// Extracts Bun ORM model tables from Go files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["go"], &settings.paths);
    let mut structs = Vec::new();
    for file in &files {
        if file.ends_with("_test.go") {
            continue;
        }

        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("bun:\"") && !src.contains("bun.BaseModel") {
            continue;
        }

        parse_file(file, &src, &mut structs);
    }

    let table_map: BTreeMap<StructKey, String> = structs
        .iter()
        .map(|s| {
            let tbl = s.explicit_table.clone().unwrap_or_else(|| {
                default_table_name(&s.name, Some(&s.pkg), &settings.naming, &settings.tables)
            });
            ((s.dir.clone(), s.name.clone()), tbl)
        })
        .collect();
    let registry = BunRegistry {
        structs: &structs,
        table_map: &table_map,
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
        let Some(table_name) = table_map.get(&key) else {
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
        let mut st_joins = Vec::new();

        for field in &st.fields {
            if field.name.is_none() || field.type_text.contains("BaseModel") {
                continue;
            }

            let Some(fname) = field.name.as_deref() else {
                continue;
            };
            if !fname.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                continue;
            }

            apply_bun_field(
                st,
                field,
                fname,
                settings,
                &registry,
                &mut table,
                &mut st_joins,
            );
        }

        if !table.columns.is_empty() || !table.relations.is_empty() || !table.primary_key.is_empty()
        {
            tables.push(table);
            for (m2m_name, jt) in st_joins {
                join_tables.entry(m2m_name).or_insert(jt);
            }
        }
    }

    tables.extend(join_tables.into_values());
    tables
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct BunFieldTag {
    skip: bool,
    col_override: Option<String>,
    pk: bool,
    notnull: bool,
    unique: bool,
    max_len: Option<u32>,
    rel_kind: Option<String>,
    join_spec: Option<String>,
    m2m_table: Option<String>,
}

fn parse_bun_tag(tag: &str) -> BunFieldTag {
    let mut out = BunFieldTag::default();
    for (idx, raw) in tag.split(',').enumerate() {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }

        if idx == 0 && token == "-" {
            out.skip = true;
            return out;
        }

        match token {
            "pk" => out.pk = true,
            "notnull" => out.notnull = true,
            "unique" => out.unique = true,
            _ if token.starts_with("type:") => {
                let ty = token.trim_start_matches("type:").to_ascii_lowercase();
                if (ty.starts_with("varchar(") || ty.starts_with("char("))
                    && let Some(open) = ty.find('(')
                    && let Some(close) = ty[open + 1..].find(')')
                {
                    out.max_len = ty[open + 1..open + 1 + close].trim().parse::<u32>().ok();
                }
            }
            _ if token.starts_with("rel:") => {
                out.rel_kind = Some(token.trim_start_matches("rel:").to_owned());
            }
            _ if token.starts_with("join:") => {
                out.join_spec = Some(token.trim_start_matches("join:").to_owned());
            }
            _ if token.starts_with("m2m:") => {
                out.m2m_table = Some(token.trim_start_matches("m2m:").to_owned());
            }
            _ if idx == 0 && !token.contains(':') && !is_bun_flag(token) => {
                out.col_override = Some(token.to_owned());
            }
            _ => {}
        }
    }
    out
}

fn apply_bun_field(
    st: &BunStruct,
    field: &BunField,
    fname: &str,
    settings: &ModelExtractionSettings,
    registry: &BunRegistry<'_>,
    table: &mut DbModelTable,
    st_joins: &mut Vec<(String, DbModelTable)>,
) {
    let parsed = parse_bun_tag(&field.bun_tag);
    if parsed.skip {
        return;
    }

    if let Some(m2m) = parsed.m2m_table {
        if let Some(target_tbl) = registry.resolve_target_table(&st.dir, &field.type_text) {
            let mut jt = DbModelTable {
                schema: None,
                name: String::new(),
                source: Some(DbSourcePosition {
                    file: st.file.clone(),
                    line: field.line,
                }),
                columns: Vec::new(),
                primary_key: Vec::new(),
                join_table_for: vec![table.table_ref(), table_ref_from_qualified(&target_tbl)],
                uniques: Vec::new(),
                relations: Vec::new(),
            };
            jt.set_qualified_name(&m2m);
            st_joins.push((m2m, jt));
        }

        return;
    }

    if let Some(kind) = parsed.rel_kind {
        if kind == "belongs-to"
            && let Some(join) = parsed.join_spec
            && let Some((lhs, rhs)) = join.split_once('=')
            && let Some(target_tbl) = registry.resolve_target_table(&st.dir, &field.type_text)
        {
            table.relations.push(DbModelRelation {
                columns: vec![lhs.trim().to_owned()],
                references: table_ref_from_qualified(&target_tbl),
                ref_columns: vec![rhs.trim().to_owned()],
            });
        }

        return;
    }

    let col_name = parsed
        .col_override
        .unwrap_or_else(|| default_column_name(fname, &settings.naming));
    if parsed.pk {
        table.primary_key.push(col_name.clone());
    }

    table.columns.push(DbModelColumn {
        name: col_name,
        required: parsed.notnull || parsed.pk,
        unique: parsed.unique,
        max_length: parsed.max_len,
        enum_values: Vec::new(),
        source: Some(DbSourcePosition {
            file: st.file.clone(),
            line: field.line,
        }),
    });
}

fn is_bun_flag(token: &str) -> bool {
    matches!(
        token,
        "pk" | "notnull" | "unique" | "autoincrement" | "identity" | "nullzero" | "soft_delete"
    )
}

fn parse_file(file: &str, src: &str, out: &mut Vec<BunStruct>) {
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
        if child.kind() == "package_clause" {
            if let Some(id) = child.named_child(0) {
                node_text(id, src).clone_into(&mut pkg);
            }
        } else if child.kind() == "type_declaration" {
            let mut tc = child.walk();
            for spec in child.named_children(&mut tc) {
                if spec.kind() != "type_spec" {
                    continue;
                }

                let (Some(name_node), Some(type_node)) = (
                    spec.child_by_field_name("name"),
                    spec.child_by_field_name("type"),
                ) else {
                    continue;
                };
                if type_node.kind() != "struct_type" {
                    continue;
                }

                if let Some(st) =
                    parse_struct(file, &pkg, node_text(name_node, src), type_node, src)
                {
                    out.push(st);
                }
            }
        }
    }
}

fn parse_struct(file: &str, pkg: &str, name: &str, node: Node<'_>, src: &str) -> Option<BunStruct> {
    let line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
    let mut fields = Vec::new();
    let mut is_bun = false;
    let mut explicit_table = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
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
            let raw_tag = fnode
                .child_by_field_name("tag")
                .map(|n| node_text(n, src).trim_matches('`').to_owned())
                .unwrap_or_default();
            let bun_tag = extract_struct_tag(&raw_tag, "bun").unwrap_or_default();
            if !bun_tag.is_empty() || type_text.contains("bun.BaseModel") {
                is_bun = true;
            }

            if type_text.contains("BaseModel") {
                for part in bun_tag.split(',') {
                    if let Some(t) = part.trim().strip_prefix("table:") {
                        explicit_table = Some(t.trim().to_owned());
                    }
                }
            }

            fields.push(BunField {
                line: fline,
                name: fnode
                    .child_by_field_name("name")
                    .map(|n| node_text(n, src).to_owned()),
                type_text,
                bun_tag,
            });
        }
    }

    is_bun.then(|| BunStruct {
        file: file.to_owned(),
        dir: dir_of(file),
        line,
        pkg: pkg.to_owned(),
        name: name.to_owned(),
        explicit_table,
        fields,
    })
}
