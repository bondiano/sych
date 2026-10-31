//! Model profiles resolved to a callable transport for a local personal run: mode, egress,
//! model, price and credentials are checked before any request.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use std::cell::RefCell;
use std::collections::BTreeMap;
use sych_adapters::opencode_go::{self, OpencodeGo};
use sych_adapters::openrouter::{self, OpenRouter};
use sych_adapters::pi::{self, PiCli};
use sych_adapters::siwc::{self, Siwc};

use sych_core::config::{
    Adapter, EnvName, FallbackTrigger, FixedFee, Profile, Provider, ReasoningEffort, Role, RunMode,
    ServerConfig,
};
use sych_core::model::{
    Capabilities, Checkpoint, ErrorKind, Hop, Limits, ModelProvider, ModelReply, ModelRequest,
    PriceCard, PriceCatalog, ProviderError, Purpose, REVIEW_ROLES, Recheck, Route, RoutePlan,
    Router, plan,
};
use sych_core::{Redactor, RunId, Sha256Digest, Timestamp, canonical_sha256};
use sych_storage::TENANT_LOCAL;
use tokio::time::timeout;

/// Every transport behind one gateway; secrets stay inside the adapter.
pub enum Backend {
    OpenRouter(OpenRouter),
    OpencodeGo(OpencodeGo),
    Pi(PiCli),
    Siwc(Siwc),
}

impl ModelProvider for Backend {
    async fn complete(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        let call = async {
            match self {
                Self::OpenRouter(p) => p.complete(request).await,
                Self::OpencodeGo(p) => p.complete(request).await,
                Self::Pi(p) => p.complete(request).await,
                Self::Siwc(p) => p.complete(request).await,
            }
        };
        let Some(deadline) = request.deadline else {
            return call.await;
        };
        let left = deadline
            .duration_since(Timestamp::now())
            .try_into()
            .unwrap_or(Duration::ZERO);
        // Dropping the call aborts the request; it may have been sent, so the charge is unknown.
        timeout(left, call).await.unwrap_or_else(|_| {
            Err(ProviderError {
                kind: ErrorKind::Timeout,
                dispatched: true,
                message: "run deadline reached while waiting for the model".into(),
                retry_after_seconds: None,
            })
        })
    }
}

/// A profile resolved to a callable route.
pub struct Resolved {
    pub route: Route,
    pub upstreams: Vec<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub price: Option<PriceCard>,
    pub price_revision: Option<String>,
    pub fixed_fee: Option<FixedFee>,
    pub provider: Backend,
}

pub enum ResolveError {
    Invalid(String),
    Credentials(String),
}

fn var(name: &EnvName, key: &str) -> Result<String, String> {
    env::var(name.as_str()).map_err(|_| format!("{} is not set ({key})", name.as_str()))
}

/// The provider's secret named by `field`; a missing value is a credentials problem.
fn secret(name: &str, field: &str, env: Option<&EnvName>) -> Result<String, ResolveError> {
    let env =
        env.ok_or_else(|| ResolveError::Invalid(format!("provider `{name}` has no {field}")))?;
    var(env, &format!("models.providers.{name}.{field}")).map_err(ResolveError::Credentials)
}

/// The sign-in file of a `ChatGPT` plan provider, named by `connection_ref_env`.
pub fn connection_file(provider: &Provider, name: &str) -> Result<PathBuf, ResolveError> {
    secret(
        name,
        "connection_ref_env",
        provider.connection_ref_env.as_ref(),
    )
    .map(PathBuf::from)
    .map_err(|e| match e {
        ResolveError::Credentials(e) => ResolveError::Invalid(e),
        e @ ResolveError::Invalid(_) => e,
    })
}

/// Overage would turn an exhausted plan into money; only the owner can confirm it is off.
fn overage_off(provider: &Provider, name: &str) -> Result<(), ResolveError> {
    if provider.overage_disabled {
        return Ok(());
    }

    Err(ResolveError::Invalid(format!(
        "confirm that extra usage is off in the provider console, then set \
         models.providers.{name}.overage_disabled: true"
    )))
}

