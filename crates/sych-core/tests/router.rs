//! Role routing and the bounded fallback loop (roadmap M1.13): deterministic traces on scripted
//! providers and one in-memory ledger, so every hop spends the same run budget.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};

use sych_core::config::{Budgets, FallbackTrigger, FixedFee, Role, ServerConfig};
use sych_core::model::{
    Denied, ErrorKind, Hop, HopOutcome, Limits, Purpose, Route, RoutePlan, RoutedOutcome, Router,
    TaskFailure, plan, reusable,
};
use sych_core::{MicroUsd, Redactor, RunId, Timestamp};

mod common;

use common::*;

static CLOCK: AtomicI64 = AtomicI64::new(1_790_000_000);

fn clock() -> Timestamp {
    Timestamp::from_second(CLOCK.load(Ordering::SeqCst)).expect("valid")
}

fn route(profile: &str) -> Route {
    let base = if profile.starts_with("sub") {
        subscription_route()
    } else {
        api_route()
    };
    Route {
        profile: profile.into(),
        ..base
    }
}

#[allow(clippy::unnecessary_wraps)] // the router's resolved-or-reason slot
fn hop<'a>(provider: &'a Scripted, profile: &str) -> Result<Hop<'a, Scripted>, String> {
    Ok(Hop {
        provider,
        route: route(profile),
        price: Some(PRICE),
        fixed_fee: Some(FixedFee {
            amount_micro_usd: MicroUsd(20_000_000),
            allocated_micro_usd: MicroUsd(10_000_000),
        }),
    })
}

fn chain(names: &[&str]) -> RoutePlan {
    RoutePlan {
        chains: BTreeMap::from([(Role::Reviewer, names.iter().map(|&n| n.into()).collect())]),
        revision: "test".into(),
    }
}

fn router<'a>(
    ledger: &'a MemLedger,
    hops: BTreeMap<String, Result<Hop<'a, Scripted>, String>>,
    names: &[&str],
    attempts: u32,
) -> Router<'a, Scripted, MemLedger> {
    Router {
        hops,
        plan: chain(names),
        on: vec![
            FallbackTrigger::QuotaExhausted,
            FallbackTrigger::RateLimited,
            FallbackTrigger::Transient,
            FallbackTrigger::Timeout,
        ],
        cooldown_seconds: 30,
        ledger,
        tenant_id: "local".into(),
        run_id: RunId::new_v7(),
        purpose: Purpose::Analysis,
        limits: limits(300_000, attempts),
        deadline: None,
        now: clock,
        redactor: Redactor::builtin(),
        cooldowns: RefCell::default(),
        reuse: BTreeMap::new(),
        models_revision: "models-1".into(),
        eligible: None,
    }
}

async fn review(r: &Router<'_, Scripted, MemLedger>) -> RoutedOutcome<Out> {
    r.structured::<Out>(Role::Reviewer, &prompt(), "discovery")
        .await
}

fn outcomes(out: &RoutedOutcome<Out>) -> Vec<(String, &'static str)> {
    out.hops
        .iter()
        .map(|h| {
            let tag = match h.outcome {
                HopOutcome::Answered => "answered",
                HopOutcome::FellBack { .. } => "fell_back",
                HopOutcome::Stopped { .. } => "stopped",
                HopOutcome::Skipped { .. } => "skipped",
                HopOutcome::Reused { .. } => "reused",
            };
            (h.profile.clone(), tag)
        })
        .collect()
}

fn ok() -> Step {
    reply(r#"{"claim":"x"}"#, Some(10))
}

#[tokio::test(flavor = "current_thread")]
async fn cheap_then_subscription_then_premium_share_one_budget() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::QuotaExhausted, false)]);
    let sub = Scripted::new(vec![error(ErrorKind::RateLimited, false)]);
    let premium = Scripted::new(vec![ok()]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("sub".into(), hop(&sub, "sub")),
        ("premium".into(), hop(&premium, "premium")),
    ]);
    let r = router(&ledger, hops, &["cheap", "sub", "premium"], 6);
    let out = review(&r).await;
    assert!(out.value.is_ok());
    assert_eq!(
        outcomes(&out),
        [
            ("cheap".into(), "fell_back"),
            ("sub".into(), "fell_back"),
            ("premium".into(), "answered")
        ]
    );
    let profiles: Vec<&str> = out.attempts.iter().map(|a| a.profile.as_str()).collect();
    assert_eq!(profiles, ["cheap", "sub", "premium"]);
    assert_eq!(ledger.rows.borrow().len(), 3, "one ledger, one run");
}

