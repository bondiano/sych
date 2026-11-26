//! Database and application schema adapters: read-only catalog inspection
//! (PostgreSQL, MySQL, SQLite), static SQL DDL reconstruction, built-in
//! static ORM model extractors and review-pipeline fact collection.

pub mod collect;
pub mod extract;
pub mod inspect;
pub mod sql_parse;

use std::io;
use std::path::Path;

use sych_core::db_schema::{DbColumnType, SqlDialect};
use thiserror::Error;
use url::Url;

/// Errors returned by database schema inspection, SQL reconstruction or model extraction.
#[derive(Debug, Error)]
pub enum DbSchemaError {
    #[error("invalid database DSN `{dsn}`: {detail}")]
    InvalidDsn { dsn: String, detail: String },
    #[error("cannot connect to database `{dsn}`: {detail}")]
    Connect { dsn: String, detail: String },
    #[error("database inspection query failed ({dsn}): {detail}")]
    Query { dsn: String, detail: String },
    #[error("database inspection timed out after {seconds}s ({dsn})")]
    Timeout { dsn: String, seconds: u64 },
    #[error("SQL parse error: {0}")]
    SqlParse(String),
    #[error("model extraction failed: {0}")]
    ModelExtract(String),
    #[error("external schema adapter `{adapter}` failed: {detail}")]
    ExternalAdapter { adapter: String, detail: String },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

/// Strips credentials (password, userinfo and password query parameters) from a DSN
/// before it can ever appear in an error message, log or limitation string.
pub fn redact_dsn(dsn: &str) -> String {
    let trimmed = dsn.trim();
    if let Ok(mut parsed) = Url::parse(trimmed) {
        if parsed.password().is_some() {
            let _ = parsed.set_password(Some("REDACTED"));
        }

        if parsed.query().is_some() {
            let pairs: Vec<(String, String)> = parsed
                .query_pairs()
                .map(|(k, v)| {
                    let key_lower = k.to_ascii_lowercase();
                    if key_lower.contains("pass")
                        || key_lower.contains("secret")
                        || key_lower.contains("token")
                    {
                        (k.into_owned(), "REDACTED".to_owned())
                    } else {
                        (k.into_owned(), v.into_owned())
                    }
                })
                .collect();
            parsed.query_pairs_mut().clear().extend_pairs(pairs);
        }

        return parsed.to_string();
    }

    // Non-URL DSN (e.g. SQLite path or PG key=value string): mask password=... tokens.
    let mut parts = Vec::new();
    for token in trimmed.split_whitespace() {
        if let Some((k, _)) = token.split_once('=') {
            let lower = k.to_ascii_lowercase();
            if lower.contains("pass") || lower.contains("secret") || lower.contains("token") {
                parts.push(format!("{k}=REDACTED"));
                continue;
            }
        }

        parts.push(token.to_owned());
    }
    if parts.len() > 1 {
        return parts.join(" ");
    }

    // Fallback for malformed URLs containing `://user:pass@host`: mask everything between `://` and `@`.
    if let Some((scheme, rest)) = trimmed.split_once("://")
        && let Some((_, host_part)) = rest.rsplit_once('@')
    {
        return format!("{scheme}://REDACTED@{host_part}");
    }

    trimmed.to_owned()
}

/// Detects the SQL dialect from a connection DSN or local SQLite database path.
pub fn detect_dialect(dsn: &str) -> Result<SqlDialect, DbSchemaError> {
    let trimmed = dsn.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("postgres://") || lower.starts_with("postgresql://") {
        return Ok(SqlDialect::Postgres);
    }

    if lower.starts_with("mysql://") || lower.starts_with("mariadb://") {
        return Ok(SqlDialect::Mysql);
    }

    if lower.starts_with("sqlite://") || lower.starts_with("sqlite:") || lower.starts_with("file:")
    {
        return Ok(SqlDialect::Sqlite);
    }

    let path = Path::new(trimmed);
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "db" | "sqlite" | "sqlite3"
            )
        })
    {
        return Ok(SqlDialect::Sqlite);
    }

    Err(DbSchemaError::InvalidDsn {
        dsn: redact_dsn(trimmed),
        detail: "unsupported scheme; expected postgres://, mysql://, sqlite:// or a .db/.sqlite file path".into(),
    })
}