/// The provider endpoint, if `path` under it is inside the egress allowlist.
fn endpoint<'a>(
    server: &ServerConfig,
    provider: &'a Provider,
    name: &str,
    path: &str,
) -> Result<&'a str, ResolveError> {
    let endpoint = provider
        .endpoint
        .as_deref()
        .ok_or_else(|| ResolveError::Invalid(format!("provider `{name}` has no endpoint")))?;
    let url = format!("{}{path}", endpoint.trim_end_matches('/'));
    if !server.egress_allowed(&url) {
        return Err(ResolveError::Invalid(format!(
            "{url} is outside the egress allowlist"
        )));
    }

    Ok(endpoint)
}

/// The adapter of the profile's provider, built with its endpoint and credentials.
fn transport(
    server: &ServerConfig,
    name: &str,
    profile: &Profile,
    model: &str,
) -> Result<(&'static str, Capabilities, Backend), ResolveError> {
    use ResolveError::Invalid;
    let provider_name = &profile.provider;
    let provider = &server.models.providers[provider_name];
    let key = || secret(provider_name, "api_key_env", provider.api_key_env.as_ref());
    let invalid = |e: ProviderError| Invalid(e.message);
    Ok(match provider.adapter {
        Adapter::Openrouter => {
            let base = endpoint(server, provider, provider_name, "/chat/completions")?;
            let p = OpenRouter::new(
                base,
                key()?,
                model,
                profile.upstreams.clone(),
                profile.reasoning_effort,
            )
            .map_err(invalid)?;
            (
                "openrouter",
                openrouter::CAPABILITIES,
                Backend::OpenRouter(p),
            )
        }
        Adapter::OpencodeGo => {
            overage_off(provider, provider_name)?;
            let base = endpoint(server, provider, provider_name, "/chat/completions")?;
            let p = OpencodeGo::new(base, key()?, model).map_err(invalid)?;
            (
                "opencode_go",
                opencode_go::CAPABILITIES,
                Backend::OpencodeGo(p),
            )
        }
        Adapter::GlmCodingPlan => {
            overage_off(provider, provider_name)?;
            let base = endpoint(server, provider, provider_name, "/chat/completions")?;
            let executable = secret(
                provider_name,
                "executable_env",
                provider.executable_env.as_ref(),
            )
            .map_err(|e| match e {
                ResolveError::Credentials(e) => Invalid(e),
                e @ ResolveError::Invalid(_) => e,
            })?;
            let p = PiCli::new(
                executable.into(),
                base,
                key()?,
                model,
                profile.reasoning_effort,
            )
            .map_err(invalid)?;
            ("glm_coding_plan", pi::CAPABILITIES, Backend::Pi(p))
        }
        Adapter::ChatgptSubscription => {
            let base = endpoint(server, provider, provider_name, "/responses")?;
            let file = connection_file(provider, provider_name)?;
            let p = Siwc::new(base, siwc::auth::AUTH_BASE, file).map_err(|e| match e.kind {
                ErrorKind::Auth | ErrorKind::Entitlement => ResolveError::Credentials(e.message),
                _ => Invalid(e.message),
            })?;
            ("chatgpt_subscription", siwc::CAPABILITIES, Backend::Siwc(p))
        }
        Adapter::OpenaiCompatible => {
            return Err(Invalid(format!(
                "profile `{name}` uses an adapter that is not implemented"
            )));
        }
    })
}