#[tokio::test(flavor = "current_thread")]
async fn auth_is_not_healed_by_another_route() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Auth, false)]);
    let premium = Scripted::new(vec![]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("premium".into(), hop(&premium, "premium")),
    ]);
    let r = router(&ledger, hops, &["cheap", "premium"], 6);
    let out = review(&r).await;
    assert_eq!(outcomes(&out), [("cheap".into(), "stopped")]);
    assert!(matches!(
        out.value,
        Err(TaskFailure::Provider {
            kind: ErrorKind::Auth,
            ..
        })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn a_cancelled_call_is_not_retried_elsewhere() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Cancelled, true)]);
    let backup = Scripted::new(vec![]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("backup".into(), hop(&backup, "backup")),
    ]);
    let r = router(&ledger, hops, &["cheap", "backup"], 6);
    let out = review(&r).await;
    assert_eq!(outcomes(&out), [("cheap".into(), "stopped")]);
    assert_eq!(
        ledger.committed_money(),
        out.attempts[0].reserved.0,
        "the cancelled call's charge stays held"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_primary_cools_down_and_is_tried_again_after() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Transient, false), ok()]);
    let backup = Scripted::new(vec![ok(), ok()]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("backup".into(), hop(&backup, "backup")),
    ]);
    let r = router(&ledger, hops, &["cheap", "backup"], 6);
    let first = review(&r).await;
    assert_eq!(
        outcomes(&first),
        [("cheap".into(), "fell_back"), ("backup".into(), "answered")]
    );
    CLOCK.fetch_add(10, Ordering::SeqCst);
    let second = review(&r).await;
    assert_eq!(
        outcomes(&second),
        [("cheap".into(), "skipped"), ("backup".into(), "answered")]
    );
    CLOCK.fetch_add(30, Ordering::SeqCst);
    let third = review(&r).await;
    assert_eq!(outcomes(&third), [("cheap".into(), "answered")]);
}

