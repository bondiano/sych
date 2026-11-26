//! Static Rust `SeaORM` entity extractor (`DeriveEntityModel` / `DeriveRelation`) using `syn`.

use std::collections::BTreeMap;

use quote::ToTokens;
use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbSourcePosition,
    ModelExtractionSettings,
};
use syn::spanned::Spanned;
use syn::{Attribute, Fields, Item, ItemEnum, ItemStruct};

use super::naming::to_snake_case;
use super::source::SourceTree;

struct PendingSeaOrm {
    mod_hint: String,
    table: DbModelTable,
    raw_relations: Vec<(String, String, String)>, // (local_col, target_hint, ref_col)
}

/// Extracts `SeaORM` entities from Rust files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["rs"], &settings.paths);
    let mut enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut pending: Vec<PendingSeaOrm> = Vec::new();

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("DeriveEntityModel") && !src.contains("DeriveActiveEnum") {
            continue;
        }

        let Ok(parsed) = syn::parse_file(&src) else {
            continue;
        };
        collect_active_enums(&parsed.items, &mut enums);
    }

    for file in &files {
        let Some(src) = tree.read_text(file) else {
            continue;
        };
        if !src.contains("DeriveEntityModel") {
            continue;
        }

        let Ok(parsed) = syn::parse_file(&src) else {
            continue;
        };
        let mod_hint = file_stem(file);
        collect_entities(
            file,
            &mod_hint,
            &parsed.items,
            &enums,
            settings,
            &mut pending,
        );
    }

    let by_mod: BTreeMap<String, (Option<String>, String)> = pending
        .iter()
        .map(|p| {
            (
                p.mod_hint.clone(),
                (p.table.schema.clone(), p.table.name.clone()),
            )
        })
        .collect();

    let mut out = Vec::new();
    for mut item in pending {
        for (local_col, target_hint, ref_col) in item.raw_relations {
            let (ref_schema, ref_table) = by_mod
                .get(&target_hint)
                .cloned()
                .unwrap_or((None, target_hint));
            item.table.relations.push(DbModelRelation {
                columns: vec![local_col],
                references: DbModelTableRef {
                    schema: ref_schema,
                    table: ref_table,
                },
                ref_columns: vec![ref_col],
            });
        }
        out.push(item.table);
    }
    out
}

fn collect_active_enums(items: &[Item], enums: &mut BTreeMap<String, Vec<String>>) {
    for item in items {
        match item {
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect_active_enums(inner, enums);
                }
            }
            Item::Enum(en) if has_derive(&en.attrs, "DeriveActiveEnum") => {
                let vals: Vec<String> = en
                    .variants
                    .iter()
                    .filter_map(|v| {
                        extract_sea_orm_kv(&v.attrs, "string_value")
                            .or_else(|| Some(v.ident.to_string()))
                    })
                    .collect();
                if !vals.is_empty() {
                    enums.insert(en.ident.to_string(), vals);
                }
            }
            _ => {}
        }
    }
}

fn collect_entities(
    file: &str,
    mod_hint: &str,
    items: &[Item],
    enums: &BTreeMap<String, Vec<String>>,
    settings: &ModelExtractionSettings,
    out: &mut Vec<PendingSeaOrm>,
) {
    let mut local_rels = Vec::new();
    let mut local_models = Vec::new();

    for item in items {
        match item {
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect_entities(file, &m.ident.to_string(), inner, enums, settings, out);
                }
            }
            Item::Struct(st) if has_derive(&st.attrs, "DeriveEntityModel") => {
                local_models.push(st);
            }
            Item::Enum(en) if has_derive(&en.attrs, "DeriveRelation") => {
                local_rels.extend(parse_relations(en));
            }
            _ => {}
        }
    }

    for st in local_models {
        if let Some(table) = parse_model_struct(file, mod_hint, st, enums, settings) {
            out.push(PendingSeaOrm {
                mod_hint: mod_hint.to_owned(),
                table,
                raw_relations: local_rels.clone(),
            });
        }
    }
}

