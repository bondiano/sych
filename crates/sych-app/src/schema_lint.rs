//! Rendering helpers for standalone `sych schema` reports (text, Markdown,
//! SARIF 2.1.0, rule catalogue and rule explanations).

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{Value, json};
use sych_core::Severity;
use sych_core::db_schema::rules::{ALL_RULES, RuleSpec, lookup, review_rule_id};
use sych_core::db_schema::{DbSchemaReport, DbSchemaRuleInfo, STALE_EXCLUSION_RULE};

use crate::render::md;

const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

/// Renders a standalone [`DbSchemaReport`] as clean, terminal-safe plain text.
pub fn render_text(report: &DbSchemaReport) -> String {
    let mut out = String::new();
    for f in &report.findings {
        let loc = f
            .source
            .as_ref()
            .map(|p| format!(" ({}:{})", p.file, p.line))
            .unwrap_or_default();
        let _ = writeln!(out, "[{}] {}{loc}", f.rule, f.object.display_path());
        let _ = writeln!(out, "  {}", f.message);
        let _ = writeln!(out, "  fix: {}", f.fix);
        let _ = writeln!(out, "  exclude: {}", f.object.format_exclusion());
    }

    for stale in &report.unmatched_exclusions {
        let _ = writeln!(out, "[stale-exclusion] {} on {}", stale.rule, stale.object);
        let _ = writeln!(
            out,
            "  exclusion matched no finding (reason: {})",
            stale.reason
        );
    }

    for warn in &report.warnings {
        let _ = writeln!(out, "warning: {warn}");
    }
    for lim in &report.limitations {
        let _ = writeln!(out, "limitation: {lim}");
    }
    if !report.skipped_rules.is_empty() {
        let _ = writeln!(
            out,
            "skipped ({}): {}",
            report.dialect,
            report.skipped_rules.join(", ")
        );
    }

    if report.passed() {
        let _ = writeln!(out, "OK ({}): 0 findings", report.dialect);
    } else {
        let _ = writeln!(
            out,
            "FAILED ({}): {} finding(s), {} stale exclusion(s)",
            report.dialect,
            report.findings.len(),
            report.unmatched_exclusions.len()
        );
    }

    out
}

/// Renders a standalone [`DbSchemaReport`] as GitHub-flavoured Markdown with escaped untrusted text.
pub fn render_markdown(report: &DbSchemaReport) -> String {
    let mut out = format!("# Database schema check ({})\n\n", report.dialect);
    if report.passed() {
        out.push_str("Status: **PASS** (0 findings, 0 stale exclusions)\n");
    } else {
        let _ = writeln!(
            out,
            "Status: **FAIL** ({} finding(s), {} stale exclusion(s))",
            report.findings.len(),
            report.unmatched_exclusions.len()
        );
    }

    if !report.findings.is_empty() {
        out.push_str("\n## Findings\n\n");
        for f in &report.findings {
            let at = f
                .source
                .as_ref()
                .map(|p| format!(" at `{}:{}`", md(&p.file), p.line))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- **{}** `{}`{at}: {}",
                md(&f.rule),
                md(&f.object.display_path()),
                md(&f.message)
            );
            let _ = writeln!(out, "  - fix: {}", md(&f.fix));
        }
    }

    if !report.unmatched_exclusions.is_empty() {
        out.push_str("\n## Stale exclusions\n\n");
        for stale in &report.unmatched_exclusions {
            let _ = writeln!(
                out,
                "- `{}` on `{}` (reason: {})",
                md(&stale.rule),
                md(&stale.object),
                md(&stale.reason)
            );
        }
    }

    if !report.warnings.is_empty() || !report.limitations.is_empty() {
        out.push_str("\n## Warnings and limitations\n\n");
        for w in &report.warnings {
            let _ = writeln!(out, "- warning: {}", md(w));
        }
        for l in &report.limitations {
            let _ = writeln!(out, "- limitation: {}", md(l));
        }
    }

    if !report.skipped_rules.is_empty() {
        let _ = writeln!(
            out,
            "\nSkipped for {}: {}",
            report.dialect,
            report.skipped_rules.join(", ")
        );
    }

    out
}

fn sarif_level(sev: Severity) -> &'static str {
    match sev {
        Severity::Critical | Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Low | Severity::Info => "note",
    }
}

