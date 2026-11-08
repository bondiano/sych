//! A claim that something is absent is published only after the verifier has seen HEAD
//! searched for what it names, and the whole answer of that search.

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use sych_app::analysis::Outcome;
use sych_core::VerificationStatus;
use sych_core::analysis::AskOperation;
use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};
use tempfile::TempDir;

mod harness;

use harness::*;

const UNSET: &str = "No test sets skip_guard: true, so the fallback is not exercised";
const MOVED: &str = "Deleting the total_of_undefined_is_zero test leaves no check of the fallback";
const TEST: &str = "test/order.test.ts";

/// A repository whose base has `base` files and whose head changes `src/order.ts` and adds
/// `added` files.
fn repo(base: &[(&str, &str)], added: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    let write = |files: &[(&str, &str)]| {
        for (path, text) in files {
            let path = d.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, text).expect("file");
        }
    };
    write(&[(".sych/rules.yaml", RULES), ("src/order.ts", BASE)]);
    write(base);
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    write(&[("src/order.ts", SAFE)]);
    write(added);
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "head"]);
    dir
}

/// Discovery claims `claim`; the verifier confirms it unless a span of the test file is shown.
fn script(request: &ModelRequest, claim: &str) -> Result<ModelReply, ProviderError> {
    let user = &request.messages[1].content;
    let (id, line) = first_ref(request, "HEAD");
    let anchors = json!([{"ref_id": id, "start_line": line, "end_line": line}]);
    match request.schema_name.as_str() {
        "discovery" | "review" => {
            let mut found = json!({
                "candidates": [{"category": "DEFECT", "claim": claim,
                    "conditions": ["items undefined"], "causal_chain": ["no test"],
                    "consequence": "a regression ships", "anchors": anchors, "evidence_ids": [],
                    "obligation_ids": [], "proposed_severity": "MEDIUM",
                    "disproof_question": "a test elsewhere?"}],
                "context_requests": [], "unresolved_questions": []
            });
            if request.schema_name == "review" {
                found["assessments"] = json!([supported(request)]);
            }

            Ok(reply(&found.to_string()))
        }
        "candidate_verdicts" => {
            let seen = user
                .lines()
                .any(|l| l.starts_with("<<<DATA ") && l.contains(&format!(" {TEST}:")));
            let verdict = if seen { "REJECTED" } else { "CONFIRMED" };
            Ok(reply(
                &json!({"verdicts": [{"candidate_id": "cand-1", "verdict": verdict,
                    "category": "DEFECT", "severity": "MEDIUM", "support": "SEMANTIC_ARGUMENT",
                    "title": "untested", "explanation": claim, "anchors": anchors,
                    "evidence_ids": [], "alternatives_checked": [], "suggested_action": "",
                    "validation_expectation": "", "limitations": []}],
                    "context_requests": []})
                .to_string(),
            ))
        }
        _ => supported_and_empty(request),
    }
}

fn unset(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    script(request, UNSET)
}

fn moved(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    script(request, MOVED)
}

async fn review(
    repo: &TempDir,
    answer: fn(&ModelRequest) -> Result<ModelReply, ProviderError>,
) -> Outcome {
    // One unit per call: the pass over `src/order.ts` does not show the test file.
    run_reusing(
        repo.path(),
        answer,
        Budgets::default(),
        (14_000, 1),
        true,
        BTreeMap::new(),
    )
    .await
    .0
}

fn searched(outcome: &Outcome, name: &str) -> usize {
    outcome
        .record
        .answers
        .iter()
        .filter(
            |a| matches!(&a.operation, AskOperation::SearchSource { query, .. } if query == name),
        )
        .map(|a| a.refs)
        .sum()
}

fn all(outcome: &Outcome, status: VerificationStatus) -> bool {
    !outcome.candidates.is_empty()
        && outcome
            .candidates
            .iter()
            .all(|c| c.verification_status == status)
}

#[tokio::test(flavor = "current_thread")]
async fn a_test_outside_the_change_refutes_a_confirmed_absence() {
    let repo = repo(&[(TEST, "run(total, { skip_guard: true });\n")], &[]);
    let outcome = review(&repo, unset).await;
    assert!(
        searched(&outcome, "skip_guard") > 0,
        "{:?}",
        outcome.record.answers
    );
    assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
    assert!(
        all(&outcome, VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_test_added_in_the_change_refutes_a_deleted_test_claim() {
    let added = "test(\"total_of_undefined_is_zero\", () => expect(total()).toBe(0));\n";
    let repo = repo(&[], &[(TEST, added)]);
    let outcome = review(&repo, moved).await;
    assert!(searched(&outcome, "total_of_undefined_is_zero") > 0);
    assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
    assert!(
        all(&outcome, VerificationStatus::Rejected),
        "{:?}",
        outcome.candidates
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_true_absence_stays_a_finding() {
    let repo = repo(&[(TEST, "run(total, {});\n")], &[]);
    let (outcome, calls) = run_on(repo, unset, Budgets::default(), 14_000, true).await;
    let searches = calls.iter().filter(|c| *c != "candidate_verdicts").count();
    assert_eq!(
        calls.iter().filter(|c| *c == "candidate_verdicts").count(),
        2 * searches,
        "confirmed again after the search: {calls:?}"
    );
    assert!(
        all(&outcome, VerificationStatus::Confirmed),
        "{:?}",
        outcome.candidates
    );
    assert!(!outcome.findings.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_search_cut_short_does_not_show_an_absence() {
    let flags = "skip_guard;\n".repeat(5);
    let repo = repo(
        &[("src/flags.ts", &flags), (TEST, "run(total, {});\n")],
        &[],
    );
    let outcome = review(&repo, unset).await;
    assert!(outcome.findings.is_empty(), "{:?}", outcome.findings);
    assert!(
        all(&outcome, VerificationStatus::Unresolved),
        "{:?}",
        outcome.candidates
    );
    assert!(outcome.candidates.iter().all(|c| {
        c.limitations
            .iter()
            .any(|l| l.contains("ABSENCE_NOT_SHOWN") && l.contains("`skip_guard`"))
    }));
}
