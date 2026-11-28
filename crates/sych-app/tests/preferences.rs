//! End-to-end model passes for `RuleKind::Preference`: unit-matched `pref-N` routing,
//! rejection of fabricated `pref-N`, and `LOW` + `PREFERENCE` confirmation with `rule_id`.

mod harness;

use serde_json::json;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};
use sych_core::{FindingCategory, Severity, Support};

use harness::*;

fn preference_script(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    match request.schema_name.as_str() {
        "discovery" => {
            if user.contains("review unit>>>\nhigh-risk obligation audit")
                || user.contains("review unit>>>\nlib/format.ts")
            {
                assert!(
                    !user.contains("# Preferences of this change"),
                    "audit and unmatched units omit preferences: {user}"
                );
                return supported_and_empty(request);
            }

            assert!(
                user.contains("# Preferences of this change")
                    && user.contains("- pref-1: preference API-PREF v1"),
                "matched unit receives pref-1: {user}"
            );
            let (id, line) = first_ref(request, "HEAD");
            let anchor = json!([{"ref_id": id, "start_line": line, "end_line": line}]);
            Ok(reply(
                &json!({
                    "candidates": [
                        {
                            "category": "NIT",
                            "claim": "name total after the domain noun",
                            "conditions": ["reading the module API"],
                            "causal_chain": ["exported name uses generic verb"],
                            "consequence": "inconsistent with glossary",
                            "anchors": anchor,
                            "evidence_ids": [],
                            "obligation_ids": [],
                            "preference_ids": ["pref-1"],
                            "proposed_severity": "LOW",
                            "disproof_question": "is total already the glossary term?"
                        },
                        {
                            "category": "NIT",
                            "claim": "invented preference",
                            "conditions": ["always"],
                            "causal_chain": ["none"],
                            "consequence": "none",
                            "anchors": anchor,
                            "evidence_ids": [],
                            "obligation_ids": [],
                            "preference_ids": ["pref-99"],
                            "proposed_severity": "LOW",
                            "disproof_question": "none"
                        }
                    ],
                    "context_requests": [],
                    "unresolved_questions": []
                })
                .to_string(),
            ))
        }
        "candidate_verdicts" => {
            assert!(
                user.contains("\"preference_ids\":[\"pref-1\"]")
                    && user.contains("# Preferences of this change"),
                "verifier sees pref-1 on the candidate and in the task: {user}"
            );
            let (id, line) = first_ref(request, "HEAD");
            Ok(reply(
                &json!({
                    "verdicts": [{
                        "candidate_id": "cand-1",
                        "verdict": "CONFIRMED",
                        "category": "NIT",
                        "severity": "LOW",
                        "support": "PREFERENCE",
                        "title": "Use the domain noun for the helper",
                        "explanation": "Matches API-PREF in src/.",
                        "anchors": [{"ref_id": id, "start_line": line, "end_line": line}],
                        "evidence_ids": [],
                        "alternatives_checked": ["glossary"],
                        "suggested_action": "Rename to orderTotal.",
                        "validation_expectation": "Exports orderTotal.",
                        "limitations": [],
                        "suggested_change": null,
                        "unverified_external_assumptions": []
                    }],
                    "context_requests": []
                })
                .to_string(),
            ))
        }
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn preference_rules_flow_only_to_matched_units_and_confirm_low_preference_findings() {
    let pref_rules = format!(
        "{}\n  - id: API-PREF\n    version: 1\n    title: Domain helper names\n    kind: preference\n    required: false\n    owner: api\n    applies_to:\n      paths: [\"src/**\"]\n    statement: Exported helpers in src/ use domain nouns.\n",
        RULES.replace("[\"src/**\"]", "[\"src/**\", \"lib/**\"]")
    );
    let (outcome, _) = run_on(
        repo_with(true, &[(".sych/rules.yaml", &pref_rules)]),
        preference_script,
        Budgets::default(),
        1,
        true,
    )
    .await;
    assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
    let finding = &outcome.findings[0];
    assert_eq!(finding.rule_id.as_deref(), Some("API-PREF"));
    assert_eq!(finding.category, FindingCategory::Nit);
    assert_eq!(finding.severity, Severity::Low);
    assert_eq!(finding.support, Support::Preference);
    assert!(
        outcome
            .record
            .discovered
            .iter()
            .flat_map(|d| &d.rejected)
            .any(|r| r.problems == ["unknown preference id `pref-99`"]),
        "{:?}",
        outcome.record.discovered
    );
}
