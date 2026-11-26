//! Static Python Django ORM model extractor using `tree-sitter-python`.

use std::collections::BTreeMap;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};
use tree_sitter::{Node, Parser};

use super::go_gorm::node_text;
use super::py_sqlalchemy::{
    extract_first_py_str, extract_py_str_list, has_kwarg_bool, unquote_py_str,
};
use super::source::SourceTree;

struct DjangoModel {
    file: String,
    line: u32,
    app_label: String,
    class_name: String,
    db_table: String,
    columns: Vec<DbModelColumn>,
    primary_key: Vec<String>,
    uniques: Vec<DbModelUnique>,
    raw_relations: Vec<(String, String)>, // (fk_col, target_model_name)
    m2m: Vec<(String, String)>,           // (field_name, target_model_name)
}

/// Extracts Django ORM models from Python files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["py"], &settings.paths);
    let mut choices_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut raw_models = Vec::new();

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("models.Model") && !src.contains("django.db") {
            continue;
        }

        parse_django_file(file, &src, settings, &mut choices_map, &mut raw_models);
    }

    let table_by_class: BTreeMap<String, String> = raw_models
        .iter()
        .map(|m| (m.class_name.clone(), m.db_table.clone()))
        .collect();

    let mut out = Vec::new();
    for m in raw_models {
        if settings.exclude.iter().any(|e| e == &m.class_name) {
            continue;
        }

        let mut relations = Vec::new();
        for (fk_col, target) in m.raw_relations {
            let ref_table = resolve_django_target(&m.app_label, &target, &table_by_class);
            relations.push(DbModelRelation {
                columns: vec![fk_col],
                references: DbModelTableRef {
                    schema: None,
                    table: ref_table,
                },
                ref_columns: vec!["id".into()],
            });
        }

        let mut table = DbModelTable {
            schema: None,
            name: String::new(),
            source: Some(DbSourcePosition {
                file: m.file.clone(),
                line: m.line,
            }),
            columns: m.columns,
            primary_key: if m.primary_key.is_empty() {
                vec!["id".into()]
            } else {
                m.primary_key
            },
            join_table_for: Vec::new(),
            uniques: m.uniques,
            relations,
        };
        table.set_qualified_name(&m.db_table);
        let this_ref = table.table_ref();
        out.push(table);

        for (field_name, target) in m.m2m {
            let ref_table = resolve_django_target(&m.app_label, &target, &table_by_class);
            out.push(DbModelTable {
                schema: None,
                name: format!("{}_{field_name}", m.db_table),
                source: Some(DbSourcePosition {
                    file: m.file.clone(),
                    line: m.line,
                }),
                columns: Vec::new(),
                primary_key: Vec::new(),
                join_table_for: vec![
                    this_ref.clone(),
                    DbModelTableRef {
                        schema: None,
                        table: ref_table,
                    },
                ],
                uniques: Vec::new(),
                relations: Vec::new(),
            });
        }
    }
    out
}

fn resolve_django_target(
    app_label: &str,
    raw_target: &str,
    table_by_class: &BTreeMap<String, String>,
) -> String {
    let clean = raw_target.trim().trim_matches(['"', '\'']);
    if let Some(tbl) = table_by_class.get(clean) {
        return tbl.clone();
    }

    if let Some((app, model)) = clean.split_once('.') {
        if let Some(tbl) = table_by_class.get(model) {
            return tbl.clone();
        }

        return format!(
            "{}_{}",
            app.to_ascii_lowercase(),
            model.to_ascii_lowercase()
        );
    }

    format!("{app_label}_{}", clean.to_ascii_lowercase())
}

