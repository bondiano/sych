//! Static Python `SQLAlchemy` (1.x & 2.x) model extractor using `tree-sitter-python`.

use std::collections::BTreeMap;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::go_gorm::node_text;
use super::source::SourceTree;

/// Extracts `SQLAlchemy` model tables from Python files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["py"], &settings.paths);
    let mut py_enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut parsed_files: Vec<(String, String)> = Vec::new();

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("__tablename__")
            && !src.contains("sqlalchemy")
            && !src.contains("mapped_column")
            && !src.contains("Column(")
            && !src.contains("Enum)")
        {
            continue;
        }

        collect_py_enums(&src, &mut py_enums);
        parsed_files.push((file.clone(), src));
    }

    let mut out = Vec::new();
    for (file, src) in parsed_files {
        if !src.contains("__tablename__") {
            continue;
        }

        extract_file(&file, &src, &py_enums, settings, &mut out);
    }
    out
}

fn collect_py_enums(src: &str, enums: &mut BTreeMap<String, Vec<String>>) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return;
    }

    let Some(tree) = parser.parse(src, None) else {
        return;
    };
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "class_definition" {
            continue;
        }

        let Some(superclasses) = child.child_by_field_name("superclasses") else {
            continue;
        };
        if !node_text(superclasses, src).contains("Enum") {
            continue;
        }

        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(body) = child.child_by_field_name("body") else {
            continue;
        };
        let mut vals = Vec::new();
        let mut bc = body.walk();
        for stmt in body.named_children(&mut bc) {
            let line = node_text(stmt, src);
            if let Some((_, rhs)) = line.split_once('=')
                && let Some(val) = unquote_py_str(rhs.trim())
            {
                vals.push(val);
            }
        }
        if !vals.is_empty() {
            enums.insert(node_text(name_node, src).to_owned(), vals);
        }
    }
}

fn extract_file(
    file: &str,
    src: &str,
    py_enums: &BTreeMap<String, Vec<String>>,
    settings: &ModelExtractionSettings,
    out: &mut Vec<DbModelTable>,
) {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return;
    }

    let Some(tree) = parser.parse(src, None) else {
        return;
    };
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() != "class_definition" {
            continue;
        }

        if let Some(tbl) = parse_sa_class(file, child, src, py_enums, settings) {
            out.push(tbl);
        }
    }
}

fn parse_sa_class(
    file: &str,
    node: Node<'_>,
    src: &str,
    py_enums: &BTreeMap<String, Vec<String>>,
    settings: &ModelExtractionSettings,
) -> Option<DbModelTable> {
    let class_name = node_text(node.child_by_field_name("name")?, src);
    if settings.exclude.iter().any(|e| e == class_name) {
        return None;
    }

    let body = node.child_by_field_name("body")?;
    let class_line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);

    let mut table_name: Option<String> = settings.tables.get(class_name).cloned();
    let mut schema: Option<String> = None;
    let mut columns = Vec::new();
    let mut primary_key = Vec::new();
    let mut uniques = Vec::new();
    let mut relations = Vec::new();

    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        let stmt_text = node_text(stmt, src).trim();
        let line = u32::try_from(stmt.start_position().row + 1).unwrap_or(class_line);
        if let Some(rest) = stmt_text.strip_prefix("__tablename__")
            && let Some((_, rhs)) = rest.split_once('=')
            && table_name.is_none()
        {
            table_name = unquote_py_str(rhs.trim());
            continue;
        }

        if stmt_text.starts_with("__table_args__") {
            parse_table_args(stmt_text, &mut schema, &mut uniques, &mut relations);
            continue;
        }

        if stmt_text.contains("mapped_column(") || stmt_text.contains("Column(") {
            parse_sa_column(
                file,
                line,
                stmt_text,
                py_enums,
                &mut columns,
                &mut primary_key,
                &mut relations,
            );
        }
    }

    let name = table_name?;
    let mut table = DbModelTable {
        schema,
        name: String::new(),
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line: class_line,
        }),
        columns,
        primary_key,
        join_table_for: Vec::new(),
        uniques,
        relations,
    };
    table.set_qualified_name(&name);
    Some(table)
}

