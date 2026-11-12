//! SARIF 2.1.0 of a saved review for code scanning and editors. Each open or dismissed finding
//! is a result at its first HEAD anchor (one only on deleted lines has no location), keyed by
//! Sych's fingerprint. Partial coverage, UNKNOWN obligations, unreviewed paths and open
//! questions are notifications of the invocation: an absent result is not a clean review.

use serde_json::{Value, json};
use sych_core::{
    Coverage, Finding, FindingCategory, Lifecycle, ObligationStatus, ReviewResult, Severity,
    SourceSide,
};

const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

fn category_rule(c: FindingCategory) -> &'static str {
    match c {
        FindingCategory::Defect => "sych/defect",
        FindingCategory::RuleViolation => "sych/rule-violation",
        FindingCategory::Improvement => "sych/improvement",
        FindingCategory::Nit => "sych/nit",
    }
}

fn rule_id(f: &Finding) -> &str {
    f.rule_id
        .as_deref()
        .unwrap_or_else(|| category_rule(f.category))
}

fn level(s: Severity) -> &'static str {
    match s {
        Severity::Critical | Severity::High => "error",
        Severity::Medium => "warning",
        Severity::Low | Severity::Info => "note",
    }
}

fn result(f: &Finding) -> Value {
    let text = if f.explanation.is_empty() {
        f.title.clone()
    } else {
        format!("{}\n\n{}", f.title, f.explanation)
    };
    let mut r = json!({
        "ruleId": rule_id(f),
        "level": level(f.severity),
        "message": { "text": text },
        "partialFingerprints": { "sych/v1": f.fingerprint.hex() },
        "properties": {
            "findingId": f.id,
            "category": f.category,
            "severity": f.severity,
            "support": f.support,
        },
    });
    if let Some(a) = f.anchors.iter().find(|a| a.side == SourceSide::Head) {
        r["locations"] = json!([{ "physicalLocation": {
            "artifactLocation": { "uri": a.path.as_str() },
            "region": { "startLine": a.start_line, "endLine": a.end_line },
        }}]);
    }

    if f.lifecycle == Lifecycle::Dismissed {
        r["suppressions"] = json!([{ "kind": "external", "justification": "dismissed in Sych" }]);
    }

    r
}

fn notification(level: &str, text: &str) -> Value {
    json!({ "level": level, "message": { "text": text } })
}

/// What the review did not decide, for the invocation.
fn notifications(r: &ReviewResult) -> Vec<Value> {
    let mut out = Vec::new();
    if r.coverage != Coverage::CompleteForScope {
        out.push(notification(
            "warning",
            &format!(
                "coverage {:?}: part of the scope was not reviewed, so a missing result is not \
                 evidence of no problem",
                r.coverage
            ),
        ));
    }

    for o in &r.obligations {
        if let ObligationStatus::Unknown { reason_code, .. } = &o.status {
            out.push(notification(
                "warning",
                &format!("{} UNKNOWN ({reason_code}): {}", o.rule_id, o.question),
            ));
        }
    }
    for gap in &r.excluded_or_unreviewed {
        out.push(notification(
            "note",
            &format!("{} not reviewed: {}", gap.path, gap.reason),
        ));
    }
    for q in &r.unresolved_questions {
        out.push(notification("note", q));
    }
    out
}

pub fn sarif(r: &ReviewResult) -> Value {
    let findings: Vec<&Finding> = r
        .findings
        .iter()
        .filter(|f| matches!(f.lifecycle, Lifecycle::Open | Lifecycle::Dismissed))
        .collect();
    let mut rules: Vec<Value> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for f in &findings {
        let id = rule_id(f);
        if ids.contains(&id) {
            continue;
        }

        let description = r.obligations.iter().find(|o| o.rule_id == id).map_or_else(
            || format!("{:?} finding", f.category),
            |o| o.question.clone(),
        );
        ids.push(id);
        rules.push(json!({ "id": id, "shortDescription": { "text": description } }));
    }
    json!({
        "$schema": SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": "sych",
                "version": env!("CARGO_PKG_VERSION"),
                "informationUri": "https://github.com/bondiano/sych",
                "rules": rules,
            }},
            "invocations": [{
                "executionSuccessful": true,
                "toolExecutionNotifications": notifications(r),
            }],
            "results": findings.iter().map(|f| result(f)).collect::<Vec<_>>(),
            "properties": {
                "runId": r.run_id,
                "recommendation": r.recommendation,
                "coverage": r.coverage,
            },
        }],
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn fixture() -> ReviewResult {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json");
        serde_json::from_str(&fs::read_to_string(path).expect("fixture")).expect("result")
    }

    #[test]
    fn a_finding_is_a_result_at_its_head_anchor_keyed_by_its_fingerprint() {
        let r = fixture();
        let doc = sarif(&r);
        let run = &doc["runs"][0];
        assert_eq!(doc["version"], "2.1.0");
        let result = &run["results"][0];
        assert_eq!(result["ruleId"], "EFFECT-01");
        assert_eq!(result["level"], "error");
        assert_eq!(
            result["partialFingerprints"]["sych/v1"],
            r.findings[0].fingerprint.hex()
        );
        let at = &result["locations"][0]["physicalLocation"];
        assert_eq!(at["artifactLocation"]["uri"], "backend/orders/create.go");
        assert_eq!(at["region"]["startLine"], 41);
        assert_eq!(run["tool"]["driver"]["rules"][0]["id"], "EFFECT-01");
    }

    #[test]
    fn what_was_not_decided_is_a_notification_not_silence() {
        let notes: Vec<String> =
            sarif(&fixture())["runs"][0]["invocations"][0]["toolExecutionNotifications"]
                .as_array()
                .expect("notifications")
                .iter()
                .map(|n| n["message"]["text"].as_str().expect("text").to_owned())
                .collect();
        assert!(notes[0].starts_with("coverage Partial"), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("EFFECT-02 UNKNOWN")));
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("backend/gen/orders.pb.go not reviewed"))
        );
        assert!(notes.iter().any(|n| n.contains("event ordering")));
    }

    #[test]
    fn resolved_findings_are_left_out_and_dismissed_ones_suppressed() {
        let mut r = fixture();
        let mut dismissed = r.findings[0].clone();
        dismissed.lifecycle = Lifecycle::Dismissed;
        r.findings[0].lifecycle = Lifecycle::Resolved;
        r.findings.push(dismissed);
        let results = sarif(&r)["runs"][0]["results"].clone();
        assert_eq!(results.as_array().map(Vec::len), Some(1));
        assert_eq!(results[0]["suppressions"][0]["kind"], "external");
    }
}