/// Resolves profile `name` for a run in `mode`; a provider not allowed there is refused.
pub fn resolve(
    config: &Path,
    server: &ServerConfig,
    name: &str,
    mode: RunMode,
) -> Result<Resolved, ResolveError> {
    use ResolveError::Invalid;
    let profile = server
        .models
        .profiles
        .get(name)
        .ok_or_else(|| Invalid(format!("unknown model profile `{name}`")))?;
    let provider_name = &profile.provider;
    let provider = &server.models.providers[provider_name];
    if !provider.allowed_modes.contains(&mode) {
        return Err(Invalid(format!(
            "provider `{provider_name}` is not allowed in {mode:?} mode"
        )));
    }

    if (profile.billing_kind, profile.enforcement) != provider.adapter.billing() {
        return Err(Invalid(format!(
            "profile `{name}` declares billing that its provider adapter does not have"
        )));
    }

    let model = match (&profile.model, &profile.model_env) {
        (Some(model), _) => model.clone(),
        (None, Some(env)) => {
            var(env, &format!("models.profiles.{name}.model_env")).map_err(Invalid)?
        }
        (None, None) => return Err(Invalid(format!("profile `{name}` has no model"))),
    };
    let (adapter, capabilities, backend) = transport(server, name, profile, &model)?;
    let (price, price_revision) = price(config, server, &model)?;
    Ok(Resolved {
        route: Route {
            profile: name.into(),
            provider: provider_name.clone(),
            adapter: adapter.into(),
            model,
            billing: profile.billing_kind,
            enforcement: profile.enforcement,
            capabilities,
        },
        upstreams: profile.upstreams.clone(),
        reasoning_effort: profile.reasoning_effort,
        price,
        price_revision,
        fixed_fee: provider.fixed_fee,
        provider: backend,
    })
}

/// The model's price card; a metered profile without one is refused by the gateway.
fn price(
    config: &Path,
    server: &ServerConfig,
    model: &str,
) -> Result<(Option<PriceCard>, Option<String>), ResolveError> {
    let Some(file) = &server.models.price_catalog_file else {
        return Ok((None, None));
    };
    let invalid = ResolveError::Invalid;
    let path = config.parent().unwrap_or(Path::new(".")).join(file);
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let catalog = PriceCatalog::parse(&path.display().to_string(), &text)
        .map_err(|e| invalid(e.to_string()))?;
    Ok((catalog.prices.get(model).copied(), Some(catalog.revision)))
}

/// The route plan of one run and every profile it names: resolved, or why it cannot be used.
pub struct Routing {
    pub plan: RoutePlan,
    pub profiles: BTreeMap<String, Result<Resolved, String>>,
    pub on: Vec<FallbackTrigger>,
    pub cooldown_seconds: u32,
    pub redactor: Redactor,
    /// Digest of `models`; a dependency of every checkpoint of the run.
    pub models_revision: String,
    /// Resolves a profile again before each hop: mode, billing, egress and credentials still
    /// hold, and the route is the one planned.
    pub recheck: Box<Recheck>,
}

impl Routing {
    /// The router of one run and purpose over these resolved profiles.
    pub fn router<'a, L>(
        &'a self,
        ledger: &'a L,
        run_id: RunId,
        purpose: Purpose,
        limits: Limits,
        deadline: Option<Timestamp>,
        reuse: BTreeMap<Sha256Digest, Checkpoint>,
    ) -> Router<'a, Backend, L> {
        Router {
            hops: self
                .profiles
                .iter()
                .map(|(name, resolved)| {
                    let hop = resolved.as_ref().map_err(Clone::clone).map(|r| Hop {
                        provider: &r.provider,
                        route: r.route.clone(),
                        price: r.price,
                        fixed_fee: r.fixed_fee,
                    });
                    (name.clone(), hop)
                })
                .collect(),
            plan: self.plan.clone(),
            on: self.on.clone(),
            cooldown_seconds: self.cooldown_seconds,
            ledger,
            tenant_id: TENANT_LOCAL.into(),
            run_id,
            purpose,
            limits,
            deadline,
            now: Timestamp::now,
            redactor: &self.redactor,
            cooldowns: RefCell::default(),
            reuse,
            models_revision: self.models_revision.clone(),
            eligible: Some(&*self.recheck),
        }
    }
}

