//! Role routing and the bounded fallback loop (spec §16.7, roadmap M1.13). A run fixes one
//! [`RoutePlan`]: per role the primary profile, then exactly one chain chosen by the priority
//! profile → role → provider. A task visits each profile of its chain at most once; only the
//! configured retryable failures move on, and every hop spends the same run budget in the
//! ledger. A failed profile cools down and is tried again after the cooldown.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::gateway::output_schema;
use super::{
    AttemptRecord, Checkpoint, CheckpointDeps, Denied, ErrorKind, Gateway, Ledger, Limits,
    ModelProvider, PriceCard, Prompt, Purpose, Route, TaskFailure,
};
use crate::config::{FallbackTrigger, FixedFee, Models, Role};
use crate::{Redactor, RunId, SchemaVersion, Sha256Digest, Timestamp, canonical_sha256};

/// Ordered profiles per role, primary first; fixed for the whole run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutePlan {
    pub chains: BTreeMap<Role, Vec<String>>,
    /// Digest of `chains`, recorded with the run.
    pub revision: String,
}

/// Re-checks a profile by name before its hop; `Err` is why it cannot be used now.
pub type Recheck = dyn Fn(&str) -> Result<(), String> + Send + Sync;

/// Roles a review run calls.
pub const REVIEW_ROLES: [Role; 3] = [Role::Reviewer, Role::Verifier, Role::RiskAuditor];

