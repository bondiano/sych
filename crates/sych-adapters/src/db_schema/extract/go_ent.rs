//! Static Go Ent schema extractor (`ent/schema/*.go`) using `tree-sitter-go`.

use std::collections::BTreeMap;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::go_gorm::node_text;
use super::naming::default_table_name;
use super::source::SourceTree;

struct EntSchema {
    file: String,
    line: u32,
    name: String,
    table_override: Option<String>,
    fields: Vec<DbModelColumn>,
    uniques: Vec<DbModelUnique>,
    edges: Vec<(String, String)>, // (local_fk_col, target_schema_name)
}

/// Extracts Ent schema definitions from Go files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["go"], &settings.paths);
    let mut schemas = Vec::new();

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

        parse_ent_file(file, &src, &mut schemas);
    }

    let table_names: BTreeMap<String, String> = schemas
        .iter()
        .map(|s| {
            let tbl = s.table_override.clone().unwrap_or_else(|| {
                default_table_name(&s.name, None, &settings.naming, &settings.tables)
            });
            (s.name.clone(), tbl)
        })
        .collect();

    let mut out = Vec::new();
    for s in schemas {
        if settings.exclude.iter().any(|e| e == &s.name) {
            continue;
        }

        let mut table = DbModelTable {
            schema: None,
            name: String::new(),
            source: Some(DbSourcePosition {
                file: s.file,
                line: s.line,
            }),
            columns: s.fields,
            primary_key: vec!["id".into()],
            join_table_for: Vec::new(),
            uniques: s.uniques,
            relations: Vec::new(),
        };
        table.set_qualified_name(&table_names[&s.name]);

        for (local_col, target_schema) in s.edges {
            let target_tbl = table_names.get(&target_schema).cloned().unwrap_or_else(|| {
                default_table_name(&target_schema, None, &settings.naming, &settings.tables)
            });
            table.relations.push(DbModelRelation {
                columns: vec![local_col],
                references: DbModelTableRef {
                    schema: None,
                    table: target_tbl,
                },
                ref_columns: vec!["id".into()],
            });
        }
        out.push(table);
    }
    out
}

fn parse_ent_file(file: &str, src: &str, out: &mut Vec<EntSchema>) {
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
    let mut by_name: BTreeMap<String, EntSchema> = BTreeMap::new();
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        if child.kind() == "type_declaration" {
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
                    && node_text(type_node, src).contains("ent.Schema")
                {
                    let name = node_text(name_node, src).to_owned();
                    let line = u32::try_from(spec.start_position().row + 1).unwrap_or(u32::MAX);
                    by_name.insert(
                        name.clone(),
                        EntSchema {
                            file: file.to_owned(),
                            line,
                            name,
                            table_override: None,
                            fields: Vec::new(),
                            uniques: Vec::new(),
                            edges: Vec::new(),
                        },
                    );
                }
            }
        }
    }

    if by_name.is_empty() {
        return;
    }

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
        let Some(schema) = by_name.get_mut(recv_type) else {
            continue;
        };

        let body_text = node_text(body_node, src);
        match node_text(name_node, src) {
            "Fields" => collect_ent_fields(file, body_node, src, &mut schema.fields),
            "Indexes" => collect_ent_indexes(body_text, &mut schema.uniques),
            "Edges" => collect_ent_edges(body_text, &mut schema.edges),
            "Annotations" => {
                if let Some(tbl) = extract_ent_table_annotation(body_text) {
                    schema.table_override = Some(tbl);
                }
            }
            _ => {}
        }
    }

    out.extend(by_name.into_values());
}

fn collect_ent_fields(file: &str, node: Node<'_>, src: &str, out: &mut Vec<DbModelColumn>) {
    let text = node_text(node, src);
    let base_line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
    // Walk top-level literal elements of the returned `[]ent.Field{ ... }` slice.
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        if cur.kind() == "literal_element" {
            let elem = node_text(cur, src);
            if elem.contains("field.")
                && let Some(col) = parse_ent_field_chain(
                    file,
                    u32::try_from(cur.start_position().row + 1).unwrap_or(base_line),
                    elem,
                )
            {
                out.push(col);
            }

            continue;
        }

        let mut c = cur.walk();
        for ch in cur.named_children(&mut c) {
            stack.push(ch);
        }
    }
    let _ = text;
}

fn parse_ent_field_chain(file: &str, line: u32, expr: &str) -> Option<DbModelColumn> {
    let fpos = expr.find("field.")?;
    let rest = &expr[fpos + "field.".len()..];
    let open = rest.find('(')?;
    let kind = &rest[..open];
    let name = extract_first_quoted(&rest[open + 1..])?;
    let col_name = extract_call_string(expr, ".StorageKey(").unwrap_or(name);
    let optional = expr.contains(".Optional()") || expr.contains(".Nillable()");
    let unique = expr.contains(".Unique()");
    let max_length = extract_call_u32(expr, ".MaxLen(");
    let enum_values = if kind == "Enum" {
        extract_call_strings(expr, ".Values(")
    } else {
        Vec::new()
    };

    Some(DbModelColumn {
        name: col_name,
        required: !optional,
        unique,
        max_length,
        enum_values,
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line,
        }),
    })
}

fn collect_ent_indexes(body: &str, out: &mut Vec<DbModelUnique>) {
    let mut rest = body;
    while let Some(pos) = rest.find("index.Fields(") {
        rest = &rest[pos + "index.Fields(".len()..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let cols = extract_all_quoted(&rest[..end]);
        let after = &rest[end + 1..];
        let next_comma = after.find('\n').unwrap_or(after.len());
        if after[..next_comma].contains(".Unique()") && !cols.is_empty() {
            out.push(DbModelUnique { columns: cols });
        }
    }
}

fn collect_ent_edges(body: &str, out: &mut Vec<(String, String)>) {
    let mut rest = body;
    while let Some(pos) = rest.find("edge.From(").or_else(|| rest.find("edge.To(")) {
        let slice = &rest[pos..];
        let end = slice.find(",\n").unwrap_or(slice.len());
        let chunk = &slice[..end];
        if let Some(fk_col) = extract_call_string(chunk, ".Field(")
            && let Some(type_pos) = chunk.find(".Type")
        {
            let before = &chunk[..type_pos];
            let target = before
                .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .unwrap_or("");
            if !target.is_empty() {
                out.push((fk_col, target.to_owned()));
            }
        }

        rest = &slice[4..];
    }
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
