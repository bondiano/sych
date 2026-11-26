//! Static Prisma schema (`.prisma`) application model extractor.

use std::collections::BTreeMap;
use std::mem;

use sych_core::db_schema::{
    DbModelColumn, DbModelRelation, DbModelTable, DbModelTableRef, DbModelUnique, DbSourcePosition,
    ModelExtractionSettings,
};

use super::source::SourceTree;

const SCALAR_TYPES: &[&str] = &[
    "String", "Boolean", "Int", "BigInt", "Float", "Decimal", "DateTime", "Json", "Bytes",
];

type PrismaModelLookup = BTreeMap<String, (Option<String>, String, BTreeMap<String, String>)>;

/// Extracts model tables from all `*.prisma` files in `tree`.
pub fn extract(tree: &SourceTree<'_>, settings: &ModelExtractionSettings) -> Vec<DbModelTable> {
    let files = tree.files_with_extensions(&["prisma"], &settings.paths);
    let mut enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut raw_models: Vec<ParsedModel> = Vec::new();

    for file in &files {
        let Some(text) = tree.read_text(file) else {
            continue;
        };
        parse_prisma_file(file, &text, &mut enums, &mut raw_models);
    }

    let mut model_info: PrismaModelLookup = BTreeMap::new();
    for m in &raw_models {
        let table_name = settings
            .tables
            .get(&m.model_name)
            .cloned()
            .or_else(|| m.mapped_table.clone())
            .unwrap_or_else(|| m.model_name.clone());
        let field_map: BTreeMap<String, String> = m
            .fields
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    f.mapped_col.clone().unwrap_or_else(|| f.name.clone()),
                )
            })
            .collect();
        model_info.insert(
            m.model_name.clone(),
            (m.schema.clone(), table_name, field_map),
        );
    }

    raw_models
        .into_iter()
        .filter(|m| !settings.exclude.iter().any(|e| e == &m.model_name))
        .filter_map(|m| build_prisma_table(m, &enums, &model_info))
        .collect()
}

fn build_prisma_table(
    m: ParsedModel,
    enums: &BTreeMap<String, Vec<String>>,
    model_info: &PrismaModelLookup,
) -> Option<DbModelTable> {
    let (schema, table_name, field_map) = model_info.get(&m.model_name)?;
    let mut columns = Vec::new();
    let mut primary_key = Vec::new();
    let mut uniques = Vec::new();
    let mut relations = Vec::new();

    for f in &m.fields {
        if let Some(rel) = &f.relation {
            if let Some((ref_schema, ref_table, ref_fields)) = model_info.get(&f.base_type) {
                let local_cols: Vec<String> = rel
                    .fields
                    .iter()
                    .map(|n| field_map.get(n).cloned().unwrap_or_else(|| n.clone()))
                    .collect();
                let ref_cols: Vec<String> = rel
                    .references
                    .iter()
                    .map(|n| ref_fields.get(n).cloned().unwrap_or_else(|| n.clone()))
                    .collect();
                if !local_cols.is_empty() {
                    relations.push(DbModelRelation {
                        columns: local_cols,
                        references: DbModelTableRef {
                            schema: ref_schema.clone(),
                            table: ref_table.clone(),
                        },
                        ref_columns: ref_cols,
                    });
                }
            }

            continue;
        }

        let is_enum = enums.contains_key(&f.base_type);
        let is_scalar = SCALAR_TYPES.contains(&f.base_type.as_str()) || is_enum;
        if !is_scalar || f.is_list {
            continue;
        }

        let col_name = f.mapped_col.clone().unwrap_or_else(|| f.name.clone());
        if f.is_id {
            primary_key.push(col_name.clone());
        }

        columns.push(DbModelColumn {
            name: col_name,
            required: !f.is_optional,
            unique: f.is_unique,
            max_length: f.max_length,
            enum_values: enums.get(&f.base_type).cloned().unwrap_or_default(),
            source: Some(DbSourcePosition {
                file: m.file.clone(),
                line: f.line,
            }),
        });
    }

    for id_group in &m.block_ids {
        primary_key = id_group
            .iter()
            .map(|n| field_map.get(n).cloned().unwrap_or_else(|| n.clone()))
            .collect();
    }
    for uq_group in &m.block_uniques {
        let cols: Vec<String> = uq_group
            .iter()
            .map(|n| field_map.get(n).cloned().unwrap_or_else(|| n.clone()))
            .collect();
        if !cols.is_empty() {
            uniques.push(DbModelUnique { columns: cols });
        }
    }

    Some(DbModelTable {
        schema: schema.clone(),
        name: table_name.clone(),
        source: Some(DbSourcePosition {
            file: m.file,
            line: m.line,
        }),
        columns,
        primary_key,
        join_table_for: Vec::new(),
        uniques,
        relations,
    })
}

