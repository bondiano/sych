//! Go AST and struct-tag parsing helpers for the GORM static extractor.

use std::collections::BTreeMap;

use tree_sitter::{Node, Parser};

use super::super::dir_of;

pub(super) const GO_SCALARS: &[&str] = &[
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

pub(super) struct ParsedStruct {
    pub file: String,
    pub dir: String,
    pub line: u32,
    pub pkg: String,
    pub name: String,
    pub fields: Vec<ParsedField>,
    pub is_gorm: bool,
}

pub(super) struct ParsedField {
    pub line: u32,
    pub name: Option<String>,
    pub type_text: String,
    pub tag_raw: String,
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub(super) struct GormTag {
    pub skip: bool,
    pub embedded: bool,
    pub embedded_prefix: Option<String>,
    pub column: Option<String>,
    pub primary_key: bool,
    pub not_null: bool,
    pub unique: bool,
    pub unique_index_name: Option<String>,
    pub size: Option<u32>,
    pub has_type: bool,
    pub has_serializer: bool,
    pub foreign_key: Option<String>,
    pub references: Option<String>,
    pub many2many: Option<String>,
}

#[derive(Default)]
pub(super) struct ValidateTag {
    pub required: bool,
    pub max_length: Option<u32>,
    pub oneof: Vec<String>,
}

pub(super) fn parse_go_file(
    file: &str,
    src: &str,
    structs: &mut Vec<ParsedStruct>,
    table_methods: &mut BTreeMap<(String, String), Option<String>>,
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
    let dir = dir_of(file);
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

                    let parsed = parse_go_struct(
                        file,
                        &dir,
                        &pkg,
                        node_text(name_node, src),
                        type_node,
                        src,
                    );
                    structs.push(parsed);
                }
            }
            "method_declaration" => {
                if let Some((recv, tbl)) = parse_table_name_method(child, src) {
                    table_methods.insert((dir.clone(), recv), tbl);
                }
            }
            _ => {}
        }
    }
}

fn parse_go_struct(
    file: &str,
    dir: &str,
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
        dir: dir.to_owned(),
        line,
        pkg: pkg.to_owned(),
        name: name.to_owned(),
        fields,
        is_gorm,
    }
}

fn parse_table_name_method(node: Node<'_>, src: &str) -> Option<(String, Option<String>)> {
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
    let body = node.child_by_field_name("body")?;
    let ret = single_return_string_literal(body, src);
    Some((recv_type, ret))
}

/// Resolves a `TableName()` body only when it has a single `return` statement
/// returning a direct string literal. Multi-return or computed bodies return
/// `None` so the caller falls back to default naming or config overrides.
fn single_return_string_literal(body: Node<'_>, src: &str) -> Option<String> {
    let mut returns = Vec::new();
    collect_return_statements(body, &mut returns);
    if returns.len() != 1 {
        return None;
    }

    direct_return_string_literal(returns[0], src)
}

fn collect_return_statements<'a>(node: Node<'a>, out: &mut Vec<Node<'a>>) {
    if node.kind() == "func_literal" {
        return;
    }

    if node.kind() == "return_statement" {
        out.push(node);
        return;
    }

    let mut cur = node.walk();
    for child in node.named_children(&mut cur) {
        collect_return_statements(child, out);
    }
}

fn direct_return_string_literal(return_stmt: Node<'_>, src: &str) -> Option<String> {
    let expr_list = return_stmt.named_child(0)?;
    if expr_list.kind() != "expression_list" || expr_list.named_child_count() != 1 {
        return None;
    }

    let mut expr = expr_list.named_child(0)?;
    while expr.kind() == "parenthesized_expression" {
        expr = expr.named_child(0)?;
    }
    if matches!(
        expr.kind(),
        "interpreted_string_literal" | "raw_string_literal"
    ) {
        let raw = node_text(expr, src);
        Some(raw.trim_matches(['"', '`']).to_owned())
    } else {
        None
    }
}

pub(super) fn parse_gorm_tag(tag: &str) -> GormTag {
    let mut out = GormTag::default();
    for raw in tag.split(';') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }

        let lower_part = part.to_ascii_lowercase();
        if part == "-" || lower_part == "-:all" || lower_part == "-:migration" {
            out.skip = true;
            return out;
        }

        let (k, v) = part
            .split_once(':')
            .map_or((part, None), |(k, v)| (k, Some(v)));
        match k.trim().to_ascii_lowercase().as_str() {
            "embedded" => out.embedded = true,
            "embeddedprefix" => {
                out.embedded = true;
                out.embedded_prefix = v.map(|s| s.trim().to_owned());
            }
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
            "serializer" => out.has_serializer = v.is_some_and(|s| !s.trim().is_empty()),
            "type" => {
                out.has_type = v.is_some_and(|s| !s.trim().is_empty());
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

pub(super) fn parse_validate_tag(tag: &str) -> ValidateTag {
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

pub(crate) fn extract_struct_tag(raw: &str, key: &str) -> Option<String> {
    let pat = format!("{key}:\"");
    let mut offset = 0;
    while let Some(rel) = raw[offset..].find(&pat) {
        let abs = offset + rel;
        let boundary = abs == 0
            || raw[..abs]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let value_start = abs + pat.len();
        if boundary {
            let rest = &raw[value_start..];
            let end = rest.find('"')?;
            return Some(rest[..end].to_owned());
        }

        offset = value_start;
    }
    None
}

/// Strips outer pointer (`*`) and slice (`[]`) wrappers, preserving an optional
/// package qualifier (`pkg.Type`).
pub(super) fn strip_go_wrappers(ty: &str) -> &str {
    let mut s = ty.trim();
    loop {
        if let Some(rest) = s.strip_prefix('*') {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("[]") {
            s = rest.trim_start();
        } else {
            return s;
        }
    }
}

pub(crate) fn split_go_type(ty: &str) -> (Option<&str>, &str) {
    let stripped = strip_go_wrappers(ty);
    match stripped.rsplit_once('.') {
        Some((pkg, name)) => (Some(pkg), name),
        None => (None, stripped),
    }
}

pub(super) fn is_slice_go_type(ty: &str) -> bool {
    let mut s = ty.trim();
    while let Some(rest) = s.strip_prefix('*') {
        s = rest.trim_start();
    }
    s.starts_with("[]")
}

pub(super) fn is_scalar_go_type(ty: &str) -> bool {
    let (_, base) = split_go_type(ty);
    GO_SCALARS.contains(&base)
}

pub(crate) fn node_text<'a>(node: Node<'_>, src: &'a str) -> &'a str {
    src.get(node.byte_range()).unwrap_or_default()
}
