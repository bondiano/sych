//! How review units are batched into model calls, and what a batch cut short becomes: the
//! same units redone over smaller calls, never a silent gap.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use sych_app::analysis::{Pass, PassStatus};
use sych_core::config::Budgets;
use sych_core::model::{ErrorKind, Finish, ModelReply, ModelRequest, ProviderError};

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
        None,
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
    // Spans of both units (`path:start-end`), not the list naming files a call does not show.
    let wide = prompt.contains("src/order.ts:") && prompt.contains("lib/format.ts:");
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

static REVIEW_REFUSED: AtomicBool = AtomicBool::new(false);
static SEARCH_REFUSED: AtomicBool = AtomicBool::new(false);

/// Rate limits the first review and the first plain discovery, as a provider under load does
/// for a moment; every later call is answered.
fn first_calls_rate_limited(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let audit = request
        .messages
        .iter()
        .any(|m| m.content.contains("risk auditor"));
    let first = match request.schema_name.as_str() {
        "review" => !REVIEW_REFUSED.swap(true, Ordering::SeqCst),
        "discovery" if !audit => !SEARCH_REFUSED.swap(true, Ordering::SeqCst),
        _ => false,
    };
    if first {
        return Err(ProviderError {
            kind: ErrorKind::RateLimited,
            dispatched: false,
            message: "rate limited".into(),
            retry_after_seconds: None,
        });
    }

    supported_and_empty(request)
}

#[tokio::test(flavor = "current_thread")]
async fn a_search_the_provider_turned_away_is_asked_again_without_a_gap() {
    let (outcome, calls) = run_on(
        repo(true),
        first_calls_rate_limited,
        Budgets::default(),
        14_000,
        true,
    )
    .await;
    // The refused review leaves its rule to an assessment and its units to discovery; that
    // discovery is refused too and asked once more.
    let plain = |p: &&Pass| p.template.starts_with("discovery") && p.obligation_id.is_none();
    let searches: Vec<bool> = outcome
        .record
        .passes
        .iter()
        .filter(plain)
        .map(|p| matches!(p.status, PassStatus::Done))
        .collect();
    assert_eq!(searches, [false, true], "{calls:?}");
    assert!(
        calls.contains(&"obligation_assessment".to_owned()),
        "{calls:?}"
    );
    assert!(outcome.gaps.is_empty(), "{:?}", outcome.gaps);
    assert_eq!(reason(&outcome.assessments[0].status), "SUPPORTED");
}
