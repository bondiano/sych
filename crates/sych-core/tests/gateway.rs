//! Model gateway scenarios (roadmap M1.3): repair, unknown charges, unsupported routes, budget
//! exhaustion and hard-vs-soft enforcement, on a scripted provider and an in-memory ledger.

use std::sync::atomic::{AtomicI64, Ordering};

use sych_core::config::FixedFee;
use sych_core::model::{
    AttemptKind, ChargeState, Denied, ErrorKind, Gateway, Limits, ObservedUsage, Purpose, Route,
    Speaker, TaskFailure, TaskOutcome,
};
use sych_core::{MicroUsd, Redactor, RunId, Timestamp};

mod common;

use common::*;

async fn run(
    provider: &Scripted,
    ledger: &MemLedger,
    route: Route,
    limits: Limits,
    fee: Option<u64>,
) -> TaskOutcome<Out> {
    let gateway = Gateway {
        provider,
        ledger,
        route,
        price: Some(PRICE),
        fixed_fee: fee.map(|f| FixedFee {
            amount_micro_usd: MicroUsd(f * 2),
            allocated_micro_usd: MicroUsd(f),
        }),
        tenant_id: "local".into(),
        run_id: RunId::new_v7(),
        purpose: Purpose::Analysis,
        limits,
        deadline: None,
        now,
        redactor: Redactor::builtin(),
    };
    gateway.structured::<Out>(&prompt(), "discovery").await
}

