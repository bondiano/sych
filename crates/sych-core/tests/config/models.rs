//! Server config the engine refuses: model routes, billing and the worker loop it cannot keep.

use sych_core::config::ServerConfig;

use super::read;

#[test]
fn openrouter_needs_pinned_upstreams_and_never_serves_corporate_code() {
    let personal = read("examples/personal/sych.yaml");
    let text = personal.replace(
        "      upstreams: [deepinfra/fp8, fireworks, together]\n",
        "",
    );
    let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
    assert_eq!(errors[0].path, "models.profiles.cheap_api.upstreams");
    let corporate = personal
        .replace("mode: personal", "mode: corporate")
        .replace(
            "allowed_modes: [personal_local, personal_service]",
            "allowed_modes: [corporate]",
        );
    let errors = ServerConfig::parse("sych.yaml", &corporate).expect_err("rejected");
    assert!(
        errors
            .iter()
            .any(|e| e.path == "models.providers.openrouter.allowed_modes"),
        "{errors:?}"
    );
}

#[test]
fn opencode_go_stays_on_the_go_path() {
    let personal = read("examples/personal/sych.yaml");
    let text = personal.replace(
        "https://opencode.ai/inference/go/openai/v1",
        "https://opencode.ai/inference/openai/v1",
    );
    let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
    assert_eq!(errors[0].path, "models.providers.opencode_go.endpoint");
}

#[test]
fn fallback_chains_are_finite_and_never_wait_for_a_reset() {
    let personal = read("examples/personal/sych.yaml");
    let repeated = personal.replace(
        "premium_astra: [premium_fable]",
        "premium_astra: [premium_fable, premium_fable]",
    );
    let errors = ServerConfig::parse("sych.yaml", &repeated).expect_err("rejected");
    assert_eq!(
        errors[0].path,
        "models.fallback.chains.profiles.premium_astra[1]"
    );
    let own = personal.replace(
        "premium_astra: [premium_fable]",
        "premium_astra: [premium_astra]",
    );
    let errors = ServerConfig::parse("sych.yaml", &own).expect_err("rejected");
    assert_eq!(
        errors[0].path,
        "models.fallback.chains.profiles.premium_astra"
    );
    let unknown = personal.replace(
        "premium_astra: [premium_fable]",
        "premium_astra: [premium_fabel]",
    );
    let errors = ServerConfig::parse("sych.yaml", &unknown).expect_err("rejected");
    assert_eq!(
        errors[0].path,
        "models.fallback.chains.profiles.premium_astra[0]"
    );
    // Chains are flat, so A → B → A ends after B: the router never follows B's own chain.
    let back = personal.replace(
        "premium_astra: [premium_fable]",
        "premium_astra: [premium_fable]\n        premium_fable: [premium_astra]",
    );
    ServerConfig::parse("sych.yaml", &back).expect("a mutual pair is finite");
    let selector = personal
        .replace("    cheap_go:\n", "    roles:\n")
        .replace(": cheap_go", ": roles");
    let errors = ServerConfig::parse("sych.yaml", &selector).expect_err("rejected");
    assert!(
        errors.iter().any(|e| e.path == "models.profiles.roles"),
        "{errors:?}"
    );
    let waits = personal.replace("wait_for_usage_reset: false", "wait_for_usage_reset: true");
    let errors = ServerConfig::parse("sych.yaml", &waits).expect_err("rejected");
    assert_eq!(errors[0].path, "models.fallback.wait_for_usage_reset");
}

#[test]
fn workers_concurrency_promises_only_the_single_loop() {
    let personal = read("examples/personal/sych.yaml");
    for n in ["0", "8"] {
        let text = personal.replacen("concurrency: 1", &format!("concurrency: {n}"), 1);
        let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
        assert!(
            errors.iter().any(|e| e.path == "workers.concurrency"),
            "{n}: {errors:?}"
        );
    }
}

#[test]
fn billing_must_match_the_adapter() {
    let personal = read("examples/personal/sych.yaml");
    let metered_as_plan = personal.replacen(
        "model: deepseek/deepseek-v4.1-flash\n      billing_kind: api_usage\n      enforcement: api_hard",
        "model: deepseek/deepseek-v4.1-flash\n      billing_kind: subscription_quota\n      enforcement: subscription_observable",
        1,
    );
    let plan_as_metered = personal.replacen(
        "model: deepseek-v4.1-flash\n      billing_kind: subscription_quota",
        "model: deepseek-v4.1-flash\n      billing_kind: api_usage",
        1,
    );
    for (text, path) in [
        (metered_as_plan, "models.profiles.cheap_api.billing_kind"),
        (plan_as_metered, "models.profiles.cheap_go.billing_kind"),
    ] {
        let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
        assert!(errors.iter().any(|e| e.path == path), "{errors:?}");
    }
}