fn parse_sa_column(
    file: &str,
    line: u32,
    stmt: &str,
    py_enums: &BTreeMap<String, Vec<String>>,
    columns: &mut Vec<DbModelColumn>,
    primary_key: &mut Vec<String>,
    relations: &mut Vec<DbModelRelation>,
) {
    let Some((lhs, rhs)) = stmt.split_once('=') else {
        return;
    };
    let (attr_name, annotation) = match lhs.split_once(':') {
        Some((n, ann)) => (n.trim(), Some(ann.trim())),
        None => (lhs.trim(), None),
    };
    if attr_name.is_empty() || attr_name.starts_with('_') {
        return;
    }

    let call_start = rhs
        .find("mapped_column(")
        .map(|p| p + "mapped_column(".len())
        .or_else(|| rhs.find("Column(").map(|p| p + "Column(".len()));
    let Some(start) = call_start else {
        return;
    };
    let args = &rhs[start..];

    // Optional first string argument overrides column name: `Column("db_col", String(50), ...)`
    let first_arg = args.split(',').next().unwrap_or("").trim();
    let col_name = unquote_py_str(first_arg).unwrap_or_else(|| attr_name.to_owned());

    let pk = has_kwarg_bool(args, "primary_key", true);
    let unique = has_kwarg_bool(args, "unique", true);
    let explicit_nullable = if has_kwarg_bool(args, "nullable", false) {
        Some(false)
    } else if has_kwarg_bool(args, "nullable", true) {
        Some(true)
    } else {
        None
    };

    let ann_required = annotation.map(|ann| {
        !ann.contains("Optional[") && !ann.contains("| None") && !ann.contains("None |")
    });
    let required = pk
        || match explicit_nullable {
            Some(nullable) => !nullable,
            None => ann_required.unwrap_or(false),
        };

    let max_length = extract_sa_string_len(args);
    let enum_values = extract_sa_enum(args, py_enums);

    if pk {
        primary_key.push(col_name.clone());
    }

    if let Some(fk_pos) = args.find("ForeignKey(")
        && let Some(target) = extract_first_py_str(&args[fk_pos + "ForeignKey(".len()..])
        && let Some((ref_tbl, ref_col)) = split_fk_target(&target)
    {
        let mut tref = DbModelTableRef {
            schema: None,
            table: ref_tbl,
        };
        if let Some((s, t)) = tref.table.clone().split_once('.') {
            tref.schema = Some(s.to_owned());
            t.clone_into(&mut tref.table);
        }

        relations.push(DbModelRelation {
            columns: vec![col_name.clone()],
            references: tref,
            ref_columns: vec![ref_col],
        });
    }

    columns.push(DbModelColumn {
        name: col_name,
        required,
        unique,
        max_length,
        enum_values,
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line,
        }),
    });
}

