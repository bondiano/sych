//! Static TypeScript/JavaScript Drizzle ORM extractor using `oxc_parser`.

use std::collections::BTreeMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, ArrowFunctionExpression, CallExpression, Declaration,
    Expression, ObjectPropertyKind, PropertyKey, Statement, VariableDeclaration,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};
use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};

use super::source::SourceTree;

type DrizzleTableMap = BTreeMap<String, (Option<String>, String, BTreeMap<String, String>)>;

struct PendingTable {
    var_name: String,
    table: DbModelTable,
    col_prop_map: BTreeMap<String, String>,
    raw_refs: Vec<(String, String, String)>, // (local_col, target_var, target_prop)
}

/// Extracts Drizzle ORM tables from TypeScript/JavaScript files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["ts", "tsx", "mts", "js", "mjs"], &settings.paths);
    let mut enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut pending: Vec<PendingTable> = Vec::new();

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("pgTable")
            && !src.contains("mysqlTable")
            && !src.contains("sqliteTable")
            && !src.contains("pgEnum")
            && !src.contains("mysqlEnum")
        {
            continue;
        }

        parse_drizzle_file(file, &src, &mut enums, &mut pending);
    }

    let var_tables: DrizzleTableMap = pending
        .iter()
        .map(|p| {
            (
                p.var_name.clone(),
                (
                    p.table.schema.clone(),
                    p.table.name.clone(),
                    p.col_prop_map.clone(),
                ),
            )
        })
        .collect();

    let mut out = Vec::new();
    for mut item in pending {
        if settings
            .exclude
            .iter()
            .any(|e| e == &item.var_name || e == &item.table.name)
        {
            continue;
        }

        if let Some(mapped) = settings.tables.get(&item.var_name) {
            item.table.set_qualified_name(mapped);
        }

        for (local_col, target_var, target_prop) in item.raw_refs {
            if let Some((ref_schema, ref_table, ref_cols)) = var_tables.get(&target_var) {
                let ref_col = ref_cols.get(&target_prop).cloned().unwrap_or(target_prop);
                item.table.relations.push(DbModelRelation {
                    columns: vec![local_col],
                    references: DbModelTableRef {
                        schema: ref_schema.clone(),
                        table: ref_table.clone(),
                    },
                    ref_columns: vec![ref_col],
                });
            }
        }
        out.push(item.table);
    }
    out
}

fn parse_drizzle_file(
    file: &str,
    src: &str,
    enums: &mut BTreeMap<String, Vec<String>>,
    out: &mut Vec<PendingTable>,
) {
    let alloc = Allocator::default();
    let source_type = SourceType::from_path(Path::new(file)).unwrap_or_default();
    let parsed = Parser::new(&alloc, src, source_type).parse();
    for stmt in &parsed.program.body {
        match stmt {
            Statement::VariableDeclaration(decl) => {
                visit_var_decl(file, src, decl, enums, out);
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::VariableDeclaration(decl)) = &exp.declaration {
                    visit_var_decl(file, src, decl, enums, out);
                }
            }
            _ => {}
        }
    }
}

fn visit_var_decl(
    file: &str,
    src: &str,
    decl: &VariableDeclaration<'_>,
    enums: &mut BTreeMap<String, Vec<String>>,
    out: &mut Vec<PendingTable>,
) {
    for declarator in &decl.declarations {
        let Some(var_name) = declarator
            .id
            .get_identifier_name()
            .map(|s| s.as_str().to_owned())
        else {
            continue;
        };
        let Some(Expression::CallExpression(call)) = &declarator.init else {
            continue;
        };
        let callee = callee_name(&call.callee);
        if matches!(callee.as_str(), "pgEnum" | "mysqlEnum") {
            if let Some(vals) = call.arguments.get(1).and_then(extract_string_array) {
                enums.insert(var_name, vals);
            }

            continue;
        }

        if (matches!(callee.as_str(), "pgTable" | "mysqlTable" | "sqliteTable")
            || callee.rsplit_once('.').is_some_and(|(_, m)| m == "table"))
            && let Some(tbl) = parse_table_call(file, src, &var_name, call, enums)
        {
            out.push(tbl);
        }
    }
}

