//! Route planning and budget shares: which profiles a run's roles start at, and what a pass
//! keeps for the passes after it.

use std::fs;
use std::path::Path;

use sych_core::config::{Budgets, Role, ServerConfig};
use sych_core::model::{Limits, Purpose, plan};

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

#[test]
fn the_verification_reserve_keeps_a_share_of_the_tokens() {
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(6);
    budgets.analysis.max_input_tokens = Some(64_000);
    budgets.analysis.max_output_tokens = Some(24_000);
    let limits = Limits::new(&budgets, Purpose::Analysis);
    let search = limits.reserving(1);
    assert_eq!(search.run.max_attempts, Some(5));
    assert_eq!(search.run.max_input_tokens, Some(53_334));
    assert_eq!(search.run.max_output_tokens, Some(20_000));
    // Three attempts kept for later passes take three shares.
    let assessing = limits.reserving(3);
    assert_eq!(assessing.run.max_attempts, Some(3));
    assert_eq!(assessing.run.max_input_tokens, Some(32_002));
    assert_eq!(assessing.run.max_output_tokens, Some(12_000));
}

#[test]
fn discovery_keeps_a_call_per_batch_up_to_half_the_cap() {
    let share = |cap: Option<u32>, calls: usize| {
        let mut budgets = Budgets::default();
        budgets.analysis.max_attempts = cap;
        Limits::new(&budgets, Purpose::Analysis).discovery_share(calls)
    };
    assert_eq!(share(Some(12), 4), 4);
    // Half of what the verifier's attempt leaves; the required obligations keep the rest.
    assert_eq!(share(Some(12), 9), 5);
    // One attempt goes to the obligation; nothing is kept that would take it away.
    assert_eq!(share(Some(1), 3), 0);
    assert_eq!(share(Some(2), 3), 0);
    assert_eq!(share(None, 3), 0, "no attempt cap, nothing to keep");
}
