//! How one run's budget is shared between passes over a real snapshot with a scripted
//! provider: required obligations first, a share left for discovery, siblings retried.

use sych_app::analysis::PassStatus;
use sych_core::config::Budgets;

mod harness;

use harness::*;

#[tokio::test(flavor = "current_thread")]
async fn a_pass_denied_beside_an_unsettled_sibling_is_retried() {
    // Two searches in flight together exceed the cap by their byte bounds; settled, they fit.
    let run_with = |parallel| {
        let mut budgets = Budgets::default();
        budgets.analysis.max_input_tokens = Some(8_000);
        budgets.analysis.max_parallel_calls = Some(parallel);
        run_on(repo(true), supported_and_empty, budgets, 1, false)
    };
    let (sequential, mut sequential_calls) = run_with(1).await;
    let (parallel, mut parallel_calls) = run_with(2).await;
    // Parallel calls start in no fixed order.
    sequential_calls.sort();
    parallel_calls.sort();
    assert!(sequential.gaps.is_empty(), "{:?}", sequential.gaps);
    assert!(
        parallel.gaps.is_empty() && !parallel.limit_reached,
        "{:?}",
        parallel.gaps
    );
    assert_eq!(parallel_calls, sequential_calls);
    assert_eq!(parallel.record.passes.len(), sequential.record.passes.len());
}

#[tokio::test(flavor = "current_thread")]
async fn a_required_standalone_assessment_goes_before_an_optional_combined_review() {
    // The required rule spans both units, so no single batch shows it whole; the optional
    // one covers only `lib/` and is assessed inside that unit's review call.
    let optional = "\n  - id: FORMAT-01\n    version: 1\n    title: Money keeps its sign\n    kind: invariant\n    required: false\n    severity_on_violation: low\n    owner: backend\n    applies_to:\n      paths: [\"lib/**\"]\n    statement: money() keeps the sign of n.\n";
    let rules = RULES.replace("[\"src/**\"]", "[\"src/**\", \"lib/**\"]") + optional;
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(1);
    let (_, calls) = run_on(
        repo_with(true, &[(".sych/rules.yaml", &rules)]),
        supported_and_empty,
        budgets,
        1,
        true,
    )
    .await;
    assert_eq!(
        calls,
        ["obligation_assessment"],
        "the one attempt goes to the required rule"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn required_assessments_leave_discovery_its_share_of_the_attempts() {
    // Four required rules span both units, so each needs a standalone assessment; the two
    // units are two discovery batches.
    let rule = |id: &str| {
        format!(
            "\n  - id: {id}\n    version: 1\n    title: {id}\n    kind: invariant\n    required: true\n    severity_on_violation: low\n    owner: backend\n    applies_to:\n      paths: [\"src/**\", \"lib/**\"]\n    statement: {id} holds.\n"
        )
    };
    let rules = RULES.replace("[\"src/**\"]", "[\"src/**\", \"lib/**\"]")
        + &rule("EXTRA-01")
        + &rule("EXTRA-02")
        + &rule("EXTRA-03");
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(5);
    let (outcome, calls) = run_on(
        repo_with(true, &[(".sych/rules.yaml", &rules)]),
        supported_and_empty,
        budgets,
        1,
        true,
    )
    .await;
    let done = |template: &str| {
        let passes = outcome.record.passes.iter();
        passes
            .filter(|p| p.template.starts_with(template) && matches!(p.status, PassStatus::Done))
            .count()
    };
    assert_eq!(done("discovery"), 2, "{calls:?}");
    assert_eq!(
        done("assessment"),
        2,
        "the rest of the cap less the verifier's attempt"
    );
    assert_eq!(
        done("risk_audit"),
        0,
        "the audit cannot spend discovery's share"
    );
    let unassessed = outcome.assessments.iter();
    assert_eq!(
        unassessed
            .filter(|a| reason(&a.status) == "BUDGET_EXHAUSTED")
            .count(),
        2
    );
    assert!(outcome.limit_reached);
}