fn parse_model_struct(
    file: &str,
    mod_hint: &str,
    st: &ItemStruct,
    enums: &BTreeMap<String, Vec<String>>,
    settings: &ModelExtractionSettings,
) -> Option<DbModelTable> {
    let raw_name =
        extract_sea_orm_kv(&st.attrs, "table_name").unwrap_or_else(|| to_snake_case(mod_hint));
    if settings
        .exclude
        .iter()
        .any(|e| e == &raw_name || e == &st.ident.to_string())
    {
        return None;
    }

    let schema = extract_sea_orm_kv(&st.attrs, "schema_name");
    let line = u32::try_from(st.span().start().line).unwrap_or(1);

    let mut columns = Vec::new();
    let mut primary_key = Vec::new();

    if let Fields::Named(named) = &st.fields {
        for field in &named.named {
            if has_sea_orm_flag(&field.attrs, "ignore") {
                continue;
            }

            let Some(ident) = &field.ident else {
                continue;
            };
            let col_name = extract_sea_orm_kv(&field.attrs, "column_name")
                .unwrap_or_else(|| ident.to_string());
            let fline = u32::try_from(field.span().start().line).unwrap_or(line);

            let ty_str = field.ty.to_token_stream().to_string().replace(' ', "");
            let is_option = ty_str.starts_with("Option<") || ty_str.contains("::Option<");
            let inner_ty = ty_str
                .strip_prefix("Option<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or(&ty_str);
            let inner_leaf = inner_ty.rsplit("::").next().unwrap_or(inner_ty);

            let pk = has_sea_orm_flag(&field.attrs, "primary_key");
            let unique = has_sea_orm_flag(&field.attrs, "unique");
            let nullable_attr = has_sea_orm_flag(&field.attrs, "nullable");
            let max_length = extract_sea_orm_string_len(&field.attrs);
            let enum_values = enums.get(inner_leaf).cloned().unwrap_or_default();

            if pk {
                primary_key.push(col_name.clone());
            }

            columns.push(DbModelColumn {
                name: col_name,
                required: pk || (!is_option && !nullable_attr),
                unique,
                max_length,
                enum_values,
                source: Some(DbSourcePosition {
                    file: file.to_owned(),
                    line: fline,
                }),
            });
        }
    }

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
    let effective = settings
        .tables
        .get(&raw_name)
        .map_or(raw_name.as_str(), String::as_str);
    table.set_qualified_name(effective);
    Some(table)
}

fn parse_relations(en: &ItemEnum) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for var in &en.variants {
        let Some(belongs_to) = extract_sea_orm_kv(&var.attrs, "belongs_to") else {
            continue;
        };
        let Some(from) = extract_sea_orm_kv(&var.attrs, "from") else {
            continue;
        };
        let Some(to) = extract_sea_orm_kv(&var.attrs, "to") else {
            continue;
        };
        let local_col = to_snake_case(from.rsplit("::").next().unwrap_or(&from));
        let ref_col = to_snake_case(to.rsplit("::").next().unwrap_or(&to));
        let target_hint = belongs_to
            .trim_end_matches("::Entity")
            .rsplit("::")
            .next()
            .unwrap_or(&belongs_to)
            .to_owned();
        if !local_col.is_empty() && !target_hint.is_empty() {
            out.push((local_col, target_hint, ref_col));
        }
    }
    out
}

fn has_derive(attrs: &[Attribute], target: &str) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("derive") && attr.to_token_stream().to_string().contains(target)
    })
}

fn has_sea_orm_flag(attrs: &[Attribute], flag: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("sea_orm") {
            return false;
        }

        let text = attr.to_token_stream().to_string();
        text.split([',', '(', ')']).any(|part| part.trim() == flag)
    })
}

fn extract_sea_orm_kv(attrs: &[Attribute], key: &str) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("sea_orm") {
            continue;
        }

        let text = attr.to_token_stream().to_string();
        if let Some(pos) = text.find(key) {
            let rest = text[pos + key.len()..].trim_start();
            if let Some(after_eq) = rest.strip_prefix('=') {
                let val = after_eq.trim_start();
                if let Some(q1) = val.strip_prefix('"')
                    && let Some(end) = q1.find('"')
                {
                    return Some(q1[..end].to_owned());
                }
            }
        }
    }
    None
}

fn extract_sea_orm_string_len(attrs: &[Attribute]) -> Option<u32> {
    let raw = extract_sea_orm_kv(attrs, "column_type")?;
    let pos = raw.find("String(StringLen::N(")?;
    let rest = &raw[pos + "String(StringLen::N(".len()..];
    let end = rest.find(')')?;
    rest[..end].trim().parse::<u32>().ok().filter(|&n| n > 0)
}

fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".rs").unwrap_or(name).to_owned()
}