/// Normalizes a raw SQL type name into [`DbColumnType`].
pub(crate) fn classify_sql_type(
    dialect: SqlDialect,
    raw: &str,
    enum_values: Option<(Option<String>, Vec<String>)>,
) -> DbColumnType {
    if let Some((type_name, values)) = enum_values {
        return DbColumnType::Enum { type_name, values };
    }

    let clean = raw.trim();
    let lower = clean.to_ascii_lowercase();
    let base = lower.split(['(', ' ']).next().unwrap_or("").trim();

    // MySQL inline ENUM('a','b')
    if dialect == SqlDialect::Mysql
        && lower.starts_with("enum(")
        && let Some(vals) = parse_mysql_enum_values(clean)
    {
        return DbColumnType::Enum {
            type_name: None,
            values: vals,
        };
    }

    // Booleans
    if matches!(base, "bool" | "boolean")
        || (dialect == SqlDialect::Mysql && lower.starts_with("tinyint(1)"))
        || (dialect == SqlDialect::Sqlite && lower.contains("bool"))
    {
        return DbColumnType::Boolean;
    }

    // Integers
    match integer_width(dialect, &lower, base) {
        IntWidth::NotInt => {}
        IntWidth::Unsized => {
            return DbColumnType::Integer {
                raw: clean.to_owned(),
                bits: None,
            };
        }
        IntWidth::Bits(bits) => {
            return DbColumnType::Integer {
                raw: clean.to_owned(),
                bits: Some(bits),
            };
        }
    }

    // Strings
    let char_len = extract_paren_u32(&lower);
    if lower.starts_with("character varying")
        || lower.starts_with("character(")
        || lower == "character"
        || matches!(
            base,
            "varchar" | "nvarchar" | "char" | "nchar" | "bpchar" | "varying" | "varchar2"
        )
    {
        return DbColumnType::String {
            raw: clean.to_owned(),
            char_max_len: char_len,
            is_char_type: true,
        };
    }

    if matches!(
        base,
        "text" | "tinytext" | "mediumtext" | "longtext" | "clob" | "string" | "citext"
    ) {
        return DbColumnType::String {
            raw: clean.to_owned(),
            char_max_len: None,
            is_char_type: false,
        };
    }

    DbColumnType::Other {
        raw: clean.to_owned(),
    }
}

enum IntWidth {
    NotInt,
    Unsized,
    Bits(u8),
}

fn integer_width(dialect: SqlDialect, lower: &str, base: &str) -> IntWidth {
    let bits = match base {
        "tinyint" => Some(8),
        "smallint" | "int2" | "smallserial" => Some(16),
        "mediumint" => Some(24),
        "int" | "integer" | "int4" | "serial" | "signed" => Some(32),
        "bigint" | "int8" | "bigserial" => Some(64),
        _ if lower.contains("int") && dialect == SqlDialect::Sqlite => None,
        _ => return IntWidth::NotInt,
    };
    if dialect == SqlDialect::Sqlite {
        IntWidth::Unsized
    } else {
        bits.map_or(IntWidth::Unsized, IntWidth::Bits)
    }
}

fn extract_paren_u32(lower: &str) -> Option<u32> {
    let open = lower.find('(')?;
    let close = lower[open + 1..].find(')')?;
    let inner = lower[open + 1..open + 1 + close].trim();
    // Ignore precision,scale pairs like decimal(10,2).
    if inner.contains(',') {
        return None;
    }

    inner.parse::<u32>().ok()
}

/// Parses MySQL `enum('a','b','it''s')` into literal values.
pub(crate) fn parse_mysql_enum_values(raw: &str) -> Option<Vec<String>> {
    let start = raw.find('(')?;
    let end = raw.rfind(')')?;
    if end <= start {
        return None;
    }

    let inner = &raw[start + 1..end];
    let mut vals = Vec::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            continue;
        }

        let mut val = String::new();
        while let Some(ch) = chars.next() {
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    val.push('\'');
                } else {
                    break;
                }
            } else if ch == '\\' {
                if let Some(esc) = chars.next() {
                    val.push(esc);
                }
            } else {
                val.push(ch);
            }
        }
        vals.push(val);
    }
    if vals.is_empty() { None } else { Some(vals) }
}
