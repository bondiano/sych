//! Naming conversions shared by static ORM extractors: acronym-aware `snake_case`,
//! English pluralization and `ModelNamingOverrides` application.

use std::collections::BTreeMap;

use sych_core::db_schema::ModelNamingOverrides;

/// Converts `PascalCase` or `camelCase` identifiers to `snake_case`, handling
/// common acronyms (`UserID` -> `user_id`, `HTTPServer` -> `http_server`).
pub fn to_snake_case(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c == '-' || c == ' ' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }

            continue;
        }

        if c.is_ascii_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
                if (prev.is_ascii_lowercase()
                    || prev.is_ascii_digit()
                    || (prev.is_ascii_uppercase() && next_lower))
                    && !out.ends_with('_')
                {
                    out.push('_');
                }
            }

            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Converts a singular English table stem to its conventional plural form,
/// preserving the original letter casing of `word`.
pub fn pluralize(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }

    if let Some((prefix, last)) = word.rsplit_once('_') {
        return format!("{prefix}_{}", pluralize_single(last));
    }

    if let Some((prefix, last)) = split_last_pascal_word(word) {
        return format!("{prefix}{}", pluralize_single(last));
    }

    pluralize_single(word)
}

fn split_last_pascal_word(word: &str) -> Option<(&str, &str)> {
    let bytes = word.as_bytes();
    for i in (1..bytes.len()).rev() {
        if bytes[i].is_ascii_uppercase() && bytes[i - 1].is_ascii_lowercase() {
            return Some((&word[..i], &word[i..]));
        }
    }
    None
}

fn match_word_case(original: &str, replacement: &str) -> String {
    if original.chars().all(|c| !c.is_ascii_lowercase())
        && original.chars().any(|c| c.is_ascii_uppercase())
    {
        return replacement.to_ascii_uppercase();
    }

    if original
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
    {
        let mut chars = replacement.chars();
        if let Some(first) = chars.next() {
            return format!("{}{}", first.to_ascii_uppercase(), chars.as_str());
        }
    }

    replacement.to_owned()
}

fn pluralize_single(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    let irregular = match lower.as_str() {
        "person" => Some("people"),
        "child" => Some("children"),
        "man" => Some("men"),
        "woman" => Some("women"),
        "status" => Some("statuses"),
        "campus" => Some("campuses"),
        "bus" => Some("buses"),
        "alias" => Some("aliases"),
        "index" => Some("indices"),
        "data" | "metadata" | "news" | "series" | "species" | "equipment" | "information" => {
            return word.to_owned();
        }
        _ => None,
    };
    if let Some(repl) = irregular {
        return match_word_case(word, repl);
    }

    let suffix_upper = word.chars().all(|c| !c.is_ascii_lowercase())
        && word.chars().any(|c| c.is_ascii_uppercase());
    if lower.ends_with("ss")
        || lower.ends_with("sh")
        || lower.ends_with("ch")
        || lower.ends_with('x')
        || lower.ends_with('z')
    {
        return format!("{word}{}", if suffix_upper { "ES" } else { "es" });
    }

    if lower.ends_with('y')
        && lower[..lower.len() - 1]
            .chars()
            .last()
            .is_some_and(|c| !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u'))
    {
        let stem = &word[..word.len() - 1];
        return format!("{stem}{}", if suffix_upper { "IES" } else { "ies" });
    }

    if lower.ends_with('s') {
        return word.to_owned();
    }

    format!("{word}{}", if suffix_upper { "S" } else { "s" })
}

/// Applies explicit table overrides and naming strategy to a model/struct name.
pub fn default_table_name(
    model_name: &str,
    pkg_name: Option<&str>,
    overrides: &ModelNamingOverrides,
    explicit_tables: &BTreeMap<String, String>,
) -> String {
    if let Some(pkg) = pkg_name
        && let Some(mapped) = explicit_tables.get(&format!("{pkg}.{model_name}"))
    {
        return mapped.clone();
    }

    if let Some(mapped) = explicit_tables.get(model_name) {
        return mapped.clone();
    }

    let base = if overrides.no_lower_case {
        model_name.to_owned()
    } else {
        to_snake_case(model_name)
    };
    let stem = if overrides.singular_table {
        base
    } else {
        pluralize(&base)
    };
    if overrides.table_prefix.is_empty() {
        stem
    } else {
        format!("{}{stem}", overrides.table_prefix)
    }
}

/// Converts a field name into a column name according to `ModelNamingOverrides`.
pub fn default_column_name(field_name: &str, overrides: &ModelNamingOverrides) -> String {
    if overrides.no_lower_case {
        field_name.to_owned()
    } else {
        to_snake_case(field_name)
    }
}
