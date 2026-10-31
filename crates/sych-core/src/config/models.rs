//! Model providers, profiles, roles and fallback chains of the server config. Secrets are
//! referenced by environment variable name only.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::EnvName;
use crate::MicroUsd;

/// The only `OpenCode` Go inference base; without `/go` requests bill the Console balance.
pub const OPENCODE_GO_ENDPOINT: &str = "https://opencode.ai/inference/go/openai/v1";

/// Plan usage through Sign in with `ChatGPT` goes to the public Responses API, never to
/// `ChatGPT`'s own backend.
pub const SIWC_ENDPOINT: &str = "https://api.openai.com/v1";

/// The GLM Coding Plan endpoint; the general `/api/paas/v4` bills the Standard API instead.
pub const ZAI_CODING_ENDPOINT: &str = "https://api.z.ai/api/coding/paas/v4";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    Openrouter,
    OpenaiCompatible,
    ChatgptSubscription,
    GlmCodingPlan,
    OpencodeGo,
}

impl Adapter {
    /// The only billing and enforcement a profile of this adapter may declare.
    pub const fn billing(self) -> (BillingKind, Enforcement) {
        match self {
            Self::Openrouter | Self::OpenaiCompatible => {
                (BillingKind::ApiUsage, Enforcement::ApiHard)
            }
            Self::ChatgptSubscription | Self::GlmCodingPlan | Self::OpencodeGo => (
                BillingKind::SubscriptionQuota,
                Enforcement::SubscriptionObservable,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    SiwcText,
    SupportedPiCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    PersonalLocal,
    PersonalService,
    Corporate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub adapter: Adapter,
    /// Base URL; together with the other providers it forms the egress allowlist.
    pub endpoint: Option<String>,
    pub transport: Option<Transport>,
    pub api_key_env: Option<EnvName>,
    pub connection_ref_env: Option<EnvName>,
    pub executable_env: Option<EnvName>,
    pub allowed_modes: Vec<RunMode>,
    /// Required before a subscription profile of this provider can be used.
    pub fixed_fee: Option<FixedFee>,
    /// The owner confirmed that overage (`OpenCode` or Z.ai Extra Usage, auto top-up) is off
    /// in the account; Sych never turns it on.
    #[serde(default)]
    pub overage_disabled: bool,
}

/// Subscription fee per calendar month (UTC) and the share charged to the model pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedFee {
    pub amount_micro_usd: MicroUsd,
    pub allocated_micro_usd: MicroUsd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BillingKind {
    ApiUsage,
    SubscriptionQuota,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    ApiHard,
    SubscriptionObservable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub provider: String,
    pub model: Option<String>,
    pub model_env: Option<EnvName>,
    pub billing_kind: BillingKind,
    pub enforcement: Enforcement,
    /// `OpenRouter` upstream provider slugs, tried in this order with no hidden fallback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<String>,
    /// Bounds reasoning, which counts against the per-request output cap; omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Reviewer,
    Verifier,
    RiskAuditor,
    Explainer,
    Followup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackTrigger {
    QuotaExhausted,
    RateLimited,
    Transient,
    Timeout,
    UnsupportedCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevertPolicy {
    CooldownExpiry,
}

/// Fallback chains name profiles; keys name profiles, roles or providers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Chains {
    pub profiles: BTreeMap<String, Vec<String>>,
    pub roles: BTreeMap<Role, Vec<String>>,
    pub providers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fallback {
    pub on: Vec<FallbackTrigger>,
    #[serde(default)]
    pub chains: Chains,
    pub cooldown_seconds: u32,
    pub revert_policy: RevertPolicy,
    #[serde(default)]
    pub wait_for_usage_reset: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Models {
    pub providers: BTreeMap<String, Provider>,
    pub profiles: BTreeMap<String, Profile>,
    pub roles: BTreeMap<Role, String>,
    /// Role assignments of a `deep` manual run; a role without one keeps `roles`.
    pub deep_roles: BTreeMap<Role, String>,
    pub fallback: Option<Fallback>,
    /// Host path, relative to the config file; prices are keyed by model slug.
    pub price_catalog_file: Option<String>,
}