fn parse_table_call(
    file: &str,
    src: &str,
    var_name: &str,
    call: &CallExpression<'_>,
    enums: &BTreeMap<String, Vec<String>>,
) -> Option<PendingTable> {
    let table_name = call.arguments.first().and_then(extract_string_lit)?;
    let Some(Argument::ObjectExpression(cols_obj)) = call.arguments.get(1) else {
        return None;
    };

    let line = byte_to_line(src, call.span().start as usize);
    let mut table = DbModelTable {
        schema: None,
        name: String::new(),
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line,
        }),
        columns: Vec::new(),
        primary_key: Vec::new(),
        join_table_for: Vec::new(),
        uniques: Vec::new(),
        relations: Vec::new(),
    };
    table.set_qualified_name(&table_name);

    let mut col_prop_map = BTreeMap::new();
    let mut raw_refs = Vec::new();

    for prop in &cols_obj.properties {
        let ObjectPropertyKind::ObjectProperty(p) = prop else {
            continue;
        };
        let Some(prop_name) = property_key_name(&p.key) else {
            continue;
        };
        let col_line = byte_to_line(src, p.span().start as usize);
        let parsed = parse_column_chain(&p.value, &prop_name, enums);
        col_prop_map.insert(prop_name, parsed.col_name.clone());
        if parsed.primary_key {
            table.primary_key.push(parsed.col_name.clone());
        }

        if let Some((tvar, tprop)) = parsed.reference {
            raw_refs.push((parsed.col_name.clone(), tvar, tprop));
        }

        table.columns.push(DbModelColumn {
            name: parsed.col_name,
            required: parsed.not_null || parsed.primary_key,
            unique: parsed.unique,
            max_length: parsed.max_length,
            enum_values: parsed.enum_values,
            source: Some(DbSourcePosition {
                file: file.to_owned(),
                line: col_line,
            }),
        });
    }

    // Optional 3rd argument: extra constraints callback `(t) => [uniqueIndex(...).on(t.a, t.b)]`
    if let Some(arg) = call.arguments.get(2) {
        let span = arg.span();
        if let Some(cb_src) = src.get(span.start as usize..span.end as usize) {
            parse_drizzle_extra_constraints(cb_src, &col_prop_map, &mut table, &mut raw_refs);
        }
    }

    Some(PendingTable {
        var_name: var_name.to_owned(),
        table,
        col_prop_map,
        raw_refs,
    })
}

struct ParsedDrizzleCol {
    col_name: String,
    not_null: bool,
    unique: bool,
    primary_key: bool,
    max_length: Option<u32>,
    enum_values: Vec<String>,
    reference: Option<(String, String)>,
}