/// Renders a standalone [`DbSchemaReport`] as SARIF 2.1.0 with logical DB object
/// locations and physical source locations when available.
pub fn render_sarif(report: &DbSchemaReport) -> Value {
    let mut rule_ids: BTreeSet<&str> = BTreeSet::new();
    let mut results = Vec::new();

    for f in &report.findings {
        rule_ids.insert(f.rule.as_str());
        let sev = lookup(&f.rule).map_or(Severity::Medium, |r| r.default_severity);
        let mut loc = json!({
            "logicalLocations": [{
                "name": f.object.display_path(),
                "fullyQualifiedName": f.object.format_exclusion(),
                "kind": f.object.sarif_kind(),
            }]
        });
        if let Some(pos) = &f.source {
            loc["physicalLocation"] = json!({
                "artifactLocation": { "uri": pos.file },
                "region": { "startLine": pos.line.max(1) }
            });
        }

        results.push(json!({
            "ruleId": review_rule_id(&f.rule),
            "level": sarif_level(sev),
            "message": { "text": format!("{}\n\nFix: {}", f.message, f.fix) },
            "locations": [loc],
            "properties": {
                "schemaRule": f.rule,
                "object": f.object.format_exclusion(),
                "dialect": report.dialect,
            }
        }));
    }

    for stale in &report.unmatched_exclusions {
        results.push(json!({
            "ruleId": STALE_EXCLUSION_RULE,
            "level": "warning",
            "message": {
                "text": format!(
                    "Configured exclusion for rule `{}` on `{}` matched no finding (reason: {}).",
                    stale.rule, stale.object, stale.reason
                )
            },
            "locations": [{
                "logicalLocations": [{
                    "name": stale.object,
                    "fullyQualifiedName": stale.object,
                    "kind": "exclusion",
                }]
            }]
        }));
    }

    let (rules_meta, notifications) = sarif_rules_and_notifications(report, &rule_ids);
    json!({
        "$schema": SARIF_SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "sych",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules_meta,
                }
            },
            "results": results,
            "invocations": [{
                "executionSuccessful": true,
                "toolExecutionNotifications": notifications,
            }]
        }]
    })
}

fn sarif_rules_and_notifications(
    report: &DbSchemaReport,
    rule_ids: &BTreeSet<&str>,
) -> (Vec<Value>, Vec<Value>) {
    let mut rules_meta: Vec<Value> = ALL_RULES
        .iter()
        .filter(|r| rule_ids.contains(r.id))
        .map(|r| {
            json!({
                "id": r.review_rule_id(),
                "name": r.id,
                "shortDescription": { "text": r.id },
                "fullDescription": { "text": r.why },
                "help": { "text": format!("{}\n\nFix: {}\n\nValid exceptions: {}", r.why, r.fix, r.exception) },
                "defaultConfiguration": { "level": sarif_level(r.default_severity) },
            })
        })
        .collect();
    if !report.unmatched_exclusions.is_empty() {
        rules_meta.push(json!({
            "id": STALE_EXCLUSION_RULE,
            "name": "stale-exclusion",
            "shortDescription": { "text": "Configured schema exclusion matched no finding" },
            "defaultConfiguration": { "level": "warning" },
        }));
    }

    let mut notifications = Vec::new();
    for w in &report.warnings {
        notifications.push(json!({ "level": "warning", "message": { "text": w } }));
    }
    for l in &report.limitations {
        notifications.push(json!({ "level": "warning", "message": { "text": l } }));
    }
    if !report.skipped_rules.is_empty() {
        notifications.push(json!({
            "level": "note",
            "message": {
                "text": format!(
                    "Rules skipped for {}: {}",
                    report.dialect,
                    report.skipped_rules.join(", ")
                )
            }
        }));
    }

    (rules_meta, notifications)
}

/// Renders the rule catalogue as human-readable text.
pub fn render_rules_text(rules: &[DbSchemaRuleInfo]) -> String {
    let mut out = String::new();
    for r in rules {
        let dialects: Vec<&str> = r.dialects.iter().map(|d| d.as_str()).collect();
        let mut flags = Vec::new();
        if !r.default_enabled {
            flags.push("opt-in");
        }

        if r.requires_model {
            flags.push("requires-model");
        }

        let suffix = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };
        let _ = writeln!(
            out,
            "{:<36} {:<40} ({}){suffix}",
            r.id,
            r.review_rule_id,
            dialects.join(",")
        );
    }
    out
}

/// Renders one [`RuleSpec`] explanation in plain text.
pub fn render_explain_text(spec: &RuleSpec) -> String {
    let dialects: Vec<&str> = spec.dialects.iter().map(|d| d.as_str()).collect();
    let mut out = String::new();
    let _ = writeln!(out, "Rule: {} ({})", spec.id, spec.review_rule_id());
    let _ = writeln!(out, "Dialects: {}", dialects.join(", "));
    let _ = writeln!(
        out,
        "Default: {}",
        if spec.default_off { "off" } else { "on" }
    );
    let _ = writeln!(
        out,
        "Requires model: {}",
        if spec.requires_model { "yes" } else { "no" }
    );
    if let Some(parent) = spec.parent {
        let _ = writeln!(out, "Parent rule: {parent}");
    }

    let _ = writeln!(out, "\nWhy:\n  {}", spec.why);
    let _ = writeln!(out, "\nHow to fix:\n  {}", spec.fix);
    let _ = writeln!(
        out,
        "\nWhen an exclusion is appropriate:\n  {}",
        spec.exception
    );
    out
}