#[tokio::test(flavor = "current_thread")]
async fn exhausted_run_caps_stop_the_loop_instead_of_resetting() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Transient, false)]);
    let backup = Scripted::new(vec![]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("backup".into(), hop(&backup, "backup")),
    ]);
    let r = router(&ledger, hops, &["cheap", "backup"], 1);
    let out = review(&r).await;
    assert_eq!(
        outcomes(&out),
        [("cheap".into(), "fell_back"), ("backup".into(), "stopped")]
    );
    assert!(matches!(
        out.value,
        Err(TaskFailure::Budget {
            denied: Denied::Attempts { cap: 1 }
        })
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn an_unknown_charge_stays_reserved_while_the_next_route_runs() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Timeout, true)]);
    let backup = Scripted::new(vec![ok()]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("backup".into(), hop(&backup, "backup")),
    ]);
    let r = router(&ledger, hops, &["cheap", "backup"], 6);
    let out = review(&r).await;
    assert!(out.value.is_ok());
    let held = out.attempts[0].reserved.0;
    assert!(held > 0);
    assert_eq!(ledger.committed_money(), held + 10, "unknown + settled");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unconfigured_trigger_and_unusable_routes_end_honestly() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![error(ErrorKind::Unsupported, false)]);
    let backup = Scripted::new(vec![]);
    let hops = BTreeMap::from([
        ("cheap".into(), hop(&cheap, "cheap")),
        ("backup".into(), hop(&backup, "backup")),
    ]);
    let r = router(&ledger, hops, &["cheap", "backup"], 6);
    let out = review(&r).await;
    assert_eq!(
        outcomes(&out),
        [("cheap".into(), "stopped")],
        "unsupported_capability is not in `on`"
    );

    let none = BTreeMap::from([
        ("a".into(), Err("SYCH_KEY is not set".to_owned())),
        ("b".into(), Err("Pi is not the pinned release".to_owned())),
    ]);
    let r = router(&ledger, none, &["a", "b"], 6);
    let out = review(&r).await;
    let Err(TaskFailure::NoEligibleRoute { reasons }) = out.value else {
        panic!("expected no eligible route: {:?}", out.value);
    };
    assert_eq!(reasons.len(), 2);
    assert!(out.attempts.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn a_route_that_cannot_be_billed_hands_over_to_the_next() {
    let ledger = MemLedger::default();
    let sub = Scripted::new(vec![]);
    let cheap = Scripted::new(vec![ok()]);
    let mut unfunded = hop(&sub, "sub").expect("hop");
    unfunded.fixed_fee = None;
    let hops = BTreeMap::from([
        ("sub".into(), Ok(unfunded)),
        ("cheap".into(), hop(&cheap, "cheap")),
    ]);
    let r = router(&ledger, hops, &["sub", "cheap"], 6);
    let out = review(&r).await;
    assert_eq!(
        outcomes(&out),
        [("sub".into(), "fell_back"), ("cheap".into(), "answered")]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn each_hop_is_rechecked_right_before_it_dispatches() {
    let ledger = MemLedger::default();
    let gone = Scripted::new(vec![]);
    let plain = Scripted::new(vec![]);
    let cheap = Scripted::new(vec![error(ErrorKind::RateLimited, false), ok()]);
    let mut unstructured = hop(&plain, "plain").expect("hop");
    unstructured.route.capabilities.structured_output = false;
    let hops = BTreeMap::from([
        ("gone".into(), hop(&gone, "gone")),
        ("plain".into(), Ok(unstructured)),
        ("cheap".into(), hop(&cheap, "cheap")),
    ]);
    // A credential removed after planning, or a corporate run reaching an external endpoint,
    // fails the recheck however the plan resolved it.
    let recheck = |name: &str| {
        if name == "gone" {
            Err("SYCH_GONE_KEY is not set".to_owned())
        } else {
            Ok(())
        }
    };
    let mut r = router(&ledger, hops, &["gone", "plain", "cheap"], 6);
    r.eligible = Some(&recheck);
    let out = review(&r).await;
    assert!(out.value.is_err(), "the only usable route was rate limited");
    assert_eq!(
        outcomes(&out),
        [
            ("gone".into(), "skipped"),
            ("plain".into(), "skipped"),
            ("cheap".into(), "fell_back")
        ]
    );
    assert!(matches!(
        &out.hops[0].outcome,
        HopOutcome::Skipped { reason } if reason.contains("SYCH_GONE_KEY")
    ));
    assert_eq!(out.attempts.len(), 1, "skipped routes make no call");
}

fn personal() -> ServerConfig {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/personal/sych.yaml");
    let text = fs::read_to_string(path).expect("example");
    ServerConfig::parse("sych.yaml", &text).expect("valid")
}

#[test]
fn exact_profile_chain_wins_over_role_and_provider_defaults() {
    let models = personal().models;
    let roles = [Role::Reviewer, Role::Verifier, Role::RiskAuditor];
    let p = plan(&models, None, false, &roles).expect("plan");
    assert_eq!(
        p.chains[&Role::Reviewer],
        [
            "cheap_go",
            "glm_subscription",
            "gpt_subscription",
            "cheap_api"
        ]
    );
    assert_eq!(
        p.chains[&Role::Verifier],
        ["gpt_subscription", "glm_subscription", "cheap_api"]
    );
    assert_eq!(
        p.chains[&Role::RiskAuditor],
        ["premium_astra", "premium_fable"]
    );
    let selected = plan(&models, Some("cheap_go"), false, &[Role::Verifier]).expect("plan");
    assert_eq!(selected.chains[&Role::Verifier], ["cheap_go", "cheap_api"]);
    assert_ne!(p.revision, selected.revision);
    assert!(plan(&models, Some("nope"), false, &roles).is_err());
}

#[test]
fn a_deep_run_starts_roles_at_their_deep_profiles() {
    let models = personal().models;
    let roles = [Role::Reviewer, Role::Verifier];
    let deep = plan(&models, None, true, &roles).expect("plan");
    assert_eq!(
        deep.chains[&Role::Verifier],
        ["premium_astra", "premium_fable"]
    );
    // A role without a deep assignment keeps its regular route.
    let regular = plan(&models, None, false, &roles).expect("plan");
    assert_eq!(
        deep.chains[&Role::Reviewer],
        regular.chains[&Role::Reviewer]
    );
    assert_ne!(deep.revision, regular.revision);
    // A pinned profile wins over deep.
    let pinned = plan(&models, Some("cheap_go"), true, &[Role::Verifier]).expect("plan");
    assert_eq!(pinned.chains[&Role::Verifier], ["cheap_go", "cheap_api"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_search_pass_leaves_the_verification_reserve() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![ok(), ok()]);
    let hops = BTreeMap::from([("cheap".into(), hop(&cheap, "cheap"))]);
    let r = router(&ledger, hops, &["cheap"], 2);
    assert!(review(&r).await.value.is_ok());

    let search = r
        .search::<Out>(Role::Reviewer, &prompt(), "discovery")
        .await;
    assert!(
        matches!(
            search.value,
            Err(TaskFailure::Budget {
                denied: Denied::Attempts { cap: 1 }
            })
        ),
        "{:?}",
        search.value
    );
    assert!(
        review(&r).await.value.is_ok(),
        "the verifier keeps its attempt"
    );
}

#[test]
fn the_verification_reserve_keeps_a_share_of_the_tokens() {
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(6);
    budgets.analysis.max_input_tokens = Some(64_000);
    budgets.analysis.max_output_tokens = Some(24_000);
    let search = Limits::new(&budgets, Purpose::Analysis).reserving_verification();
    assert_eq!(search.run.max_attempts, Some(5));
    assert_eq!(search.run.max_input_tokens, Some(53_334));
    assert_eq!(search.run.max_output_tokens, Some(20_000));
}

#[tokio::test(flavor = "current_thread")]
async fn a_successor_reuses_an_answer_only_for_the_same_task_inputs() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![ok()]);
    let parent = router(
        &ledger,
        BTreeMap::from([("cheap".into(), hop(&cheap, "cheap"))]),
        &["cheap"],
        6,
    );
    assert!(review(&parent).await.value.is_ok());
    let saved = ledger.checkpoints.borrow().clone();
    assert_eq!(saved.len(), 1, "an accepted answer is checkpointed");
    assert_eq!(
        (saved[0].profile.as_str(), saved[0].run_id),
        ("cheap", parent.run_id)
    );
    assert_eq!(saved[0].deps.template, "discovery@1");

    // The same task in a successor makes no call and spends nothing.
    let next = MemLedger::default();
    let idle = Scripted::new(vec![]);
    let mut successor = router(
        &next,
        BTreeMap::from([("cheap".into(), hop(&idle, "cheap"))]),
        &["cheap"],
        6,
    );
    successor.reuse = reusable(saved.clone());
    let again = review(&successor).await;
    assert_eq!(again.value, Ok(Out { claim: "x".into() }));
    assert!(again.attempts.is_empty() && next.rows.borrow().is_empty());
    assert_eq!(again.reused_from, Some(parent.run_id));
    assert_eq!(outcomes(&again), [("cheap".into(), "reused")]);
    let kept = next.checkpoints.borrow().clone();
    assert_eq!(kept.len(), 1, "kept for the successor's own successors");
    assert_eq!(
        kept[0].run_id, parent.run_id,
        "the origin of the answer stays"
    );

    // Changed shown data, output contract or model config: a fresh call each.
    let fresh = Scripted::new(vec![ok(), ok(), ok()]);
    let mut other = router(
        &next,
        BTreeMap::from([("cheap".into(), hop(&fresh, "cheap"))]),
        &["cheap"],
        6,
    );
    other.reuse = reusable(saved);
    let mut changed = prompt();
    changed.data[0].content.push_str("\nfn added() {}");
    let out = other
        .structured::<Out>(Role::Reviewer, &changed, "discovery")
        .await;
    assert_eq!((out.attempts.len(), out.reused_from), (1, None));
    let out = other
        .structured::<Out>(Role::Reviewer, &prompt(), "verdicts")
        .await;
    assert_eq!((out.attempts.len(), out.reused_from), (1, None));
    other.models_revision = "models-2".into();
    let out = review(&other).await;
    assert_eq!((out.attempts.len(), out.reused_from), (1, None));
}

#[tokio::test(flavor = "current_thread")]
async fn a_tampered_or_unreadable_checkpoint_is_not_reused() {
    let ledger = MemLedger::default();
    let cheap = Scripted::new(vec![ok()]);
    let parent = router(
        &ledger,
        BTreeMap::from([("cheap".into(), hop(&cheap, "cheap"))]),
        &["cheap"],
        6,
    );
    review(&parent).await;
    let saved = ledger.checkpoints.borrow().clone();

    let mut moved = saved[0].clone();
    moved.deps.schema_name = "elsewhere".into();
    assert!(
        reusable(vec![moved]).is_empty(),
        "the key must be the digest of its dependencies"
    );

    let mut broken = saved[0].clone();
    broken.output = serde_json::json!({"unexpected": true});
    let fresh = Scripted::new(vec![ok()]);
    let mut successor = router(
        &ledger,
        BTreeMap::from([("cheap".into(), hop(&fresh, "cheap"))]),
        &["cheap"],
        6,
    );
    successor.reuse = reusable(vec![broken]);
    let out = review(&successor).await;
    assert_eq!((out.attempts.len(), out.reused_from), (1, None));
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_checkpoint_keeps_the_answer_and_says_why() {
    let ledger = MemLedger {
        checkpoint_error: Some("disk full".into()),
        ..MemLedger::default()
    };
    let cheap = Scripted::new(vec![ok()]);
    let r = router(
        &ledger,
        BTreeMap::from([("cheap".into(), hop(&cheap, "cheap"))]),
        &["cheap"],
        6,
    );
    let out = review(&r).await;
    assert_eq!(out.value, Ok(Out { claim: "x".into() }));
    assert_eq!(out.unkept.as_deref(), Some("ledger: disk full"));
    assert!(ledger.checkpoints.borrow().is_empty());
}
