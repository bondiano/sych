//! Static Rust Diesel ORM extractor (`table! { ... }` and `joinable!(...)`) using `syn`.

use std::collections::BTreeMap;

use proc_macro2::TokenTree;
use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbSourcePosition,
    ModelExtractionSettings,
};
use syn::spanned::Spanned;
use syn::{Item, ItemMacro};

use super::source::SourceTree;

/// Extracts Diesel tables and `joinable!` foreign keys from Rust files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["rs"], &settings.paths);
    let mut tables: BTreeMap<String, DbModelTable> = BTreeMap::new();
    let mut joinables: Vec<(String, String, String)> = Vec::new(); // (child, parent, fk_col)

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("table!") && !src.contains("joinable!") {
            continue;
        }

        let Ok(parsed) = syn::parse_file(&src) else {
            continue;
        };
        collect_items(file, &parsed.items, settings, &mut tables, &mut joinables);
    }

    for (child, parent, fk_col) in joinables {
        let parent_pk = tables
            .get(&parent)
            .and_then(|t| t.primary_key.first().cloned())
            .unwrap_or_else(|| "id".into());
        let parent_schema = tables.get(&parent).and_then(|t| t.schema.clone());
        if let Some(child_tbl) = tables.get_mut(&child) {
            child_tbl.relations.push(DbModelRelation {
                columns: vec![fk_col],
                references: DbModelTableRef {
                    schema: parent_schema,
                    table: parent,
                },
                ref_columns: vec![parent_pk],
            });
        }
    }

    tables.into_values().collect()
}

fn collect_items(
    file: &str,
    items: &[Item],
    settings: &ModelExtractionSettings,
    tables: &mut BTreeMap<String, DbModelTable>,
    joinables: &mut Vec<(String, String, String)>,
) {
    for item in items {
        match item {
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect_items(file, inner, settings, tables, joinables);
                }
            }
            Item::Macro(mac) => {
                let Some(ident) = mac.mac.path.segments.last().map(|s| s.ident.to_string()) else {
                    continue;
                };
                if ident == "table" {
                    if let Some(tbl) = parse_table_macro(file, mac, settings) {
                        tables.insert(tbl.name.clone(), tbl);
                    }
                } else if ident == "joinable"
                    && let Some(j) = parse_joinable_macro(mac)
                {
                    joinables.push(j);
                }
            }
            _ => {}
        }
    }
}

fn parse_table_macro(
    file: &str,
    mac: &ItemMacro,
    settings: &ModelExtractionSettings,
) -> Option<DbModelTable> {
    let tokens: Vec<TokenTree> = mac.mac.tokens.clone().into_iter().collect();
    let line = u32::try_from(mac.span().start().line).unwrap_or(1);

    let mut i = 0;
    // Skip `use ...;` imports inside `diesel::table!`.
    while i < tokens.len() {
        if let TokenTree::Ident(id) = &tokens[i]
            && *id == "use"
        {
            while i < tokens.len() {
                let is_semi = matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == ';');
                i += 1;
                if is_semi {
                    break;
                }
            }
            continue;
        }

        // Skip outer attributes like `#[sql_name = "..."]`.
        if matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == '#') {
            i += 2;
            continue;
        }

        break;
    }

    // Read `[schema.]table_name (pk1, pk2) { ... }`
    let first = match tokens.get(i)? {
        TokenTree::Ident(id) => id.to_string(),
        _ => return None,
    };
    i += 1;
    let (schema, raw_name) = if matches!(tokens.get(i), Some(TokenTree::Punct(p)) if p.as_char() == '.')
    {
        i += 1;
        let tbl = match tokens.get(i)? {
            TokenTree::Ident(id) => id.to_string(),
            _ => return None,
        };
        i += 1;
        (Some(first), tbl)
    } else {
        (None, first)
    };

    if settings.exclude.iter().any(|e| e == &raw_name) {
        return None;
    }

    let mut primary_key = Vec::new();
    if let Some(TokenTree::Group(g)) = tokens.get(i) {
        for tt in g.stream() {
            if let TokenTree::Ident(id) = tt {
                primary_key.push(id.to_string());
            }
        }
        i += 1;
    }

    let body = match tokens.get(i)? {
        TokenTree::Group(g) => g.stream(),
        _ => return None,
    };
    let columns = parse_diesel_columns(file, body);

    let mut table = DbModelTable {
        schema,
        name: String::new(),
        source: Some(DbSourcePosition {
            file: file.to_owned(),
            line,
        }),
        columns,
        primary_key,
        join_table_for: Vec::new(),
        uniques: Vec::new(),
        relations: Vec::new(),
    };
    let effective_name = settings
        .tables
        .get(&raw_name)
        .map_or(raw_name.as_str(), String::as_str);
    table.set_qualified_name(effective_name);
    Some(table)
}

fn parse_diesel_columns(file: &str, stream: proc_macro2::TokenStream) -> Vec<DbModelColumn> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut columns = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let mut max_length = None;
        let mut sql_name = None;
        // Parse `#[max_length = 255]` / `#[sql_name = "foo"]` attributes before column.
        while i + 1 < tokens.len()
            && matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == '#')
        {
            if let TokenTree::Group(attr_group) = &tokens[i + 1] {
                let text = attr_group.stream().to_string();
                if let Some(rest) = text.strip_prefix("max_length")
                    && let Some((_, val)) = rest.split_once('=')
                {
                    max_length = val.trim().parse::<u32>().ok().filter(|&n| n > 0);
                } else if let Some(rest) = text.strip_prefix("sql_name")
                    && let Some((_, val)) = rest.split_once('=')
                {
                    let trimmed = val.trim().trim_matches('"');
                    if !trimmed.is_empty() {
                        sql_name = Some(trimmed.to_owned());
                    }
                }
            }

            i += 2;
        }

        let Some(TokenTree::Ident(col_ident)) = tokens.get(i) else {
            i += 1;
            continue;
        };
        let col_line = u32::try_from(col_ident.span().start().line).unwrap_or(1);
        let col_name = sql_name.unwrap_or_else(|| col_ident.to_string());
        i += 1;

        // Collect tokens up to the next `,` for the SQL type expression.
        let mut ty_tokens = String::new();
        while i < tokens.len() {
            if matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == ',') {
                i += 1;
                break;
            }

            ty_tokens.push_str(&tokens[i].to_string());
            i += 1;
        }

        let nullable = ty_tokens.contains("Nullable<");
        columns.push(DbModelColumn {
            name: col_name,
            required: !nullable,
            unique: false,
            max_length,
            enum_values: Vec::new(),
            source: Some(DbSourcePosition {
                file: file.to_owned(),
                line: col_line,
            }),
        });
    }
    columns
}

fn parse_joinable_macro(mac: &ItemMacro) -> Option<(String, String, String)> {
    // Format: `joinable!(child -> parent (fk_col));`
    let tokens: Vec<TokenTree> = mac.mac.tokens.clone().into_iter().collect();
    let mut idents = Vec::new();
    let mut fk_col = None;
    for tt in tokens {
        match tt {
            TokenTree::Ident(id) => idents.push(id.to_string()),
            TokenTree::Group(g) => {
                if let Some(TokenTree::Ident(col)) = g.stream().into_iter().next() {
                    fk_col = Some(col.to_string());
                }
            }
            _ => {}
        }
    }
    if idents.len() >= 2
        && let Some(col) = fk_col
    {
        return Some((idents[0].clone(), idents[1].clone(), col));
    }

    None
}
