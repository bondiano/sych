//! Validation of the trusted `models` section: providers, profiles, roles and fallback chains.

use super::server::{Mode, ServerConfig};
use super::{
    Adapter, Fallback, Models, OPENCODE_GO_ENDPOINT, Problems, Provider, Role, RunMode,
    SIWC_ENDPOINT, Transport, ZAI_CODING_ENDPOINT, endpoint,
};

impl ServerConfig {
    pub(super) fn validate_models(&self, p: &mut Problems<'_>) {
        let m = &self.models;
        for (name, provider) in &m.providers {
            self.validate_provider(p, name, provider);
        }
        for (name, profile) in &m.profiles {
            let path = format!("models.profiles.{name}");
            p.check(
                m.providers.contains_key(&profile.provider),
                format!("{path}.provider"),
                &format!("unknown provider `{}`", profile.provider),
            );
            let openrouter =
                m.providers.get(&profile.provider).map(|p| p.adapter) == Some(Adapter::Openrouter);
            p.check(
                openrouter != profile.upstreams.is_empty(),
                format!("{path}.upstreams"),
                "required for an openrouter profile and only allowed there",
            );
            p.check(
                profile.model.is_some() != profile.model_env.is_some(),
                &path,
                "set exactly one of `model` and `model_env`",
            );
        }
        for (name, profile) in &m.profiles {
            // `--models fake|roles` are selectors, so a profile with that name could never be picked.
            p.check(
                !matches!(name.as_str(), "fake" | "roles"),
                format!("models.profiles.{name}"),
                "`fake` and `roles` are model selectors, not profile names",
            );
            let Some(provider) = m.providers.get(&profile.provider) else {
                continue;
            };
            // A mislabelled metered route would reserve no money for a billed request.
            p.check(
                (profile.billing_kind, profile.enforcement) == provider.adapter.billing(),
                format!("models.profiles.{name}.billing_kind"),
                "billing_kind and enforcement must match the provider adapter: api_usage + \
                 api_hard for a metered API, subscription_quota + subscription_observable for a \
                 plan",
            );
        }
        let profile = |p: &mut Problems<'_>, path: String, name: &str| {
            p.check(
                m.profiles.contains_key(name),
                path,
                &format!("unknown profile `{name}`"),
            );
        };
        for (role, name) in &m.roles {
            profile(p, format!("models.roles.{}", role_key(*role)), name);
        }
        for (role, name) in &m.deep_roles {
            profile(p, format!("models.deep_roles.{}", role_key(*role)), name);
        }
        if let Some(fallback) = &m.fallback {
            Self::validate_fallback(m, p, fallback);
        }
    }

    fn validate_fallback(m: &Models, p: &mut Problems<'_>, fallback: &Fallback) {
        let profile = |p: &mut Problems<'_>, path: String, name: &str| {
            p.check(
                m.profiles.contains_key(name),
                path,
                &format!("unknown profile `{name}`"),
            );
        };
        let chains = &fallback.chains;
        for (key, chain) in &chains.profiles {
            profile(p, format!("models.fallback.chains.profiles.{key}"), key);
            p.check(
                !chain.contains(key),
                format!("models.fallback.chains.profiles.{key}"),
                "a profile cannot fall back to itself",
            );
        }
        for key in chains.providers.keys() {
            p.check(
                m.providers.contains_key(key),
                format!("models.fallback.chains.providers.{key}"),
                &format!("unknown provider `{key}`"),
            );
        }
        let all = chains
            .profiles
            .iter()
            .map(|(k, c)| (format!("profiles.{k}"), c))
            .chain(
                chains
                    .roles
                    .iter()
                    .map(|(r, c)| (format!("roles.{}", role_key(*r)), c)),
            )
            .chain(
                chains
                    .providers
                    .iter()
                    .map(|(k, c)| (format!("providers.{k}"), c)),
            );
        for (key, chain) in all {
            for (i, name) in chain.iter().enumerate() {
                profile(p, format!("models.fallback.chains.{key}[{i}]"), name);
                p.check(
                    !chain[..i].contains(name),
                    format!("models.fallback.chains.{key}[{i}]"),
                    &format!("`{name}` is already in this chain"),
                );
            }
        }
        // The router never parks a job until a quota resets.
        p.check(
            !fallback.wait_for_usage_reset,
            "models.fallback.wait_for_usage_reset",
            "waiting for a usage reset is not supported; the next route or a gap is used",
        );
    }

    fn validate_provider(&self, p: &mut Problems<'_>, name: &str, provider: &Provider) {
        let path = format!("models.providers.{name}");
        if let Some(endpoint) = &provider.endpoint {
            p.check(
                endpoint::callable(endpoint).is_some(),
                format!("{path}.endpoint"),
                "must be an https:// URL or loopback http://, without userinfo",
            );
        }

        p.check(
            !provider.allowed_modes.is_empty(),
            format!("{path}.allowed_modes"),
            "must list at least one mode",
        );
        if let Some(fee) = provider.fixed_fee {
            p.check(
                fee.allocated_micro_usd <= fee.amount_micro_usd,
                format!("{path}.fixed_fee.allocated_micro_usd"),
                "exceeds amount_micro_usd",
            );
        }

        let pinned = |p: &mut Problems<'_>, base: &str| {
            p.check(
                provider
                    .endpoint
                    .as_deref()
                    .is_some_and(|e| e == base || endpoint::is_loopback_http(e)),
                format!("{path}.endpoint"),
                &format!("must be {base}"),
            );
        };
        match provider.adapter {
            Adapter::OpencodeGo => pinned(p, OPENCODE_GO_ENDPOINT),
            Adapter::GlmCodingPlan => {
                pinned(p, ZAI_CODING_ENDPOINT);
                p.check(
                    provider.transport == Some(Transport::SupportedPiCli),
                    format!("{path}.transport"),
                    "the Coding Plan is only used through the supported Pi CLI",
                );
                p.check(
                    provider.executable_env.is_some(),
                    format!("{path}.executable_env"),
                    "names the variable that holds the pinned Pi executable",
                );
            }
            Adapter::ChatgptSubscription => {
                pinned(p, SIWC_ENDPOINT);
                p.check(
                    provider.transport == Some(Transport::SiwcText),
                    format!("{path}.transport"),
                    "the ChatGPT plan is only used through text-only Sign in with ChatGPT",
                );
                p.check(
                    provider.connection_ref_env.is_some() && provider.api_key_env.is_none(),
                    format!("{path}.connection_ref_env"),
                    "names the variable that holds the sign-in file; an API key is another bill",
                );
            }
            _ => {}
        }

        // A personal plan login is the owner's alone: no hosted bot, no corporate code.
        let subscription = matches!(
            provider.adapter,
            Adapter::ChatgptSubscription | Adapter::GlmCodingPlan | Adapter::OpencodeGo
        );
        p.check(
            !subscription || provider.allowed_modes == [RunMode::PersonalLocal],
            format!("{path}.allowed_modes"),
            "a personal subscription is only allowed in personal_local",
        );
        let corporate = provider.allowed_modes.contains(&RunMode::Corporate);
        // Corporate code goes only to an approved VPC endpoint.
        p.check(
            !(corporate && provider.adapter == Adapter::Openrouter),
            format!("{path}.allowed_modes"),
            "public OpenRouter is not allowed in corporate mode",
        );
        p.check(
            self.mode == Mode::Corporate || !corporate,
            format!("{path}.allowed_modes"),
            "corporate mode is not available in a personal config",
        );
    }
}

fn role_key(role: Role) -> &'static str {
    match role {
        Role::Reviewer => "reviewer",
        Role::Verifier => "verifier",
        Role::RiskAuditor => "risk_auditor",
        Role::Explainer => "explainer",
        Role::Followup => "followup",
    }
}
