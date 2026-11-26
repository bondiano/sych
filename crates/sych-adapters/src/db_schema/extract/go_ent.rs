//! Static Go Ent schema extractor (`ent/schema/*.go`) using `tree-sitter-go`.

use std::collections::BTreeMap;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::go_gorm::node_text;
use super::naming::default_table_name;
use super::source::SourceTree;
use super::{dir_of, table_ref_from_qualified};

type SchemaKey = (String, String);

struct EntSchema {
    file: String,
    dir: String,
    pkg: String,
    line: u32,
    name: String,
    table_override: Option<String>,
    id_storage_key: Option<String>,
    fields: Vec<DbModelColumn>,
    uniques: Vec<DbModelUnique>,
    edges: Vec<(String, String)>, // (local_fk_col, target_schema_name)
}

/// Extracts Ent schema definitions and any resolution limitations from Go files in `tree`.
pub fn extract(
    tree: &SourceTree<'_>,
    settings: &ModelExtractionSettings,
) -> (Vec<DbModelTable>, Vec<String>) {
    let files = tree.files_with_extensions(&["go"], &settings.paths);
    let mut parsed_files = Vec::new();
    let mut by_key: BTreeMap<SchemaKey, EntSchema> = BTreeMap::new();

    for file in &files {
        if file.ends_with("_test.go") {
            continue;
        }

        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("ent.Schema") && !src.contains("entgo.io/ent") {
            continue;
        }

        collect_ent_types(file, &src, &mut by_key);
        parsed_files.push((file.clone(), src));
    }

    for (file, src) in &parsed_files {
        collect_ent_methods(file, src, &mut by_key);
    }

    let table_names: BTreeMap<SchemaKey, (String, String)> = by_key
        .iter()
        .map(|(key, s)| {
            let tbl = s.table_override.clone().unwrap_or_else(|| {
                default_table_name(&s.name, Some(&s.pkg), &settings.naming, &settings.tables)
            });
            let pk = s.id_storage_key.clone().unwrap_or_else(|| "id".to_owned());
            (key.clone(), (tbl, pk))
        })
        .collect();

    let mut out = Vec::new();
    let mut limitations = Vec::new();
    for (key, s) in by_key {
        if settings
            .exclude
            .iter()
            .any(|e| e == &s.name || e == &format!("{}.{}", s.pkg, s.name))
        {
            continue;
        }

        let Some((table_name, pk_col)) = table_names.get(&key) else {
            continue;
        };

        let mut table = DbModelTable {
            schema: None,
            name: String::new(),
            source: Some(DbSourcePosition {
                file: s.file.clone(),
                line: s.line,
            }),
            columns: s.fields,
            primary_key: vec![pk_col.clone()],
            join_table_for: Vec::new(),
            uniques: s.uniques,
            relations: Vec::new(),
        };
        table.set_qualified_name(table_name);

        for (local_col, target_schema) in s.edges {
            let (target_tbl, ref_col) = match resolve_edge_target(
                &table_names,
                &s.dir,
                &target_schema,
            ) {
                EdgeTarget::Resolved(tbl, pk) => (tbl, pk),
                EdgeTarget::Missing => {
                    let inferred = default_table_name(
                        &target_schema,
                        None,
                        &settings.naming,
                        &settings.tables,
                    );
                    limitations.push(format!(
                        "ent: edge target `{target_schema}` on `{}` ({}) not found in scanned schemas; inferred `{inferred}`(`id`)",
                        s.name, s.file
                    ));
                    (inferred, "id".to_owned())
                }
                EdgeTarget::Ambiguous => {
                    limitations.push(format!(
                            "ent: ambiguous edge target `{target_schema}` on `{}` ({}); skipped relation on `{local_col}`",
                            s.name, s.file
                        ));
                    continue;
                }
            };
            table.relations.push(DbModelRelation {
                columns: vec![local_col],
                references: table_ref_from_qualified(&target_tbl),
                ref_columns: vec![ref_col],
            });
        }
        out.push(table);
    }
    (out, limitations)
}

enum EdgeTarget {
    Resolved(String, String),
    Missing,
    Ambiguous,
}

fn resolve_edge_target(
    table_names: &BTreeMap<SchemaKey, (String, String)>,
    current_dir: &str,
    target_schema: &str,
) -> EdgeTarget {
    if let Some((tbl, pk)) = table_names.get(&(current_dir.to_owned(), target_schema.to_owned())) {
        return EdgeTarget::Resolved(tbl.clone(), pk.clone());
    }

    let mut matches = table_names
        .iter()
        .filter(|((_, name), _)| name == target_schema)
        .map(|(_, entry)| entry);
    let Some((first_tbl, first_pk)) = matches.next() else {
        return EdgeTarget::Missing;
    };
    if matches.next().is_some() {
        return EdgeTarget::Ambiguous;
    }

    EdgeTarget::Resolved(first_tbl.clone(), first_pk.clone())
}

