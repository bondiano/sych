//! A candidate the verifier leaves open without asking is still searched for what refutes it;
//! one about removed lines comes with what else the change removes of the names they use.

use std::fs;

use serde_json::json;
use sych_core::VerificationStatus;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

fn verdict(status: &str) -> String {
    json!({"verdicts": [{"candidate_id": "cand-1", "verdict": status, "category": "DEFECT",
        "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT", "title": "t",
        "explanation": "`?? []` keeps it", "anchors": [], "evidence_ids": [],
        "alternatives_checked": [], "suggested_action": "", "validation_expectation": "",
        "limitations": []}], "context_requests": []})
    .to_string()
}

/// Discovery claims `reduce` throws and asks nothing; the verifier decides only once it sees
/// where `reduce` is used.
fn silent(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let (id, line) = first_ref(request, "HEAD");
            let mut found = json!({
                "candidates": [{"category": "DEFECT", "claim": "`reduce` throws on undefined items",
                    "conditions": ["items undefined"], "causal_chain": ["no guard"], "consequence": "throws",
                    "anchors": [{"ref_id": id, "start_line": line, "end_line": line}], "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM", "disproof_question": "a fallback?"}],
                "context_requests": [], "unresolved_questions": []
            });
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts" if user.contains("answer to: where else is `reduce`") => {
            Ok(reply(&verdict("REJECTED")))
        }
        "candidate_verdicts" => Ok(reply(&verdict("UNRESOLVED"))),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_open_candidate_is_searched_for_its_refutation() {
    let (outcome, calls) = run(silent, Budgets::default()).await;
    assert!(
        calls.iter().filter(|c| *c == "candidate_verdicts").count() >= 2,
        "{calls:?}"
    );
    assert!(!outcome.record.answers.is_empty(), "the engine searched");
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}

const GUARDED: &str = "export function total(order) {\n  if (order.legacy) {\n    throw new Error(\"legacy orders\");\n  }\n  return order.items.reduce(sum, 0);\n}\n";
const UNGUARDED: &str = "export function total(order) {\n  return order.items.reduce(sum, 0);\n}\n";

/// The guard on `order.legacy` is removed together with the `legacy` field in another file.
fn guard_and_field_removed() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    for sub in [".sych", "src", "lib"] {
        fs::create_dir_all(d.join(sub)).expect("dir");
    }
    fs::write(d.join(".sych/rules.yaml"), RULES).expect("rules");
    fs::write(d.join("src/order.ts"), GUARDED).expect("base");
    let model = "export type Order = {\n  items: number[];\n  legacy: boolean;\n};\n";
    fs::write(d.join("lib/model.ts"), model).expect("model");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    fs::write(d.join("src/order.ts"), UNGUARDED).expect("head");
    fs::write(
        d.join("lib/model.ts"),
        model.replace("  legacy: boolean;\n", ""),
    )
    .expect("model");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "head"]);
    dir
}

/// Discovery claims the removed guard is replaced by nothing; the verifier rejects it only when
/// it is shown the anchor's side and the removal of `legacy` in `lib/model.ts`.
fn removed_guard(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let guard = user
                .lines()
                .filter_map(|l| l.strip_prefix("<<<DATA "))
                .find_map(|l| {
                    let mut words = l.split(' ').skip(1);
                    let (id, side, place) = (words.next()?, words.next()?, words.next()?);
                    (side == "BASE" && place.starts_with("src/order.ts:")).then_some(id)
                });
            let candidates = guard.map_or_else(Vec::new, |id| {
                vec![json!({"category": "DEFECT", "claim": "the legacy guard is dropped and nothing replaces it",
                    "conditions": ["a legacy order"], "causal_chain": ["no guard"], "consequence": "totals it",
                    "anchors": [{"ref_id": id, "start_line": 2, "end_line": 4}], "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM", "disproof_question": "moved?"})]
            });
            let mut found = json!({"candidates": candidates, "context_requests": [], "unresolved_questions": []});
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts"
            if user.contains("BASE src/order.ts:2-4")
                && user.contains("lib/model.ts:3 -   legacy: boolean;") =>
        {
            Ok(reply(&verdict("REJECTED")))
        }
        "candidate_verdicts" => Ok(reply(&verdict("UNRESOLVED"))),
        _ => supported_and_empty(request),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_removed_guard_is_verified_with_the_field_the_change_also_removes() {
    let (outcome, calls) = run_on(
        guard_and_field_removed(),
        removed_guard,
        Budgets::default(),
        14_000,
        true,
    )
    .await;
    assert!(calls.iter().any(|c| c == "candidate_verdicts"), "{calls:?}");
    assert!(
        !outcome.candidates.is_empty()
            && outcome
                .candidates
                .iter()
                .all(|c| c.verification_status == VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}
