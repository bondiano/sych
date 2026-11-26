//! Normalization and analysis of SQL `CHECK` constraint expressions across
//! PostgreSQL, MySQL and SQLite renderings.

/// Distinct sorted names for order-insensitive comparison.
pub(super) fn name_set(names: &[String]) -> Vec<String> {
    let mut out = names.to_vec();
    out.sort();
    out.dedup();
    out
}

/// Renders names as `"a", "b"`.
pub(super) fn quote_list(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders a value set as `("a", "b")`.
pub(super) fn describe_set(vals: &[String]) -> String {
    format!("({})", quote_list(&name_set(vals)))
}

const PG_CASTS: &[&str] = &[
    "character varying[]",
    "character varying",
    "character[]",
    "character",
    "bpchar[]",
    "bpchar",
    "varchar[]",
    "varchar",
    "citext[]",
    "citext",
    "text[]",
    "text",
    "name[]",
    "name",
    "integer[]",
    "integer",
    "bigint[]",
    "bigint",
    "smallint[]",
    "smallint",
    "int4[]",
    "int4",
    "int8[]",
    "int8",
];

/// Flattens a `CHECK` expression so engines' different renderings compare equal:
/// string literals become `?` (values returned in order), PostgreSQL casts,
/// MySQL charset prefixes (`_utf8mb4'...'`), quoting and parentheses disappear,
/// and the rest is lower-cased and single-space separated.
pub(super) fn skeleton(expr: &str) -> (String, Vec<String>) {
    let bytes = expr.as_bytes();
    let mut vals = Vec::new();
    let mut step1 = String::with_capacity(expr.len());
    let mut i = 0;

    while i < bytes.len() {
        // String literal `'...'` with `''` escape.
        if bytes[i] == b'\'' {
            let mut val = String::new();
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        val.push('\'');
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    let ch = expr[i..].chars().next().unwrap_or('\0');
                    val.push(ch);
                    i += ch.len_utf8();
                }
            }
            // Strip a preceding MySQL charset prefix like `_utf8mb4`.
            strip_mysql_charset_prefix(&mut step1);
            vals.push(val);
            step1.push('?');
            continue;
        }

        // PostgreSQL cast `::<known_type>`.
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            let after = &expr[i + 2..];
            let lower_after = after.to_ascii_lowercase();
            if let Some(cast) = PG_CASTS.iter().find(|c| {
                lower_after.starts_with(**c)
                    && !after
                        .as_bytes()
                        .get(c.len())
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            }) {
                i += 2 + cast.len();
                continue;
            }
        }

        let ch = expr[i..].chars().next().unwrap_or('\0');
        step1.push(ch);
        i += ch.len_utf8();
    }

    let mut normalized = String::with_capacity(step1.len());
    let mut prev_space = true;
    for ch in step1.chars() {
        match ch {
            '"' | '`' => {}
            '(' | ')' | '[' | ']' | ',' => {
                if !prev_space {
                    normalized.push(' ');
                    prev_space = true;
                }
            }
            c if c.is_whitespace() => {
                if !prev_space {
                    normalized.push(' ');
                    prev_space = true;
                }
            }
            c => {
                normalized.push(c.to_ascii_lowercase());
                prev_space = false;
            }
        }
    }

    (normalized.trim().to_owned(), vals)
}

fn strip_mysql_charset_prefix(buf: &mut String) {
    let bytes = buf.as_bytes();
    let mut end = bytes.len();
    if end == 0 {
        return;
    }

    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    let token = &buf[start..end];
    if token.len() >= 2
        && token.starts_with('_')
        && token[1..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        end = start;
        buf.truncate(end);
    }
}

fn strip_null_escape<'a>(sk: &'a str, col_lower: &str) -> &'a str {
    let prefix = format!("{col_lower} is null or ");
    sk.strip_prefix(&prefix).unwrap_or(sk)
}

/// Returns the tightest length limit a `CHECK` expression puts on `col`
/// (supporting `char_length`, `character_length`, `length` with `<=` or `<`).
pub(super) fn check_max_length(expr: &str, col: &str) -> Option<u32> {
    let (sk, _) = skeleton(expr);
    let col_lower = col.to_ascii_lowercase();
    let sk = strip_null_escape(&sk, &col_lower);
    if sk.contains(" or ") {
        return None;
    }

    let tokens: Vec<&str> = sk.split_whitespace().collect();
    let mut best: Option<u32> = None;
    for window in tokens.windows(4) {
        let [func, ident, op, raw_n] = window else {
            continue;
        };
        if !matches!(*func, "char_length" | "character_length" | "length") || *ident != col_lower {
            continue;
        }

        let Ok(mut n) = raw_n.parse::<u32>() else {
            continue;
        };
        match *op {
            "<=" => {}
            "<" => {
                n = n.saturating_sub(1);
            }
            _ => continue,
        }
        best = Some(best.map_or(n, |cur| cur.min(n)));
    }

    best
}

/// Returns the allowed value list when the whole `CHECK` expression restricts
/// `col` to a list of string literals (`IN (...)`, `= ANY (ARRAY[...])` or
/// `col = 'a' OR col = 'b'`).
pub(super) fn check_enum_values(expr: &str, col: &str) -> Option<Vec<String>> {
    let (sk, vals) = skeleton(expr);
    if vals.is_empty() {
        return None;
    }

    let col_lower = col.to_ascii_lowercase();
    let sk = strip_null_escape(&sk, &col_lower);
    let tokens: Vec<&str> = sk.split_whitespace().collect();

    if matches_list_form(&tokens, &col_lower) || matches_or_form(&tokens, &col_lower) {
        Some(vals)
    } else {
        None
    }
}

fn matches_list_form(tokens: &[&str], col: &str) -> bool {
    if tokens.first().copied() != Some(col) {
        return false;
    }

    let Some(["in", rest @ ..] | ["=", "any", "array", rest @ ..] | ["=", "any", rest @ ..]) =
        tokens.get(1..)
    else {
        return false;
    };
    !rest.is_empty() && rest.iter().all(|t| *t == "?")
}

fn matches_or_form(tokens: &[&str], col: &str) -> bool {
    // Sequence of `col = ?` separated by `or`: length is 4*k - 1 (3, 7, 11, ...).
    if tokens.len() < 3 || !(tokens.len() + 1).is_multiple_of(4) {
        return false;
    }

    let mut i = 0;
    while i < tokens.len() {
        if tokens.get(i..i + 3) != Some(&[col, "=", "?"]) {
            return false;
        }

        i += 3;
        if i < tokens.len() {
            if tokens[i] != "or" {
                return false;
            }

            i += 1;
        }
    }

    true
}