fn parse_table_args(
    stmt: &str,
    schema: &mut Option<String>,
    uniques: &mut Vec<DbModelUnique>,
    relations: &mut Vec<DbModelRelation>,
) {
    if let Some(pos) = stmt.find("\"schema\"").or_else(|| stmt.find("'schema'"))
        && let Some((_, after)) = stmt[pos..].split_once(':')
        && let Some(s) = extract_first_py_str(after)
    {
        *schema = Some(s);
    }

    let mut rest = stmt;
    while let Some(pos) = rest.find("UniqueConstraint(") {
        let tail = &rest[pos + "UniqueConstraint(".len()..];
        let Some(end) = tail.find(')') else {
            break;
        };
        let cols: Vec<String> = tail[..end]
            .split(',')
            .filter_map(|p| {
                let p = p.trim();
                if p.contains('=') {
                    None
                } else {
                    unquote_py_str(p)
                }
            })
            .collect();
        if !cols.is_empty() {
            uniques.push(DbModelUnique { columns: cols });
        }

        rest = &tail[end + 1..];
    }

    let mut rest = stmt;
    while let Some(pos) = rest.find("ForeignKeyConstraint(") {
        let tail = &rest[pos + "ForeignKeyConstraint(".len()..];
        if let Some(l_open) = tail.find('[')
            && let Some(l_close) = tail[l_open + 1..].find(']')
            && let Some(r_open) = tail[l_open + l_close + 2..].find('[')
        {
            let r_start = l_open + l_close + 2 + r_open + 1;
            if let Some(r_close) = tail[r_start..].find(']') {
                let locals = extract_py_str_list(&tail[l_open + 1..l_open + 1 + l_close]);
                let refs = extract_py_str_list(&tail[r_start..r_start + r_close]);
                if !locals.is_empty() && locals.len() == refs.len() {
                    let parsed_refs: Vec<(String, String)> =
                        refs.iter().filter_map(|r| split_fk_target(r)).collect();
                    if parsed_refs.len() == refs.len() {
                        let ref_tbl = parsed_refs[0].0.clone();
                        let ref_cols = parsed_refs.into_iter().map(|(_, c)| c).collect();
                        let mut tref = DbModelTableRef {
                            schema: None,
                            table: ref_tbl,
                        };
                        if let Some((s, t)) = tref.table.clone().split_once('.') {
                            tref.schema = Some(s.to_owned());
                            t.clone_into(&mut tref.table);
                        }

                        relations.push(DbModelRelation {
                            columns: locals,
                            references: tref,
                            ref_columns: ref_cols,
                        });
                    }
                }
            }
        }

        rest = tail;
    }
}

fn split_fk_target(target: &str) -> Option<(String, String)> {
    let (table_part, col) = target.rsplit_once('.')?;
    if table_part.is_empty() || col.is_empty() {
        return None;
    }

    Some((table_part.to_owned(), col.to_owned()))
}

fn extract_sa_string_len(args: &str) -> Option<u32> {
    for pat in ["String(", "VARCHAR(", "Unicode(", "CHAR("] {
        if let Some(pos) = args.find(pat) {
            let rest = &args[pos + pat.len()..];
            if let Some(end) = rest.find([')', ','])
                && let Ok(n) = rest[..end].trim().parse::<u32>()
                && n > 0
            {
                return Some(n);
            }
        }
    }
    None
}

fn extract_sa_enum(args: &str, py_enums: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let Some(pos) = args.find("Enum(") else {
        return Vec::new();
    };
    let rest = &args[pos + "Enum(".len()..];
    let Some(end) = rest.find(')') else {
        return Vec::new();
    };
    let inner = rest[..end].trim();
    let first = inner.split(',').next().unwrap_or("").trim();
    if let Some(vals) = py_enums.get(first) {
        return vals.clone();
    }

    inner
        .split(',')
        .filter_map(|p| {
            let t = p.trim();
            if t.contains('=') {
                None
            } else {
                unquote_py_str(t)
            }
        })
        .collect()
}

pub(super) fn has_kwarg_bool(args: &str, key: &str, expected: bool) -> bool {
    let val = if expected { "True" } else { "False" };
    let needle = format!("{key}=");
    let spaced = format!("{key} =");
    for pat in [&needle, &spaced] {
        if let Some(pos) = args.find(pat) {
            let rest = args[pos + pat.len()..].trim_start();
            if rest.starts_with(val) {
                return true;
            }
        }
    }
    false
}

pub(super) fn unquote_py_str(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return Some(trimmed[1..trimmed.len() - 1].to_owned());
    }

    None
}

pub(super) fn extract_first_py_str(s: &str) -> Option<String> {
    for (idx, c) in s.char_indices() {
        if c == '"' || c == '\'' {
            let rest = &s[idx + 1..];
            if let Some(end) = rest.find(c) {
                return Some(rest[..end].to_owned());
            }

            return None;
        }
    }
    None
}

pub(super) fn extract_py_str_list(s: &str) -> Vec<String> {
    s.split(',').filter_map(unquote_py_str).collect()
}