fn collect_ent_types(file: &str, src: &str, by_key: &mut BTreeMap<SchemaKey, EntSchema>) {
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
    let dir = dir_of(file);
    let mut pkg = String::new();
    let mut ent_alias: Option<String> = None;
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        if child.kind() == "package_clause" {
            if let Some(id) = child.named_child(0) {
                node_text(id, src).clone_into(&mut pkg);
            }
        } else if child.kind() == "import_declaration" {
            collect_ent_import_alias(child, src, &mut ent_alias);
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
                if type_node.kind() == "struct_type"
                    && embeds_ent_schema(type_node, src, ent_alias.as_deref())
                {
                    let name = node_text(name_node, src).to_owned();
                    let line = u32::try_from(spec.start_position().row + 1).unwrap_or(u32::MAX);
                    by_key.insert(
                        (dir.clone(), name.clone()),
                        EntSchema {
                            file: file.to_owned(),
                            dir: dir.clone(),
                            pkg: pkg.clone(),
                            line,
                            name,
                            table_override: None,
                            id_storage_key: None,
                            fields: Vec::new(),
                            uniques: Vec::new(),
                            edges: Vec::new(),
                        },
                    );
                }
            }
        }
    }
}

fn collect_ent_import_alias(node: Node<'_>, src: &str, ent_alias: &mut Option<String>) {
    if node.kind() == "import_spec" {
        let Some(path_node) = node.child_by_field_name("path") else {
            return;
        };
        let path = node_text(path_node, src).trim_matches(['"', '`']);
        if path == "entgo.io/ent" || path.ends_with("/entgo.io/ent") {
            let alias = node
                .child_by_field_name("name")
                .map(|n| node_text(n, src).trim())
                .filter(|a| !a.is_empty() && *a != "_");
            *ent_alias = Some(alias.unwrap_or("ent").to_owned());
        }

        return;
    }

    let mut cur = node.walk();
    for ch in node.named_children(&mut cur) {
        collect_ent_import_alias(ch, src, ent_alias);
    }
}

fn embeds_ent_schema(struct_node: Node<'_>, src: &str, ent_alias: Option<&str>) -> bool {
    let mut cursor = struct_node.walk();
    for child in struct_node.named_children(&mut cursor) {
        if child.kind() != "field_declaration_list" {
            continue;
        }

        let mut fc = child.walk();
        for fnode in child.named_children(&mut fc) {
            if fnode.kind() != "field_declaration" || fnode.child_by_field_name("name").is_some() {
                continue;
            }

            let Some(ty) = fnode.child_by_field_name("type") else {
                continue;
            };
            let trimmed = node_text(ty, src).trim().trim_start_matches('*').trim();
            if trimmed == "ent.Schema" {
                return true;
            }

            match ent_alias {
                Some(".") if trimmed == "Schema" => return true,
                Some(alias)
                    if trimmed
                        .strip_prefix(alias)
                        .and_then(|rest| rest.strip_prefix('.'))
                        == Some("Schema") =>
                {
                    return true;
                }
                _ => {}
            }
        }
    }
    false
}

fn collect_ent_methods(file: &str, src: &str, by_key: &mut BTreeMap<SchemaKey, EntSchema>) {
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
    let dir = dir_of(file);
    let mut mc = root.walk();

    for child in root.named_children(&mut mc) {
        if child.kind() != "method_declaration" {
            continue;
        }

        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(recv_node) = child.child_by_field_name("receiver") else {
            continue;
        };
        let Some(body_node) = child.child_by_field_name("body") else {
            continue;
        };
        let recv_type = node_text(recv_node, src)
            .trim_matches(['(', ')'])
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_start_matches('*');
        let Some(schema) = by_key.get_mut(&(dir.clone(), recv_type.to_owned())) else {
            continue;
        };

        let body_text = node_text(body_node, src);
        match node_text(name_node, src) {
            "Fields" => collect_ent_fields(
                file,
                body_node,
                src,
                &mut schema.fields,
                &mut schema.id_storage_key,
            ),
            "Indexes" => collect_ent_indexes(body_node, src, &mut schema.uniques),
            "Edges" => collect_ent_edges(body_node, src, &mut schema.edges),
            "Annotations" => {
                if let Some(tbl) = extract_ent_table_annotation(body_text) {
                    schema.table_override = Some(tbl);
                }
            }
            _ => {}
        }
    }
}

/// Collects outermost `literal_element` AST nodes in source order without
/// descending into nested composite literals (e.g. `entsql.Annotation{...}`).
fn collect_top_literal_elements<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    if node.kind() == "literal_element" {
        out.push(node);
        return;
    }

    let mut cur = node.walk();
    for ch in node.named_children(&mut cur) {
        collect_top_literal_elements(ch, out);
    }
}

