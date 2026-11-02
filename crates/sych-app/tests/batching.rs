//! How review units are batched into model calls, and what a batch cut short becomes: the
//! same units redone over smaller calls, never a silent gap.

use std::collections::BTreeMap;

use sych_app::analysis::PassStatus;
use sych_core::config::Budgets;
use sych_core::model::{Finish, ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

#[tokio::test(flavor = "current_thread")]
async fn small_units_share_one_discovery_call() {
    let (outcome, calls) = run_on(
        repo(true),
        supported_and_empty,
        Budgets::default(),
        14_000,
        true,
    )
    .await;
    // One review call for both units that also assesses the rule, then the risk audit.
    assert_eq!(calls, ["review", "discovery"]);
    let review = &outcome.record.passes[0];
    assert_eq!(review.packets.len(), 2, "{:?}", review.packets);
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
}

#[tokio::test(flavor = "current_thread")]
async fn a_profile_bound_on_units_keeps_small_units_in_separate_calls() {
    let repo = repo(true);
    let (outcome, calls, _) = run_reusing(
        repo.path(),
        supported_and_empty,
        Budgets::default(),
        (14_000, 1),
        true,
        BTreeMap::new(),
    )
    .await;
    // The two units would share one review call; one unit per call keeps them apart, and the
    // rule that spans both is assessed on its own.
    let searches = outcome.record.passes.iter();
    let searches: Vec<usize> = searches
        .filter(|p| p.template.starts_with("discovery") && p.obligation_id.is_none())
        .map(|p| p.packets.len())
        .collect();
    assert_eq!(searches, [1, 1], "{calls:?}");
    assert!(!calls.contains(&"review".to_owned()), "{calls:?}");
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
}

/// Cuts every reply that covers both units, as a model whose reasoning runs away on wide
/// calls; the high-risk audit, one packet, is answered.
fn wide_calls_cut(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let prompt: String = request
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    let audit = prompt.contains("risk auditor");
    let wide = prompt.contains("src/order.ts") && prompt.contains("lib/format.ts");
    if request.schema_name == "review" || (request.schema_name == "discovery" && wide && !audit) {
        let mut cut = reply("");
        cut.finish = Finish::Truncated;
        return Ok(cut);
    }

    supported_and_empty(request)
}

#[tokio::test(flavor = "current_thread")]
async fn a_cut_wide_call_is_redone_unit_by_unit_without_a_gap() {
    let (outcome, calls) =
        run_on(repo(true), wide_calls_cut, Budgets::default(), 14_000, true).await;
    // The cut review leaves its rule to an assessment of its own and its units to discovery,
    // which is cut over both units and redone over each.
    assert_eq!(
        calls.iter().filter(|c| *c == "review").count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.contains(&"obligation_assessment".to_owned()),
        "{calls:?}"
    );
    let split = outcome.record.passes.iter();
    let split: Vec<usize> = split
        .filter(|p| matches!(p.status, PassStatus::Split { .. }))
        .map(|p| p.packets.len())
        .collect();
    assert_eq!(split, [2, 2], "the review and the wide discovery");
    let single = outcome.record.passes.iter().filter(|p| {
        p.template.starts_with("discovery")
            && p.obligation_id.is_none()
            && p.packets.len() == 1
            && matches!(p.status, PassStatus::Done)
    });
    assert_eq!(single.count(), 2);
    assert!(outcome.gaps.is_empty(), "{:?}", outcome.gaps);
    assert!(!outcome.limit_reached);
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
    let failed = outcome.record.passes.iter();
    assert!(
        !failed
            .into_iter()
            .any(|p| matches!(p.status, PassStatus::Failed { .. }))
    );
}
