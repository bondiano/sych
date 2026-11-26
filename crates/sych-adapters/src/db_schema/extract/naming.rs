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

/// Converts a singular English table stem to its conventional plural form.
pub fn pluralize(word: &str) -> String {
    if word.is_empty() {
        return String::new();
    }

    if let Some((prefix, last)) = word.rsplit_once('_') {
        return format!("{prefix}_{}", pluralize_single(last));
    }

    pluralize_single(word)
}

fn pluralize_single(word: &str) -> String {
    let lower = word.to_ascii_lowercase();
    match lower.as_str() {
        "person" => return "people".into(),
        "child" => return "children".into(),
        "man" => return "men".into(),
        "woman" => return "women".into(),
        "status" => return "statuses".into(),
        "campus" => return "campuses".into(),
        "bus" => return "buses".into(),
        "alias" => return "aliases".into(),
        "index" => return "indices".into(),
        "data" | "metadata" | "news" | "series" | "species" | "equipment" | "information" => {
            return lower;
        }
        _ => {}
    }

    if lower.ends_with("ss")
        || lower.ends_with("sh")
        || lower.ends_with("ch")
        || lower.ends_with('x')
        || lower.ends_with('z')
    {
        return format!("{lower}es");
    }

    if let Some(stem) = lower.strip_suffix('y')
        && stem
            .chars()
            .last()
            .is_some_and(|c| !matches!(c, 'a' | 'e' | 'i' | 'o' | 'u'))
    {
        return format!("{stem}ies");
    }

    if lower.ends_with('s') {
        return lower;
    }

    format!("{lower}s")
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