#[tokio::test(flavor = "current_thread")]
async fn valid_reply_is_settled_with_the_reported_charge() {
    let provider = Scripted::new(vec![reply(r#"{"claim":"x"}"#, Some(140))]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert_eq!(out.value, Ok(Out { claim: "x".into() }));
    let [attempt] = out.attempts.as_slice() else {
        panic!("one attempt")
    };
    assert_eq!(attempt.charge_state, ChargeState::Settled);
    assert_eq!(attempt.charge, Some(MicroUsd(140)));
    assert_eq!(
        attempt.resolved_model.as_deref(),
        Some("vendor/model-2026-10")
    );
    assert!(attempt.reserved.0 >= 140, "upper bound covers the charge");
    assert_eq!(ledger.committed_money(), 140);
    let seen = provider.seen.borrow();
    assert_eq!(seen[0].max_output_tokens, Some(2_000));
    assert_eq!(seen[0].schema["properties"]["claim"]["type"], "string");
}

#[tokio::test(flavor = "current_thread")]
async fn configured_request_cap_bounds_output_and_the_reservation() {
    let provider = Scripted::new(vec![reply(r#"{"claim":"x"}"#, Some(1))]);
    let ledger = MemLedger::default();
    let mut wide = limits(300_000, 6);
    wide.run.max_output_tokens_per_request = Some(4_000);
    let out = run(&provider, &ledger, api_route(), wide, None).await;
    assert!(out.value.is_ok());
    assert_eq!(provider.seen.borrow()[0].max_output_tokens, Some(4_000));
    let default = Scripted::new(vec![reply(r#"{"claim":"x"}"#, Some(1))]);
    let small = run(
        &default,
        &MemLedger::default(),
        api_route(),
        limits(300_000, 6),
        None,
    )
    .await;
    assert!(
        out.attempts[0].reserved > small.attempts[0].reserved,
        "the money bound covers the larger cap"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn repository_text_stays_fenced_data() {
    let provider = Scripted::new(vec![reply(r#"{"claim":"x"}"#, Some(1))]);
    let ledger = MemLedger::default();
    run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    let seen = provider.seen.borrow();
    let [system, user] = seen[0].messages.as_slice() else {
        panic!("two messages")
    };
    assert_eq!(system.speaker, Speaker::System);
    assert!(!system.content.contains("ignore all rules"));
    let fence = user
        .content
        .split("<<<DATA ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .expect("fenced");
    assert!(user.content.contains(&format!("<<<END {fence}>>>")));
    assert_ne!(fence, "x");
}

#[tokio::test(flavor = "current_thread")]
async fn one_repair_fixes_invalid_json() {
    let provider = Scripted::new(vec![
        reply("Sure! claim: x", Some(10)),
        reply(r#"{"claim":"x"}"#, Some(12)),
    ]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert!(out.value.is_ok());
    let kinds: Vec<_> = out.attempts.iter().map(|a| a.kind).collect();
    assert_eq!(kinds, [AttemptKind::Initial, AttemptKind::Repair]);
    assert!(matches!(
        out.attempts[0].failure,
        Some(TaskFailure::Schema { .. })
    ));
    assert_eq!(
        ledger.committed_money(),
        22,
        "the repair is a separate paid attempt"
    );
    let repair = &provider.seen.borrow()[1].messages;
    assert_eq!(repair[2].content, "Sure! claim: x");
}

#[tokio::test(flavor = "current_thread")]
async fn failed_repair_leaves_the_task_failed_without_a_third_call() {
    let provider = Scripted::new(vec![
        reply(r#"{"claim":1}"#, Some(10)),
        reply(r#"{"claim":"x","extra":true}"#, Some(10)),
    ]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert!(matches!(out.value, Err(TaskFailure::Schema { .. })));
    assert_eq!(out.attempts.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_after_dispatch_keeps_the_reservation_as_unknown_charge() {
    let provider = Scripted::new(vec![error(ErrorKind::Timeout, true)]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    let Err(TaskFailure::Provider { kind, .. }) = out.value else {
        panic!("provider failure")
    };
    assert_eq!(kind, ErrorKind::Timeout);
    let attempt = &out.attempts[0];
    assert_eq!(attempt.charge_state, ChargeState::Unknown);
    assert_eq!(
        attempt.usage,
        ObservedUsage::default(),
        "unknown usage, not zero"
    );
    assert_eq!(ledger.committed_money(), attempt.reserved.0);
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_before_dispatch_releases_and_is_not_success() {
    let provider = Scripted::new(vec![error(ErrorKind::Unsupported, false)]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert!(matches!(
        out.value,
        Err(TaskFailure::Provider {
            kind: ErrorKind::Unsupported,
            ..
        })
    ));
    assert_eq!(out.attempts[0].charge_state, ChargeState::Released);
    assert_eq!(ledger.committed_money(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn exhausted_budget_denies_before_dispatch() {
    let provider = Scripted::new(vec![]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(10, 6), None).await;
    assert!(matches!(
        out.value,
        Err(TaskFailure::Budget {
            denied: Denied::Money { .. }
        })
    ));
    assert!(out.attempts.is_empty() && provider.seen.borrow().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn repair_counts_against_the_attempt_cap() {
    let provider = Scripted::new(vec![reply("not json", Some(1))]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 1), None).await;
    assert_eq!(
        out.value,
        Err(TaskFailure::Budget {
            denied: Denied::Attempts { cap: 1 }
        })
    );
    assert_eq!(out.attempts.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn metered_route_without_a_hard_bound_is_refused() {
    let provider = Scripted::new(vec![]);
    let ledger = MemLedger::default();
    let route = Route {
        capabilities: caps(false),
        ..api_route()
    };
    let out = run(&provider, &ledger, route, limits(300_000, 6), None).await;
    assert_eq!(
        out.value,
        Err(TaskFailure::Budget {
            denied: Denied::NoPriceBound
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn missing_reported_cost_without_usage_is_unknown_not_zero() {
    let mut step = reply(r#"{"claim":"x"}"#, None);
    if let Ok(r) = &mut step {
        r.usage = ObservedUsage::default();
    }

    let provider = Scripted::new(vec![step]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert!(out.value.is_ok());
    assert_eq!(out.attempts[0].charge_state, ChargeState::Unknown);
    assert!(ledger.committed_money() > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn subscription_needs_an_accepted_fee_and_keeps_upstream_limits_soft() {
    let ledger = MemLedger::default();
    let none = Scripted::new(vec![]);
    let out = run(
        &none,
        &ledger,
        subscription_route(),
        limits(300_000, 6),
        None,
    )
    .await;
    assert_eq!(
        out.value,
        Err(TaskFailure::Budget {
            denied: Denied::FixedFeeUnknown
        })
    );

    let provider = Scripted::new(vec![reply(r#"{"claim":"x"}"#, None)]);
    let out = run(
        &provider,
        &ledger,
        subscription_route(),
        limits(300_000, 6),
        Some(5_000_000),
    )
    .await;
    assert!(out.value.is_ok());
    assert_eq!(out.attempts[0].reserved, MicroUsd(0));
    assert_eq!(
        out.attempts[0].charge, None,
        "no reported money is not a zero charge"
    );
    assert_eq!(provider.seen.borrow()[0].max_output_tokens, None);
    let rows = ledger.rows.borrow();
    assert_eq!(rows[0].0.output_tokens, None, "upstream output is soft");
    assert_eq!(
        rows[0].0.fixed_fee.map(|f| f.allocated_micro_usd),
        Some(MicroUsd(5_000_000))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn plan_allowance_is_recorded_apart_from_a_zero_charge() {
    let mut within = reply(r#"{"claim":"x"}"#, Some(0)).expect("reply");
    within.billing_source = Some("go".into());
    within.allowance = Some(MicroUsd(210));
    let provider = Scripted::new(vec![Ok(within)]);
    let ledger = MemLedger::default();
    let out = run(
        &provider,
        &ledger,
        subscription_route(),
        limits(300_000, 6),
        Some(5_000_000),
    )
    .await;
    let attempt = &out.attempts[0];
    assert_eq!(attempt.charge_state, ChargeState::Settled);
    assert_eq!(attempt.charge, Some(MicroUsd(0)), "reported zero is a fact");
    assert_eq!(attempt.allowance, Some(MicroUsd(210)));
    assert_eq!(attempt.billing_source.as_deref(), Some("go"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_repair_continues_the_same_task() {
    let provider = Scripted::new(vec![
        reply("not json", Some(10)),
        reply(r#"{"claim":"x"}"#, Some(10)),
    ]);
    let ledger = MemLedger::default();
    let out = run(&provider, &ledger, api_route(), limits(300_000, 6), None).await;
    assert!(out.value.is_ok());
    let seen = provider.seen.borrow();
    assert_eq!(seen[0].task_id, seen[1].task_id);
    assert_ne!(seen[0].attempt_id, seen[1].attempt_id);
}

/// The clock moves ten seconds per reading after the first.
fn ticking() -> Timestamp {
    static READS: AtomicI64 = AtomicI64::new(0);
    let reads = READS.fetch_add(1, Ordering::SeqCst);
    Timestamp::from_second(now().as_second() + reads * 10).expect("valid")
}

#[tokio::test(flavor = "current_thread")]
async fn deadline_reached_while_reserving_releases_without_dispatch() {
    let provider = Scripted::new(Vec::new());
    let ledger = MemLedger::default();
    let gateway = Gateway {
        provider: &provider,
        ledger: &ledger,
        route: api_route(),
        price: Some(PRICE),
        fixed_fee: None,
        tenant_id: "local".into(),
        run_id: RunId::new_v7(),
        purpose: Purpose::Analysis,
        limits: limits(300_000, 6),
        deadline: Some(Timestamp::from_second(now().as_second() + 5).expect("valid")),
        now: ticking,
        redactor: Redactor::builtin(),
    };
    let out = gateway.structured::<Out>(&prompt(), "discovery").await;
    assert!(matches!(
        out.value,
        Err(TaskFailure::Provider {
            kind: ErrorKind::Timeout,
            ..
        })
    ));
    assert!(provider.seen.borrow().is_empty(), "nothing was sent");
    assert_eq!(out.attempts[0].charge_state, ChargeState::Released);
    assert_eq!(ledger.committed_money(), 0);
}
