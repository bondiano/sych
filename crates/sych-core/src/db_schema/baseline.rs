//! Idempotent baseline generation for schema linter exclusions, preserving
//! existing YAML comments, reasons and formatting.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use super::config::{DbSchemaConfig, DbSchemaExclusion};
use super::model::DbSchemaFinding;
use crate::config::ConfigError;

/// Reason recorded on every exclusion created by `sych schema baseline`.
pub const BASELINE_REASON: &str = "baseline";

/// Appends baseline exclusions for every finding not already silenced by
/// `existing_yaml`.
///
/// Existing exclusions, reasons and comments survive untouched. Running this
/// function twice on the same findings adds nothing and returns
/// `(existing_yaml, 0)`.
pub fn add_baseline(
    origin: &str,
    existing_yaml: &str,
    findings: Vec<DbSchemaFinding>,
) -> Result<(String, usize), Vec<ConfigError>> {
    let cfg = DbSchemaConfig::parse_yaml(origin, existing_yaml)?;
    let (kept, _) = cfg.apply(findings);
    let mut seen = BTreeSet::new();
    let mut added = Vec::new();
    for finding in kept {
        let exclusion = DbSchemaExclusion {
            rule: finding.rule,
            object: finding.object.format_exclusion(),
            reason: BASELINE_REASON.to_owned(),
        };
        if seen.insert(exclusion.clone()) {
            added.push(exclusion);
        }
    }

    if added.is_empty() {
        return Ok((existing_yaml.to_owned(), 0));
    }

    let updated = append_exclusions_preserving_yaml(existing_yaml, &added);
    // Guarantee the rewritten document remains strictly valid YAML.
    DbSchemaConfig::parse_yaml(origin, &updated)?;
    Ok((updated, added.len()))
}

fn yaml_scalar(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.starts_with([
            ' ', '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"',
            '%', '@', '`',
        ])
        || value.ends_with(' ')
        || value.contains(": ")
        || value.contains(" #")
        || value.contains(['\n', '\r', '\t'])
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "null" | "~" | "true" | "false" | "yes" | "no" | "on" | "off"
        );
    if !needs_quotes {
        return value.to_owned();
    }

    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn render_entries(entries: &[DbSchemaExclusion], indent: &str) -> String {
    let mut out = String::new();
    for e in entries {
        let _ = writeln!(out, "{indent}- rule: {}", yaml_scalar(&e.rule));
        let _ = writeln!(out, "{indent}  object: {}", yaml_scalar(&e.object));
        let _ = writeln!(out, "{indent}  reason: {}", yaml_scalar(&e.reason));
    }
    out
}

fn append_exclusions_preserving_yaml(existing: &str, added: &[DbSchemaExclusion]) -> String {
    if existing.trim().is_empty() {
        let mut out = existing.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }

        out.push_str("exclusions:\n");
        out.push_str(&render_entries(added, "  "));
        return out;
    }

    let lines: Vec<&str> = existing.lines().collect();
    let key_idx = lines.iter().position(|line| {
        let trimmed = line.trim_end();
        !line.starts_with([' ', '\t'])
            && (trimmed == "exclusions:"
                || trimmed.starts_with("exclusions: []")
                || trimmed.starts_with("exclusions: null")
                || trimmed.starts_with("exclusions: #"))
    });

    let Some(idx) = key_idx else {
        let mut out = existing.to_owned();
        if !out.ends_with('\n') {
            out.push('\n');
        }

        out.push_str("exclusions:\n");
        out.push_str(&render_entries(added, "  "));
        return out;
    };

    // Find the end of the top-level `exclusions:` block and detect item indentation.
    let mut insert_at = idx + 1;
    let mut item_indent = "  ".to_owned();
    let mut saw_item = false;
    while insert_at < lines.len() {
        let line = lines[insert_at];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            insert_at += 1;
            continue;
        }

        let leading_spaces = line.len() - line.trim_start_matches(' ').len();
        if leading_spaces == 0 && !trimmed.starts_with('-') {
            break;
        }

        if !saw_item && trimmed.starts_with('-') {
            item_indent = " ".repeat(leading_spaces);
            saw_item = true;
        }

        insert_at += 1;
    }

    // Pull back past trailing blank lines so new items sit directly under the list.
    while insert_at > idx + 1 && lines[insert_at - 1].trim().is_empty() {
        insert_at -= 1;
    }

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i == idx {
            let trimmed = line.trim_end();
            if let Some(rest) = trimmed.strip_prefix("exclusions: []") {
                out.push_str("exclusions:");
                out.push_str(rest);
                out.push('\n');
            } else if let Some(rest) = trimmed.strip_prefix("exclusions: null") {
                out.push_str("exclusions:");
                out.push_str(rest);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }

        if i + 1 == insert_at {
            out.push_str(&render_entries(added, &item_indent));
        }
    }

    out
}