/// The chain of `role` whose primary is `primary`: an exact profile chain wins over the
/// role's, which wins over the provider default. A profile the operator pinned skips the role
/// chain. Chains are flat: a fallback profile's own chain is not followed, so no hop restarts
/// the loop.
pub fn chain(models: &Models, role: Option<Role>, primary: &str) -> Vec<String> {
    let chains = models.fallback.as_ref().map(|f| &f.chains);
    let provider = models.profiles.get(primary).map(|p| p.provider.as_str());
    let fallback = chains.and_then(|c| {
        c.profiles
            .get(primary)
            .or_else(|| role.and_then(|r| c.roles.get(&r)))
            .or_else(|| provider.and_then(|p| c.providers.get(p)))
    });
    let mut out = vec![primary.to_owned()];
    for name in fallback.into_iter().flatten() {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// One plan for `roles`: every role starts at the pinned `selected` profile when given, else
/// at its `deep_roles` profile on a `deep` run, else at its assigned profile, else at the
/// reviewer's.
pub fn plan(
    models: &Models,
    selected: Option<&str>,
    deep: bool,
    roles: &[Role],
) -> Result<RoutePlan, String> {
    let mut chains = BTreeMap::new();
    for &role in roles {
        let primary = match selected {
            Some(name) => name,
            None => deep
                .then(|| models.deep_roles.get(&role))
                .flatten()
                .or_else(|| models.roles.get(&role))
                .or_else(|| models.roles.get(&Role::Reviewer))
                .ok_or_else(|| format!("models.roles assigns no profile to {role:?}"))?,
        };
        if !models.profiles.contains_key(primary) {
            return Err(format!("unknown model profile `{primary}`"));
        }

        let by_role = selected.is_none().then_some(role);
        chains.insert(role, chain(models, by_role, primary));
    }
    let revision = canonical_sha256(&chains)
        .map_err(|e| e.to_string())?
        .to_string();
    Ok(RoutePlan { chains, revision })
}

/// A resolved profile the router may dispatch to.
pub struct Hop<'a, P> {
    pub provider: &'a P,
    pub route: Route,
    pub price: Option<PriceCard>,
    pub fixed_fee: Option<FixedFee>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HopOutcome {
    Answered,
    /// Failed with a configured fallback trigger; the next profile was tried.
    FellBack {
        failure: TaskFailure,
    },
    /// Failed in a way no other route may heal; the task stopped here.
    Stopped {
        failure: TaskFailure,
    },
    Skipped {
        reason: String,
    },
    /// A checkpoint with the same dependencies answered; no call was made.
    Reused {
        run_id: RunId,
    },
}

/// One visited profile of a task, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HopRecord {
    pub role: Role,
    pub profile: String,
    #[serde(flatten)]
    pub outcome: HopOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedOutcome<T> {
    pub value: Result<T, TaskFailure>,
    pub attempts: Vec<AttemptRecord>,
    pub hops: Vec<HopRecord>,
    /// The run whose checkpoint answered this task.
    pub reused_from: Option<RunId>,
    /// Why the answer could not be checkpointed; a successor will ask again.
    pub unkept: Option<String>,
}

pub struct Router<'a, P, L> {
    /// Every profile of the plan: resolved, or why it cannot be used in this run.
    pub hops: BTreeMap<String, Result<Hop<'a, P>, String>>,
    pub plan: RoutePlan,
    pub on: Vec<FallbackTrigger>,
    pub cooldown_seconds: u32,
    pub ledger: &'a L,
    pub tenant_id: String,
    pub run_id: RunId,
    pub purpose: Purpose,
    pub limits: Limits,
    pub deadline: Option<Timestamp>,
    pub now: fn() -> Timestamp,
    pub redactor: &'a Redactor,
    /// Profile → when it may be tried again; empty at the start of a run.
    pub cooldowns: RefCell<BTreeMap<String, Timestamp>>,
    /// The parent run's checkpoints a successor may reuse; empty otherwise.
    pub reuse: BTreeMap<Sha256Digest, Checkpoint>,
    /// Digest of the trusted model config, a dependency of every checkpoint.
    pub models_revision: String,
    /// Re-checks a profile's mode, billing, egress and credentials right before its hop.
    pub eligible: Option<&'a Recheck>,
}

fn trigger(kind: ErrorKind) -> Option<FallbackTrigger> {
    match kind {
        ErrorKind::QuotaExhausted => Some(FallbackTrigger::QuotaExhausted),
        ErrorKind::RateLimited => Some(FallbackTrigger::RateLimited),
        ErrorKind::Transient => Some(FallbackTrigger::Transient),
        ErrorKind::Timeout => Some(FallbackTrigger::Timeout),
        ErrorKind::Unsupported => Some(FallbackTrigger::UnsupportedCapability),
        // Auth, entitlement, config, billing and cancellation are not healed by another route.
        _ => None,
    }
}

impl<P: ModelProvider, L: Ledger> Router<'_, P, L> {
    /// Whether `failure` lets the task move to the next profile.
    fn falls_back(&self, failure: &TaskFailure) -> bool {
        match failure {
            TaskFailure::Provider { kind, .. } => {
                trigger(*kind).is_some_and(|t| self.on.contains(&t))
            }
            // This route cannot bound or bill the call; another route reserves anew.
            TaskFailure::Budget {
                denied: Denied::NoPriceBound | Denied::FixedFeeUnknown | Denied::BillingMismatch,
            } => true,
            // Shared caps, deadline, schema, cut or refused output: no route changes them.
            _ => false,
        }
    }

    fn cool_down(&self, profile: &str, failure: &TaskFailure, now: Timestamp) {
        let TaskFailure::Provider {
            retry_after_seconds,
            ..
        } = failure
        else {
            return;
        };
        let seconds = retry_after_seconds.unwrap_or(0).max(self.cooldown_seconds);
        if let Ok(until) = now.checked_add(Duration::from_secs(seconds.into())) {
            self.cooldowns
                .borrow_mut()
                .insert(profile.to_owned(), until);
        }
    }

    /// One structured task for `role` along its chain; never waits for a quota reset.
    pub async fn structured<T: Serialize + DeserializeOwned + JsonSchema>(
        &self,
        role: Role,
        prompt: &Prompt,
        schema_name: &str,
    ) -> RoutedOutcome<T> {
        self.route(role, prompt, schema_name, self.limits).await
    }

    /// [`Self::structured`] under [`Limits::reserving`] `kept` attempts for later passes; a
    /// search keeps at least the verifier's, so it cannot leave candidates without a check.
    pub async fn leaving<T: Serialize + DeserializeOwned + JsonSchema>(
        &self,
        role: Role,
        prompt: &Prompt,
        schema_name: &str,
        kept: u32,
    ) -> RoutedOutcome<T> {
        let limits = self.limits.reserving(kept);
        self.route(role, prompt, schema_name, limits).await
    }

    fn deps<T: JsonSchema>(
        &self,
        role: Role,
        chain: &[String],
        prompt: &Prompt,
        schema_name: &str,
    ) -> Option<CheckpointDeps> {
        Some(CheckpointDeps {
            template: prompt.template.clone(),
            schema_name: schema_name.to_owned(),
            role,
            chain: chain.to_vec(),
            messages_sha256: canonical_sha256(&prompt.messages(self.redactor)).ok()?,
            schema_sha256: canonical_sha256(&output_schema::<T>()).ok()?,
            models_revision: self.models_revision.clone(),
        })
    }

    /// The parent's answer to a task with exactly these dependencies; one that no longer
    /// parses is not reused, and the task runs instead.
    fn reused<T: DeserializeOwned>(
        &self,
        deps: Option<&CheckpointDeps>,
        key: Option<&Sha256Digest>,
    ) -> Option<(T, &Checkpoint)> {
        let c = self.reuse.get(key?).filter(|c| Some(&c.deps) == deps)?;
        Some((serde_json::from_value(c.output.clone()).ok()?, c))
    }

    /// The outcome of a task its parent's checkpoint answered; kept again for this run.
    async fn answered_by<T>(
        &self,
        c: &Checkpoint,
        role: Role,
        value: T,
        mut out: RoutedOutcome<T>,
    ) -> RoutedOutcome<T> {
        out.unkept = self.ledger.checkpoint(c).await.err().map(|e| e.to_string());
        out.hops.push(HopRecord {
            role,
            profile: c.profile.clone(),
            outcome: HopOutcome::Reused { run_id: c.run_id },
        });
        out.reused_from = Some(c.run_id);
        out.value = Ok(value);
        out
    }

    /// Checkpoints this run's accepted answer to a task with these dependencies; why not,
    /// when it could not be kept.
    async fn keep<T: Serialize>(
        &self,
        deps: Option<&CheckpointDeps>,
        key: Option<&Sha256Digest>,
        profile: &str,
        value: &T,
    ) -> Option<String> {
        let (Some(deps), Some(key)) = (deps, key) else {
            return Some("the task's dependencies cannot be hashed".into());
        };
        let output = match serde_json::to_value(value) {
            Ok(output) => output,
            Err(e) => return Some(format!("the answer cannot be stored: {e}")),
        };
        let checkpoint = Checkpoint {
            schema_version: SchemaVersion,
            key: key.clone(),
            deps: deps.clone(),
            profile: profile.to_owned(),
            run_id: self.run_id,
            output,
        };
        self.ledger
            .checkpoint(&checkpoint)
            .await
            .err()
            .map(|e| e.to_string())
    }

    /// The resolved hop of `profile` if it may take a task now: not cooling down, still
    /// eligible, and able to answer structured.
    fn usable(&self, profile: &str) -> Result<&Hop<'_, P>, String> {
        let now = (self.now)();
        if let Some(until) = self.cooldowns.borrow().get(profile).filter(|t| **t > now) {
            return Err(format!("cooling down until {until}"));
        }

        let h = match self.hops.get(profile) {
            Some(Ok(h)) => h,
            Some(Err(reason)) => return Err(reason.clone()),
            None => return Err("not resolved for this run".into()),
        };
        if let Some(eligible) = self.eligible {
            eligible(profile)?;
        }

        // Every routed task is structured; a route without it would only fail.
        if !h.route.capabilities.structured_output {
            return Err("the route has no structured output".into());
        }

        Ok(h)
    }

    async fn route<T: Serialize + DeserializeOwned + JsonSchema>(
        &self,
        role: Role,
        prompt: &Prompt,
        schema_name: &str,
        limits: Limits,
    ) -> RoutedOutcome<T> {
        let mut out = RoutedOutcome {
            value: Err(TaskFailure::NoEligibleRoute {
                reasons: Vec::new(),
            }),
            attempts: Vec::new(),
            hops: Vec::new(),
            reused_from: None,
            unkept: None,
        };
        let Some(chain) = self.plan.chains.get(&role) else {
            out.value = Err(TaskFailure::NoEligibleRoute {
                reasons: vec![format!("no route is planned for {role:?}")],
            });
            return out;
        };
        let deps = self.deps::<T>(role, chain, prompt, schema_name);
        let key = deps.as_ref().and_then(CheckpointDeps::key);
        if let Some((value, c)) = self.reused::<T>(deps.as_ref(), key.as_ref()) {
            return self.answered_by(c, role, value, out).await;
        }

        let mut reasons = Vec::new();
        let mut last = None;
        for profile in chain {
            let hop = |outcome| HopRecord {
                role,
                profile: profile.clone(),
                outcome,
            };
            let resolved = self.usable(profile);
            let h = match resolved {
                Ok(h) => h,
                Err(reason) => {
                    reasons.push(format!("{profile}: {reason}"));
                    out.hops.push(hop(HopOutcome::Skipped { reason }));
                    continue;
                }
            };
            let gateway = Gateway {
                provider: h.provider,
                ledger: self.ledger,
                route: h.route.clone(),
                price: h.price,
                fixed_fee: h.fixed_fee,
                tenant_id: self.tenant_id.clone(),
                run_id: self.run_id,
                purpose: self.purpose,
                limits,
                deadline: self.deadline,
                now: self.now,
                redactor: self.redactor,
            };
            let task = gateway.structured::<T>(prompt, schema_name).await;
            out.attempts.extend(task.attempts);
            let failure = match task.value {
                Ok(value) => {
                    out.unkept = self
                        .keep(deps.as_ref(), key.as_ref(), profile, &value)
                        .await;
                    out.hops.push(hop(HopOutcome::Answered));
                    out.value = Ok(value);
                    return out;
                }
                Err(failure) => failure,
            };
            if !self.falls_back(&failure) {
                out.hops.push(hop(HopOutcome::Stopped {
                    failure: failure.clone(),
                }));
                out.value = Err(failure);
                return out;
            }

            self.cool_down(profile, &failure, (self.now)());
            out.hops.push(hop(HopOutcome::FellBack {
                failure: failure.clone(),
            }));
            last = Some(failure);
        }
        out.value = Err(last.unwrap_or(TaskFailure::NoEligibleRoute { reasons }));
        out
    }
}