struct ParsedModel {
    file: String,
    line: u32,
    model_name: String,
    schema: Option<String>,
    mapped_table: Option<String>,
    fields: Vec<ParsedField>,
    block_ids: Vec<Vec<String>>,
    block_uniques: Vec<Vec<String>>,
}

#[allow(clippy::struct_excessive_bools)]
struct ParsedField {
    line: u32,
    name: String,
    base_type: String,
    is_optional: bool,
    is_list: bool,
    is_id: bool,
    is_unique: bool,
    mapped_col: Option<String>,
    max_length: Option<u32>,
    relation: Option<ParsedRelation>,
}

struct ParsedRelation {
    fields: Vec<String>,
    references: Vec<String>,
}

fn parse_prisma_file(
    file: &str,
    text: &str,
    enums: &mut BTreeMap<String, Vec<String>>,
    models: &mut Vec<ParsedModel>,
) {
    enum Block {
        None,
        Enum(String, Vec<String>),
        Model(ParsedModel),
    }

    let mut block = Block::None;
    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let line = strip_prisma_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        match &mut block {
            Block::None => {
                if let Some(rest) = line.strip_prefix("enum ")
                    && let Some(name) = rest.split_whitespace().next()
                    && line.contains('{')
                {
                    block = Block::Enum(name.trim_end_matches('{').to_owned(), Vec::new());
                } else if let Some(rest) = line.strip_prefix("model ")
                    && let Some(name) = rest.split_whitespace().next()
                    && line.contains('{')
                {
                    block = Block::Model(ParsedModel {
                        file: file.to_owned(),
                        line: line_no,
                        model_name: name.trim_end_matches('{').to_owned(),
                        schema: None,
                        mapped_table: None,
                        fields: Vec::new(),
                        block_ids: Vec::new(),
                        block_uniques: Vec::new(),
                    });
                }
            }
            Block::Enum(_, vals) => {
                if line.starts_with('}') {
                    if let Block::Enum(name, vals) = mem::replace(&mut block, Block::None) {
                        enums.insert(name, vals);
                    }

                    continue;
                }

                if line.starts_with("@@") {
                    continue;
                }

                if let Some(ident) = line.split_whitespace().next() {
                    let val =
                        extract_attr_string(line, "@map(").unwrap_or_else(|| ident.to_owned());
                    vals.push(val);
                }
            }
            Block::Model(m) => {
                if line.starts_with('}') {
                    if let Block::Model(model) = mem::replace(&mut block, Block::None) {
                        models.push(model);
                    }

                    continue;
                }

                if line.starts_with("@@map(") {
                    if let Some(mapped) = extract_attr_string(line, "@@map(") {
                        m.mapped_table = Some(mapped);
                    }
                } else if line.starts_with("@@schema(") {
                    if let Some(schema) = extract_attr_string(line, "@@schema(") {
                        m.schema = Some(schema);
                    }
                } else if line.starts_with("@@id(") {
                    let cols = extract_bracket_list(line);
                    if !cols.is_empty() {
                        m.block_ids.push(cols);
                    }
                } else if line.starts_with("@@unique(") {
                    let cols = extract_bracket_list(line);
                    if !cols.is_empty() {
                        m.block_uniques.push(cols);
                    }
                } else if !line.starts_with("@@")
                    && let Some(field) = parse_field_line(line, line_no)
                {
                    m.fields.push(field);
                }
            }
        }
    }
}

