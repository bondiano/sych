//! Successor reuse on the real engine: a new run over the same inputs
//! takes every answer from its parent's checkpoints without a model call, and a changed
//! input runs its tasks again.

mod harness;

use std::collections::BTreeMap;
use std::fs;

use sych_app::analysis::Outcome;
use sych_core::config::Budgets;
use sych_core::model::reusable;

use harness::*;

#[tokio::test(flavor = "current_thread")]
async fn a_successor_reuses_its_parents_answers_only_for_unchanged_inputs() {
    let repo = repo(false);
    let run = |reuse| {
        run_reusing(
            repo.path(),
            supported_and_empty,
            Budgets::default(),
            (14_000, usize::MAX),
            true,
            reuse,
            None,
        )
    };
    let (parent, calls, kept) = run(BTreeMap::new()).await;
    assert_eq!(calls, ["review", "discovery"]);
    assert_eq!(kept.len(), 2, "every accepted answer is checkpointed");

    // Another run id, the same snapshot and policy: no run-specific value reaches a prompt.
    let (successor, calls, carried) = run(reusable(kept.clone())).await;
    assert!(calls.is_empty(), "no model call: {calls:?}");
    assert!(
        successor.record.attempts.is_empty(),
        "{:?}",
        successor.record.attempts
    );
    let origin = kept[0].run_id;
    assert!(
        successor
            .record
            .passes
            .iter()
            .all(|p| p.reused_from == Some(origin))
    );
    assert_eq!(
        carried, kept,
        "kept again for the successor's own successors"
    );
    let statuses = |o: &Outcome| {
        o.assessments
            .iter()
            .map(|a| reason(&a.status).to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(statuses(&successor), statuses(&parent));
    assert_eq!(statuses(&successor), ["SUPPORTED"]);

    // A changed head is new input: its tasks are asked again.
    fs::write(
        repo.path().join("src/order.ts"),
        SAFE.replace("[]", "new Array()"),
    )
    .expect("edit");
    git(repo.path(), &["commit", "-qam", "edit"]);
    let (_, calls, _) = run(reusable(kept)).await;
    assert_eq!(calls, ["review", "discovery"]);
}

#[tokio::test(flavor = "current_thread")]
async fn unrelated_preference_rule_leaves_checkpoint_hashes_unchanged() {
    let base_repo = repo(false);
    let (_, calls, kept) = run_reusing(
        base_repo.path(),
        supported_and_empty,
        Budgets::default(),
        (14_000, usize::MAX),
        true,
        BTreeMap::new(),
        None,
    )
    .await;
    assert_eq!(calls, ["review", "discovery"]);
    assert_eq!(kept.len(), 2);

    // Add a path-scoped preference for `src/api/**` on a repo whose change only touches `src/order.ts`.
    let pref_rules = format!(
        "{RULES}  - id: API-PREF\n    version: 1\n    title: API handler names\n    kind: preference\n    required: false\n    owner: api\n    applies_to:\n      paths: [\"src/api/**\"]\n    statement: Name handlers after the resource verb.\n"
    );
    let pref_repo = repo_with(false, &[(".sych/rules.yaml", &pref_rules)]);
    let (_, reuse_calls, _) = run_reusing(
        pref_repo.path(),
        supported_and_empty,
        Budgets::default(),
        (14_000, usize::MAX),
        true,
        reusable(kept),
        None,
    )
    .await;
    assert!(
        reuse_calls.is_empty(),
        "a preference scoped to src/api/** must not alter prompt hashes for src/order.ts: {reuse_calls:?}"
    );
}
