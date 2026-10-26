//! Successor reuse on the real engine (spec §8.5, §9.6): a new run over the same inputs
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
            14_000,
            true,
            reuse,
        )
    };
    let (parent, calls, kept) = run(BTreeMap::new()).await;
    assert_eq!(calls, ["review", "discovery"]);
    assert_eq!(kept.len(), 2, "every accepted answer is checkpointed");

    // Another run id, the same snapshot and policy: no run-specific value reaches a prompt.
    let (successor, calls, carried) = run(reusable(kept.clone())).await;
    assert!(calls.is_empty(), "no model call: {calls:?}");
    assert!(successor.record.attempts.is_empty());
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