fn parse_django_file(
    file: &str,
    src: &str,
    settings: &ModelExtractionSettings,
    choices_map: &mut BTreeMap<String, Vec<String>>,
    out: &mut Vec<DjangoModel>,
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
    let app_label = infer_django_app_label(file);
    let root = tree.root_node();
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        if child.kind() != "class_definition" {
            continue;
        }

        let Some(superclasses) = child.child_by_field_name("superclasses") else {
            continue;
        };
        let bases = node_text(superclasses, src);
        if bases.contains("TextChoices") || bases.contains("IntegerChoices") {
            if let Some(name) = child.child_by_field_name("name")
                && let Some(body) = child.child_by_field_name("body")
            {
                let vals = parse_django_choices_class(body, src);
                if !vals.is_empty() {
                    choices_map.insert(node_text(name, src).to_owned(), vals);
                }
            }

            continue;
        }

        if !bases.contains("models.Model") && !bases.contains("(Model)") {
            continue;
        }

        if let Some(model) = parse_django_model(file, &app_label, child, src, settings, choices_map)
        {
            out.push(model);
        }
    }
}

fn parse_django_choices_class(body: Node<'_>, src: &str) -> Vec<String> {
    let mut vals = Vec::new();
    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        let text = node_text(stmt, src);
        if let Some((_, rhs)) = text.split_once('=')
            && let Some(val) = extract_first_py_str(rhs)
        {
            vals.push(val);
        }
    }
    vals
}

fn parse_django_model(
    file: &str,
    app_label: &str,
    node: Node<'_>,
    src: &str,
    settings: &ModelExtractionSettings,
    choices_map: &BTreeMap<String, Vec<String>>,
) -> Option<DjangoModel> {
    let class_name = node_text(node.child_by_field_name("name")?, src).to_owned();
    let body = node.child_by_field_name("body")?;
    let class_line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);

    let mut db_table = settings.tables.get(&class_name).cloned();
    let mut is_abstract = false;
    let mut columns = Vec::new();
    let mut primary_key = Vec::new();
    let mut uniques = Vec::new();
    let mut raw_relations = Vec::new();
    let mut m2m = Vec::new();

    let mut cursor = body.walk();
    for stmt in body.named_children(&mut cursor) {
        if stmt.kind() == "class_definition" {
            let inner_name = stmt
                .child_by_field_name("name")
                .map_or("", |n| node_text(n, src));
            if inner_name == "Meta"
                && let Some(meta_body) = stmt.child_by_field_name("body")
            {
                let meta_text = node_text(meta_body, src);
                if meta_text.contains("abstract = True") || meta_text.contains("abstract=True") {
                    is_abstract = true;
                }

                if db_table.is_none()
                    && let Some(pos) = meta_text.find("db_table")
                    && let Some((_, rhs)) = meta_text[pos..].split_once('=')
                {
                    db_table = extract_first_py_str(rhs);
                }

                parse_django_meta_uniques(meta_text, &mut uniques);
            }

            continue;
        }

        let text = node_text(stmt, src).trim();
        let line = u32::try_from(stmt.start_position().row + 1).unwrap_or(class_line);
        let Some((lhs, rhs)) = text.split_once('=') else {
            continue;
        };
        let field_name = lhs.trim();
        if field_name.is_empty() || field_name.starts_with('_') {
            continue;
        }

        let rhs = rhs.trim();
        if !rhs.starts_with("models.") && !rhs.contains("Field(") {
            continue;
        }

        parse_django_field(
            file,
            line,
            field_name,
            rhs,
            choices_map,
            &mut columns,
            &mut primary_key,
            &mut raw_relations,
            &mut m2m,
        );
    }

    if is_abstract {
        return None;
    }

    let db_table =
        db_table.unwrap_or_else(|| format!("{app_label}_{}", class_name.to_ascii_lowercase()));
    Some(DjangoModel {
        file: file.to_owned(),
        line: class_line,
        app_label: app_label.to_owned(),
        class_name,
        db_table,
        columns,
        primary_key,
        uniques,
        raw_relations,
        m2m,
    })
}

