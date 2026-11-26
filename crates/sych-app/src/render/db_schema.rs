//! Database and application schema checks in a Markdown review report.

use std::fmt::Write;

use sych_core::db_schema::DbSchemaFacts;

use super::{md, tag};

/// Each checked database schema target: dialect, source, diff-base status,
/// findings (NEW/SAME/uncompared), fixed items, stale exclusions, warnings,
/// skipped rules and limitations.
pub(super) fn db_schema(out: &mut String, facts: &[DbSchemaFacts]) {
    if facts.is_empty() {
        return;
    }

    out.push_str("\n## Database schema\n\n");
    for target in facts {
        let model = target
            .model_extractor
            .as_deref()
            .map_or_else(String::new, |m| format!(", model {}", md(m)));
        let _ = writeln!(
            out,
            "- {} ({}, {}{model}): {}, diff base {}, by {} {}",
            md(&target.target),
            target.dialect,
            tag(&target.source),
            tag(&target.status),
            if target.base_checked {
                "checked"
            } else {
                "not checked"
            },
            md(&target.producer.name),
            md(&target.producer.version)
        );
        for f in &target.findings {
            let change = f.change.map_or_else(|| "FINDING".to_owned(), |c| tag(&c));
            let at = f
                .code_ref
                .as_ref()
                .map(|r| format!(" at {}:{}", md(r.path.as_str()), r.start_line))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  - {change} {} ({}): {}{at}",
                md(&f.rule),
                md(&f.object.display_path()),
                md(&f.message)
            );
        }
        for f in &target.fixed {
            let _ = writeln!(
                out,
                "  - FIXED {} ({}): {}",
                md(&f.rule),
                md(&f.object.display_path()),
                md(&f.message)
            );
        }
        for stale in &target.unmatched_exclusions {
            let _ = writeln!(
                out,
                "  - STALE EXCLUSION {} on {}: {}",
                md(&stale.rule),
                md(&stale.object),
                md(&stale.reason)
            );
        }
        for w in &target.warnings {
            let _ = writeln!(out, "  - warning: {}", md(w));
        }
        if !target.skipped_rules.is_empty() {
            let _ = writeln!(
                out,
                "  - skipped rules: {}",
                md(&target.skipped_rules.join(", "))
            );
        }

        for l in &target.limitations {
            let _ = writeln!(out, "  - limitation: {}", md(l));
        }
    }
}