/// Plans a review run: the pinned `selected` profile must resolve itself; with role routing
/// the reviewer's chain needs at least one usable profile. Unusable fallbacks stay in the
/// plan with their reason and are skipped.
pub fn routing(
    config: &Path,
    server: &ServerConfig,
    selected: Option<&str>,
    deep: bool,
    mode: RunMode,
) -> Result<Routing, ResolveError> {
    let plan =
        plan(&server.models, selected, deep, &REVIEW_ROLES).map_err(ResolveError::Invalid)?;
    let mut profiles = BTreeMap::new();
    if let Some(name) = selected {
        profiles.insert(name.to_owned(), Ok(resolve(config, server, name, mode)?));
    }

    for name in plan.chains.values().flatten() {
        if !profiles.contains_key(name) {
            let resolved = resolve(config, server, name, mode).map_err(|e| match e {
                ResolveError::Invalid(e) | ResolveError::Credentials(e) => e,
            });
            profiles.insert(name.clone(), resolved);
        }
    }
    let reviewer = &plan.chains[&Role::Reviewer];
    if reviewer.iter().all(|n| profiles[n].is_err()) {
        let reasons: Vec<String> = reviewer
            .iter()
            .filter_map(|n| profiles[n].as_ref().err().map(|e| format!("{n}: {e}")))
            .collect();
        return Err(ResolveError::Invalid(format!(
            "no profile of the reviewer chain can be used: {}",
            reasons.join("; ")
        )));
    }

    let planned: BTreeMap<String, Route> = profiles
        .iter()
        .filter_map(|(n, r)| Some((n.clone(), r.as_ref().ok()?.route.clone())))
        .collect();
    let (config, server_copy) = (config.to_path_buf(), server.clone());
    let recheck = move |name: &str| {
        let resolved = resolve(&config, &server_copy, name, mode).map_err(|e| match e {
            ResolveError::Invalid(e) | ResolveError::Credentials(e) => e,
        })?;
        if planned.get(name) == Some(&resolved.route) {
            Ok(())
        } else {
            Err("the profile resolves to another route than at the start of the run".into())
        }
    };
    let fallback = server.models.fallback.as_ref();
    Ok(Routing {
        plan,
        profiles,
        on: fallback.map(|f| f.on.clone()).unwrap_or_default(),
        cooldown_seconds: fallback.map_or(0, |f| f.cooldown_seconds),
        redactor: server.redaction.redactor().map_err(ResolveError::Invalid)?,
        models_revision: canonical_sha256(&server.models)
            .map_err(|e| ResolveError::Invalid(e.to_string()))?
            .to_string(),
        recheck: Box::new(recheck),
    })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use sych_core::model::{Message, Speaker};
    use sych_core::{AttemptId, TaskId};

    use crate::storage::block_on;

    use super::*;

    #[test]
    fn the_run_deadline_bounds_the_wait_and_leaves_the_charge_unknown() {
        // Accepts the connection into the backlog and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let endpoint = format!("http://{}/api/v1", listener.local_addr().expect("addr"));
        let p = OpenRouter::new(
            &endpoint,
            "sk-test-not-real".into(),
            "deepseek/deepseek-v4.1-flash",
            vec!["deepseek".into()],
            None,
        )
        .map_err(|e| e.message)
        .expect("adapter");
        let request = ModelRequest {
            attempt_id: AttemptId::new_v7(),
            task_id: TaskId::new_v7(),
            model: "deepseek/deepseek-v4.1-flash".into(),
            messages: vec![Message {
                speaker: Speaker::User,
                content: "task".into(),
            }],
            schema_name: "probe".into(),
            schema: serde_json::json!({ "type": "object" }),
            max_output_tokens: Some(100),
            deadline: Timestamp::now()
                .checked_add(Duration::from_millis(300))
                .ok(),
        };
        let started = Instant::now();
        let reply = block_on(Backend::OpenRouter(p).complete(&request)).expect("runtime");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "not the 90 s transport timeout"
        );
        let error = reply.expect_err("no reply");
        assert_eq!(error.kind, ErrorKind::Timeout);
        assert!(error.dispatched, "the charge stays unknown");
        drop(listener);
    }
}