fn parse_column_chain(
    expr: &Expression<'_>,
    fallback_name: &str,
    enums: &BTreeMap<String, Vec<String>>,
) -> ParsedDrizzleCol {
    let mut out = ParsedDrizzleCol {
        col_name: fallback_name.to_owned(),
        not_null: false,
        unique: false,
        primary_key: false,
        max_length: None,
        enum_values: Vec::new(),
        reference: None,
    };

    let mut chain = Vec::new();
    let mut cur = expr;
    while let Expression::CallExpression(call) = cur {
        chain.push(call);
        if let Expression::StaticMemberExpression(member) = &call.callee {
            cur = &member.object;
        } else {
            break;
        }
    }
    chain.reverse();

    for (idx, call) in chain.into_iter().enumerate() {
        let name = method_or_fn_name(&call.callee);
        if idx == 0 {
            if let Some(explicit) = call.arguments.first().and_then(extract_string_lit) {
                out.col_name = explicit;
            }

            if let Some(vals) = enums.get(&name) {
                out.enum_values.clone_from(vals);
            }

            if let Some(Argument::ObjectExpression(opts)) = call.arguments.get(1) {
                for p in &opts.properties {
                    if let ObjectPropertyKind::ObjectProperty(prop) = p
                        && let Some(key) = property_key_name(&prop.key)
                    {
                        if key == "length"
                            && let Expression::NumericLiteral(num) = &prop.value
                        {
                            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                            let len = num.value as u32;
                            if len > 0 {
                                out.max_length = Some(len);
                            }
                        } else if key == "enum"
                            && let Some(vals) = extract_expr_string_array(&prop.value)
                        {
                            out.enum_values = vals;
                        }
                    }
                }
            }
        } else {
            match name.as_str() {
                "notNull" => out.not_null = true,
                "unique" => out.unique = true,
                "primaryKey" => out.primary_key = true,
                "references" => {
                    if let Some(Argument::ArrowFunctionExpression(arrow)) = call.arguments.first() {
                        out.reference = extract_arrow_member(arrow);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn parse_drizzle_extra_constraints(
    cb_src: &str,
    prop_map: &BTreeMap<String, String>,
    table: &mut DbModelTable,
    raw_refs: &mut Vec<(String, String, String)>,
) {
    // Parse `uniqueIndex(...).on(t.a, t.b)` and `unique(...).on(t.a, t.b)`.
    for marker in ["uniqueIndex(", "unique("] {
        let mut rest = cb_src;
        while let Some(pos) = rest.find(marker) {
            let tail = &rest[pos + marker.len()..];
            if let Some(on_pos) = tail.find(".on(") {
                let args = &tail[on_pos + 4..];
                if let Some(close) = args.find(')') {
                    let cols = parse_member_props(&args[..close], prop_map);
                    if !cols.is_empty() {
                        table.uniques.push(DbModelUnique { columns: cols });
                    }
                }
            }

            rest = tail;
        }
    }

    // Parse `primaryKey({ columns: [t.a, t.b] })`.
    if let Some(pos) = cb_src.find("primaryKey(") {
        let tail = &cb_src[pos..];
        if let Some(open) = tail.find('[')
            && let Some(close) = tail[open + 1..].find(']')
        {
            let cols = parse_member_props(&tail[open + 1..open + 1 + close], prop_map);
            if !cols.is_empty() {
                table.primary_key = cols;
            }
        }
    }

    // Parse `foreignKey({ columns: [t.userId], foreignColumns: [users.id] })`.
    let mut rest = cb_src;
    while let Some(pos) = rest.find("foreignKey(") {
        let tail = &rest[pos + "foreignKey(".len()..];
        if let Some(fc_pos) = tail.find("foreignColumns:")
            && let Some(l_open) = tail.find('[')
            && let Some(l_close) = tail[l_open + 1..].find(']')
            && let Some(r_open) = tail[fc_pos..].find('[')
            && let Some(r_close) = tail[fc_pos + r_open + 1..].find(']')
        {
            let locals = parse_member_props(&tail[l_open + 1..l_open + 1 + l_close], prop_map);
            let foreign_raw = &tail[fc_pos + r_open + 1..fc_pos + r_open + 1 + r_close];
            let first_foreign = foreign_raw.split(',').next().unwrap_or("").trim();
            if let (Some(local), Some((tvar, tprop))) =
                (locals.first(), first_foreign.split_once('.'))
            {
                raw_refs.push((
                    local.clone(),
                    tvar.trim().to_owned(),
                    tprop.trim().to_owned(),
                ));
            }
        }

        rest = tail;
    }
}

fn parse_member_props(list: &str, prop_map: &BTreeMap<String, String>) -> Vec<String> {
    list.split(',')
        .filter_map(|item| {
            let prop = item.trim().rsplit('.').next()?.trim();
            if prop.is_empty() {
                return None;
            }

            Some(
                prop_map
                    .get(prop)
                    .cloned()
                    .unwrap_or_else(|| prop.to_owned()),
            )
        })
        .collect()
}

fn extract_arrow_member(arrow: &ArrowFunctionExpression<'_>) -> Option<(String, String)> {
    let stmt = arrow.body.statements.first()?;
    let expr = match stmt {
        Statement::ExpressionStatement(e) => &e.expression,
        Statement::ReturnStatement(r) => r.argument.as_ref()?,
        _ => return None,
    };
    if let Expression::StaticMemberExpression(m) = expr
        && let Expression::Identifier(obj) = &m.object
    {
        return Some((
            obj.name.as_str().to_owned(),
            m.property.name.as_str().to_owned(),
        ));
    }

    None
}

fn callee_name(expr: &Expression<'_>) -> String {
    match expr {
        Expression::Identifier(id) => id.name.as_str().to_owned(),
        Expression::StaticMemberExpression(m) => {
            format!("{}.{}", callee_name(&m.object), m.property.name.as_str())
        }
        _ => String::new(),
    }
}

fn method_or_fn_name(expr: &Expression<'_>) -> String {
    match expr {
        Expression::Identifier(id) => id.name.as_str().to_owned(),
        Expression::StaticMemberExpression(m) => m.property.name.as_str().to_owned(),
        _ => String::new(),
    }
}

fn property_key_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::StaticIdentifier(id) => Some(id.name.as_str().to_owned()),
        PropertyKey::StringLiteral(lit) => Some(lit.value.as_str().to_owned()),
        _ => None,
    }
}

fn extract_string_lit(arg: &Argument<'_>) -> Option<String> {
    if let Argument::StringLiteral(s) = arg {
        Some(s.value.as_str().to_owned())
    } else {
        None
    }
}

fn extract_string_array(arg: &Argument<'_>) -> Option<Vec<String>> {
    if let Argument::ArrayExpression(arr) = arg {
        let vals: Vec<String> = arr
            .elements
            .iter()
            .filter_map(|el| match el {
                ArrayExpressionElement::StringLiteral(s) => Some(s.value.as_str().to_owned()),
                _ => None,
            })
            .collect();
        (!vals.is_empty()).then_some(vals)
    } else {
        None
    }
}

fn extract_expr_string_array(expr: &Expression<'_>) -> Option<Vec<String>> {
    if let Expression::ArrayExpression(arr) = expr {
        let vals: Vec<String> = arr
            .elements
            .iter()
            .filter_map(|el| match el {
                ArrayExpressionElement::StringLiteral(s) => Some(s.value.as_str().to_owned()),
                _ => None,
            })
            .collect();
        (!vals.is_empty()).then_some(vals)
    } else {
        None
    }
}

fn byte_to_line(src: &str, offset: usize) -> u32 {
    let clamped = offset.min(src.len());
    let lines = src[..clamped].matches('\n').count();
    u32::try_from(lines + 1).unwrap_or(u32::MAX)
}