fn strip_prisma_comment(line: &str) -> &str {
    let mut in_str = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            in_str = !in_str;
        } else if !in_str && i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            return &line[..i];
        }

        i += 1;
    }
    line
}

fn parse_field_line(line: &str, line_no: u32) -> Option<ParsedField> {
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_owned();
    let raw_type = parts.next()?;
    let is_list = raw_type.ends_with("[]");
    let without_list = raw_type.strip_suffix("[]").unwrap_or(raw_type);
    let is_optional = without_list.ends_with('?');
    let base_type = without_list
        .strip_suffix('?')
        .unwrap_or(without_list)
        .to_owned();
    let attrs = line;

    let is_id = has_word_attr(attrs, "@id");
    let is_unique = has_word_attr(attrs, "@unique");
    let mapped_col = extract_attr_string(attrs, "@map(");
    let max_length = extract_db_char_len(attrs);
    let relation = extract_relation(attrs);

    Some(ParsedField {
        line: line_no,
        name,
        base_type,
        is_optional,
        is_list,
        is_id,
        is_unique,
        mapped_col,
        max_length,
        relation,
    })
}

fn has_word_attr(line: &str, attr: &str) -> bool {
    line.match_indices(attr).any(|(pos, _)| {
        let after = line.as_bytes().get(pos + attr.len()).copied();
        let before = pos
            .checked_sub(1)
            .and_then(|p| line.as_bytes().get(p).copied());
        before.is_none_or(|b| b.is_ascii_whitespace())
            && after.is_none_or(|b| b.is_ascii_whitespace() || b == b'(')
    })
}

fn extract_attr_string(line: &str, prefix: &str) -> Option<String> {
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let q1 = rest.find('"')? + 1;
    let q2 = rest[q1..].find('"')?;
    Some(rest[q1..q1 + q2].to_owned())
}

fn extract_db_char_len(line: &str) -> Option<u32> {
    for pat in ["@db.VarChar(", "@db.Char(", "@db.NVarChar(", "@db.NChar("] {
        if let Some(pos) = line.find(pat) {
            let rest = &line[pos + pat.len()..];
            if let Some(end) = rest.find(')')
                && let Ok(n) = rest[..end].trim().parse::<u32>()
            {
                return Some(n);
            }
        }
    }
    None
}

fn extract_bracket_list(segment: &str) -> Vec<String> {
    let Some(open) = segment.find('[') else {
        return Vec::new();
    };
    let Some(close) = segment[open + 1..].find(']') else {
        return Vec::new();
    };
    segment[open + 1..open + 1 + close]
        .split(',')
        .filter_map(|item| {
            let clean = item.trim();
            // Strip optional Prisma sort/length args like `email(length: 100)`.
            let name = clean.split('(').next()?.trim().trim_matches('"');
            (!name.is_empty()).then(|| name.to_owned())
        })
        .collect()
}

fn extract_relation(line: &str) -> Option<ParsedRelation> {
    let start = line.find("@relation(")? + "@relation(".len();
    let rest = &line[start..];
    let fields_pos = rest.find("fields:")?;
    let fields = extract_bracket_list(&rest[fields_pos..]);
    let references = rest
        .find("references:")
        .map(|p| extract_bracket_list(&rest[p..]))
        .unwrap_or_default();
    if fields.is_empty() {
        None
    } else {
        Some(ParsedRelation { fields, references })
    }
}
