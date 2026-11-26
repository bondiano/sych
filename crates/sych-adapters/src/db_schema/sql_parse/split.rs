//! SQL statement splitter, directive filters and index-part AST helpers.

use sqlparser::ast::{Expr, Function, IndexColumn, OrderBySort};
use sych_core::db_schema::{DbIndexPart, DbSourcePosition, SqlDialect};

pub(super) fn is_session_directive(dialect: SqlDialect, stmt: &str) -> bool {
    let upper = stmt.trim_start().to_ascii_uppercase();
    (dialect == SqlDialect::Sqlite && upper.starts_with("PRAGMA "))
        || upper == "BEGIN"
        || upper == "BEGIN TRANSACTION"
        || upper == "COMMIT"
        || upper == "COMMIT TRANSACTION"
}

pub(super) fn is_procedural_block(stmt: &str) -> bool {
    let upper = stmt.trim_start().to_ascii_uppercase();
    upper.starts_with("DO ") || upper.starts_with("DO$") || upper.starts_with("DELIMITER ")
}

/// Splits SQL text on `;` outside quotes, comments and PostgreSQL dollar-quoted strings,
/// returning `(1-based start line, statement slice)` pairs.
pub(super) fn split_sql_statements(sql: &str) -> Vec<(u32, &str)> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line: u32 = 1;
    let mut stmt_start: Option<(usize, u32)> = None;

    while i < bytes.len() {
        let b = bytes[i];
        // Line comment `-- ... \n`
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Block comment `/* ... */`
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                if bytes[i] == b'\n' {
                    line += 1;
                }

                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }

        if stmt_start.is_none() && !b.is_ascii_whitespace() && b != b';' {
            stmt_start = Some((i, line));
        }

        // Single/double/backtick quoted literal
        if matches!(b, b'\'' | b'"' | b'`') {
            let quote = b;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\n' {
                    line += 1;
                }

                if bytes[i] == b'\\' && quote == b'\'' {
                    i += 2;
                    continue;
                }

                if bytes[i] == quote {
                    if bytes.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }

                    i += 1;
                    break;
                }

                i += 1;
            }
            continue;
        }

        // PostgreSQL dollar-quoted string `$tag$ ... $tag$`
        if b == b'$'
            && let Some(tag_len) = dollar_quote_tag(&bytes[i..])
        {
            let tag = &bytes[i..i + tag_len];
            i += tag_len;
            while i < bytes.len() {
                if bytes[i] == b'\n' {
                    line += 1;
                }

                if bytes[i..].starts_with(tag) {
                    i += tag_len;
                    break;
                }

                i += 1;
            }
            continue;
        }

        if b == b'\n' {
            line += 1;
        } else if b == b';'
            && let Some((start_idx, start_line)) = stmt_start.take()
        {
            let slice = &sql[start_idx..i];
            if !slice.trim().is_empty() {
                out.push((start_line, slice));
            }
        }

        i += 1;
    }

    if let Some((start_idx, start_line)) = stmt_start {
        let slice = &sql[start_idx..];
        if !slice.trim().is_empty() {
            out.push((start_line, slice));
        }
    }

    out
}

fn dollar_quote_tag(slice: &[u8]) -> Option<usize> {
    if slice.first() != Some(&b'$') {
        return None;
    }

    for (idx, &b) in slice.iter().enumerate().skip(1) {
        if b == b'$' {
            return Some(idx + 1);
        }

        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
    }
    None
}

pub(super) fn col_part(name: String) -> DbIndexPart {
    DbIndexPart {
        column: Some(name),
        expression: None,
        desc: false,
        opclass: None,
        prefix_length: None,
    }
}

pub(super) fn index_col_to_part(ic: &IndexColumn) -> DbIndexPart {
    let desc = matches!(ic.column.options.sort, Some(OrderBySort::Desc));
    let opclass = ic.operator_class.as_ref().map(ToString::to_string);
    match &ic.column.expr {
        Expr::Identifier(id) => DbIndexPart {
            column: Some(id.value.clone()),
            expression: None,
            desc,
            opclass,
            prefix_length: None,
        },
        // MySQL prefix index `col(20)` parses as `Function { name: col, args: (20) }`.
        Expr::Function(func) if func.name.0.len() == 1 => {
            if let Some(len) = extract_prefix_function_len(func) {
                DbIndexPart {
                    column: Some(unquote_ident(&func.name.0[0].to_string())),
                    expression: None,
                    desc,
                    opclass,
                    prefix_length: Some(len),
                }
            } else {
                DbIndexPart {
                    column: None,
                    expression: Some(ic.column.expr.to_string()),
                    desc,
                    opclass,
                    prefix_length: None,
                }
            }
        }
        other => DbIndexPart {
            column: None,
            expression: Some(other.to_string()),
            desc,
            opclass,
            prefix_length: None,
        },
    }
}

fn extract_prefix_function_len(func: &Function) -> Option<u32> {
    let s = func.args.to_string();
    let trimmed = s
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    trimmed.parse::<u32>().ok().filter(|&n| n > 0)
}

pub(super) fn unquote_ident(raw: &str) -> String {
    raw.trim().trim_matches(['"', '`', '[', ']']).to_owned()
}

pub(super) fn pos(file: &str, base_line: u32, rel_line: u64) -> DbSourcePosition {
    let offset = u32::try_from(rel_line.saturating_sub(1)).unwrap_or(0);
    DbSourcePosition {
        file: file.to_owned(),
        line: base_line.saturating_add(offset),
    }
}