#[allow(clippy::too_many_arguments)]
fn parse_django_field(
    file: &str,
    line: u32,
    field_name: &str,
    rhs: &str,
    choices_map: &BTreeMap<String, Vec<String>>,
    columns: &mut Vec<DbModelColumn>,
    primary_key: &mut Vec<String>,
    raw_relations: &mut Vec<(String, String)>,
    m2m: &mut Vec<(String, String)>,
) {
    let Some(open) = rhs.find('(') else {
        return;
    };
    let field_type = rhs[..open].rsplit('.').next().unwrap_or("");
    let args = &rhs[open + 1..];

    if field_type == "ManyToManyField" {
        let target = args.split(',').next().unwrap_or("").trim();
        if !target.is_empty() {
            m2m.push((field_name.to_owned(), target.to_owned()));
        }

        return;
    }

    let is_fk = matches!(field_type, "ForeignKey" | "OneToOneField");
    let db_column = extract_kwarg_str(args, "db_column");
    let col_name = db_column.unwrap_or_else(|| {
        if is_fk {
            format!("{field_name}_id")
        } else {
            field_name.to_owned()
        }
    });

    if is_fk {
        let target = args.split(',').next().unwrap_or("").trim();
        if !target.is_empty() {
            raw_relations.push((col_name.clone(), target.to_owned()));
        }
    }

    let null_true = has_kwarg_bool(args, "null", true);
    let pk = has_kwarg_bool(args, "primary_key", true);
    let unique = has_kwarg_bool(args, "unique", true) || field_type == "OneToOneField";
    let max_length = extract_kwarg_u32(args, "max_length");
    let enum_values = extract_django_choices(args, choices_map);

    if pk {
        primary_key.push(col_name.clone());
    }

    columns.push(DbModelColumn {
        name: col_name,
        required: !null_true,
        unique,
        max_length,
        enum_values,
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line,
        }),
    });
}

fn parse_django_meta_uniques(meta: &str, uniques: &mut Vec<DbModelUnique>) {
    let mut rest = meta;
    while let Some(pos) = rest.find("UniqueConstraint(") {
        let tail = &rest[pos + "UniqueConstraint(".len()..];
        if let Some(fpos) = tail.find("fields=")
            && let Some(open) = tail[fpos..].find(['[', '('])
        {
            let start = fpos + open + 1;
            if let Some(close) = tail[start..].find([']', ')']) {
                let cols = extract_py_str_list(&tail[start..start + close]);
                if !cols.is_empty() {
                    uniques.push(DbModelUnique { columns: cols });
                }
            }
        }

        rest = tail;
    }

    if let Some(pos) = meta.find("unique_together")
        && let Some((_, rhs)) = meta[pos..].split_once('=')
        && let Some(line_end) = rhs.find('\n')
    {
        let slice = &rhs[..line_end];
        let cols: Vec<String> = slice
            .split([',', '(', ')', '[', ']'])
            .filter_map(unquote_py_str)
            .collect();
        if !cols.is_empty() {
            uniques.push(DbModelUnique { columns: cols });
        }
    }
}

fn extract_django_choices(args: &str, choices_map: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    let Some(pos) = args.find("choices=") else {
        return Vec::new();
    };
    let rest = args[pos + "choices=".len()..].trim_start();
    if let Some(ident) = rest
        .split([',', ')'])
        .next()
        .map(|s| s.trim().trim_end_matches(".choices"))
        && let Some(vals) = choices_map.get(ident)
    {
        return vals.clone();
    }

    Vec::new()
}

fn extract_kwarg_str(args: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let pos = args.find(&needle)?;
    extract_first_py_str(&args[pos + needle.len()..])
}

fn extract_kwarg_u32(args: &str, key: &str) -> Option<u32> {
    let needle = format!("{key}=");
    let pos = args.find(&needle)?;
    let rest = args[pos + needle.len()..].trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse::<u32>().ok().filter(|&n| n > 0)
}

fn infer_django_app_label(file: &str) -> String {
    let parts: Vec<&str> = file.split('/').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_ascii_lowercase()
    } else {
        "app".into()
    }
}