fn collect_ent_fields(
    file: &str,
    node: Node<'_>,
    src: &str,
    out: &mut Vec<DbModelColumn>,
    id_storage_key: &mut Option<String>,
) {
    let base_line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
    let mut elements = Vec::new();
    collect_top_literal_elements(node, &mut elements);
    for elem_node in elements {
        let elem = collapse_dot_whitespace(node_text(elem_node, src));
        if elem.contains("field.")
            && let Some((col, is_id)) = parse_ent_field_chain(
                file,
                u32::try_from(elem_node.start_position().row + 1).unwrap_or(base_line),
                &elem,
            )
        {
            if is_id {
                *id_storage_key = Some(col.name.clone());
            }

            out.push(col);
        }
    }
}

fn parse_ent_field_chain(file: &str, line: u32, expr: &str) -> Option<(DbModelColumn, bool)> {
    let fpos = expr.find("field.")?;
    let rest = &expr[fpos + "field.".len()..];
    let open = rest.find('(')?;
    let kind = &rest[..open];
    let name = extract_first_quoted(&rest[open + 1..])?;
    let is_id = name == "id";
    let col_name = extract_call_string(expr, ".StorageKey(").unwrap_or(name);
    let optional = expr.contains(".Optional()") || expr.contains(".Nillable()");
    let unique = expr.contains(".Unique()");
    let max_length = extract_call_u32(expr, ".MaxLen(");
    let enum_values = if kind == "Enum" {
        extract_call_strings(expr, ".Values(")
    } else {
        Vec::new()
    };

    Some((
        DbModelColumn {
            name: col_name,
            required: !optional,
            unique,
            max_length,
            enum_values,
            source: Some(DbSourcePosition {
                file: file.to_owned(),
                line,
            }),
        },
        is_id,
    ))
}

fn collect_ent_indexes(node: Node<'_>, src: &str, out: &mut Vec<DbModelUnique>) {
    let mut elements = Vec::new();
    collect_top_literal_elements(node, &mut elements);
    for elem_node in elements {
        let elem = collapse_dot_whitespace(node_text(elem_node, src));
        let Some(pos) = elem.find("index.Fields(") else {
            continue;
        };
        let rest = &elem[pos + "index.Fields(".len()..];
        let Some(end) = rest.find(')') else {
            continue;
        };
        let cols = extract_all_quoted(&rest[..end]);
        let after = &rest[end + 1..];
        if after.contains(".Unique()") && !cols.is_empty() {
            out.push(DbModelUnique { columns: cols });
        }
    }
}

fn collect_ent_edges(node: Node<'_>, src: &str, out: &mut Vec<(String, String)>) {
    let mut elements = Vec::new();
    collect_top_literal_elements(node, &mut elements);
    for elem_node in elements {
        let elem = collapse_dot_whitespace(node_text(elem_node, src));
        if !elem.contains("edge.From(") && !elem.contains("edge.To(") {
            continue;
        }

        if let Some(fk_col) = extract_call_string(&elem, ".Field(")
            && let Some(type_pos) = elem.find(".Type")
        {
            let before = &elem[..type_pos];
            let target = before
                .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            if !target.is_empty() {
                out.push((fk_col, target.to_owned()));
            }
        }
    }
}

/// Collapses whitespace immediately after `.` outside string literals so Go
/// multiline method chains (`index.Fields("a").\n\tUnique()`) match `.Unique()`
/// and `.Field(...)` cleanly.
fn collapse_dot_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        out.push(ch);
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                in_quote = None;
            }

            continue;
        }

        if ch == '"' || ch == '`' {
            in_quote = Some(ch);
            continue;
        }

        if ch == '.' {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
        }
    }
    out
}

fn extract_ent_table_annotation(body: &str) -> Option<String> {
    let pos = body.find("Table:")?;
    extract_first_quoted(&body[pos + "Table:".len()..])
}

fn extract_first_quoted(s: &str) -> Option<String> {
    let q1 = s.find('"')? + 1;
    let q2 = s[q1..].find('"')?;
    Some(s[q1..q1 + q2].to_owned())
}

fn extract_all_quoted(s: &str) -> Vec<String> {
    let mut vals = Vec::new();
    let mut rest = s;
    while let Some(q1) = rest.find('"') {
        let after = &rest[q1 + 1..];
        let Some(q2) = after.find('"') else {
            break;
        };
        vals.push(after[..q2].to_owned());
        rest = &after[q2 + 1..];
    }
    vals
}

fn extract_call_string(expr: &str, marker: &str) -> Option<String> {
    let pos = expr.find(marker)?;
    extract_first_quoted(&expr[pos + marker.len()..])
}

fn extract_call_strings(expr: &str, marker: &str) -> Vec<String> {
    let Some(pos) = expr.find(marker) else {
        return Vec::new();
    };
    let rest = &expr[pos + marker.len()..];
    let Some(end) = rest.find(')') else {
        return Vec::new();
    };
    extract_all_quoted(&rest[..end])
}

fn extract_call_u32(expr: &str, marker: &str) -> Option<u32> {
    let pos = expr.find(marker)?;
    let rest = &expr[pos + marker.len()..];
    let end = rest.find(')')?;
    rest[..end].trim().parse::<u32>().ok()
}
