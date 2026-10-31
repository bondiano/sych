//! Price bounds, budget admission and the ledger contract.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{AttemptKind, Message, ObservedUsage, Purpose, Route};
use crate::config::{Budget, Budgets, ConfigError, FixedFee, parse_yaml};
use crate::{AttemptId, MicroUsd, RunId, SchemaVersion, Timestamp};

/// Output tokens one request may produce.
pub const REQUEST_OUTPUT_TOKENS: u32 = 2_000;

impl Limits {
    /// The output cap of one request: the budget's, or the default.
    pub fn request_output_tokens(&self) -> u32 {
        self.run
            .max_output_tokens_per_request
            .unwrap_or(REQUEST_OUTPUT_TOKENS)
    }

    /// The caps a pass sees when `attempts` later calls must still fit: those attempts and
    /// their share of the run money and tokens stay for them.
    #[must_use]
    pub fn reserving(mut self, attempts: u32) -> Self {
        if let Some(cap) = self.run.max_attempts {
            let kept = attempts.min(cap);
            if let Some(money) = &mut self.run.max_cost_micro_usd {
                money.0 -= money.0 / u64::from(cap.max(1)) * u64::from(kept);
            }

            for tokens in [
                &mut self.run.max_input_tokens,
                &mut self.run.max_output_tokens,
            ]
            .into_iter()
            .flatten()
            {
                *tokens -= *tokens / cap.max(1) * kept;
            }

            self.run.max_attempts = Some(cap - kept);
        }

        self
    }

    /// Attempts the passes before discovery leave it: one per discovery call, at most half
    /// of the cap without the verifier's attempt, so the required obligations keep the rest.
    pub fn discovery_share(&self, calls: usize) -> u32 {
        let calls = u32::try_from(calls).unwrap_or(u32::MAX);
        self.run
            .max_attempts
            .map_or(0, |cap| calls.min(cap.saturating_sub(1) / 2))
    }
}
const MESSAGE_OVERHEAD_TOKENS: u64 = 16;

/// List prices in micro-USD per million tokens; reasoning is billed as output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCard {
    pub input_per_mtok: u64,
    pub output_per_mtok: u64,
}

impl PriceCard {
    /// Rounded up; no cache discount is assumed.
    pub fn cost(&self, input_tokens: u64, output_tokens: u64) -> MicroUsd {
        let total = (u128::from(input_tokens) * u128::from(self.input_per_mtok))
            .saturating_add(u128::from(output_tokens) * u128::from(self.output_per_mtok));
        MicroUsd(u64::try_from(total.div_ceil(1_000_000)).unwrap_or(u64::MAX))
    }
}

/// `price_catalog_file`: a dated revision of the prices the operator accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCatalog {
    pub schema_version: SchemaVersion,
    pub revision: String,
    pub prices: BTreeMap<String, PriceCard>,
}

impl PriceCatalog {
    pub fn parse(file: &str, text: &str) -> Result<Self, ConfigError> {
        parse_yaml(file, text)
    }
}

/// Upper bound on input tokens. shortcut: assumes at most one token per UTF-8 byte (true for
/// byte-level BPE tokenizers); use the provider's tokenizer if a route ever breaks that.
pub fn input_token_bound(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| m.content.len() as u64 + MESSAGE_OVERHEAD_TOKENS)
        .sum()
}

/// Hard caps for one run purpose plus the tenant day and month money pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub run: Budget,
    pub daily_money: Option<MicroUsd>,
    pub monthly_money: Option<MicroUsd>,
}

impl Limits {
    pub fn new(budgets: &Budgets, purpose: Purpose) -> Self {
        let run = match purpose {
            Purpose::Analysis => budgets.analysis,
            Purpose::Tour => budgets.tour,
            Purpose::Followup => budgets.followup_session,
        };
        Self {
            run,
            daily_money: budgets.tenant_daily.max_cost_micro_usd,
            monthly_money: budgets.tenant_monthly.max_cost_micro_usd,
        }
    }
}

/// A charge Sych holds before dispatch; it stays held until settled with a known amount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub attempt_id: AttemptId,
    pub tenant_id: String,
    pub run_id: RunId,
    pub purpose: Purpose,
    pub kind: AttemptKind,
    pub template: String,
    pub route: Route,
    pub money: MicroUsd,
    pub input_tokens: u64,
    /// `None` when the route has no enforced output cap (`subscription_observable`).
    pub output_tokens: Option<u64>,
    /// Subscription fee of the route's provider; its allocation is charged once per month.
    pub fixed_fee: Option<FixedFee>,
    pub at: Timestamp,
}

/// What the ledger already holds: spent plus active reservations plus unknown charges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Committed {
    pub run_attempts: u32,
    pub run_money: MicroUsd,
    pub run_input_tokens: u64,
    pub run_output_tokens: u64,
    pub day_money: MicroUsd,
    pub month_money: MicroUsd,
    /// Fee this reservation would add because the month has none recorded yet.
    pub fee_due: MicroUsd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Denied {
    Attempts {
        cap: u32,
    },
    Money {
        account: String,
        cap: MicroUsd,
        committed: MicroUsd,
        requested: MicroUsd,
    },
    InputTokens {
        cap: u32,
        committed: u64,
        requested: u64,
    },
    OutputTokens {
        cap: u32,
        committed: u64,
        requested: u64,
    },
    /// No price card or no enforced output cap: a hard money bound cannot be claimed.
    NoPriceBound,
    /// A subscription without an accepted fee/allocation; unknown is not zero.
    FixedFeeUnknown,
    /// An earlier unexpected charge blocks further subscription-only calls on this provider.
    BillingMismatch,
    Deadline,
}

/// The budget invariant for one new reservation.
pub fn admit(limits: &Limits, c: &Committed, r: &Reservation) -> Result<(), Denied> {
    if let Some(cap) = limits.run.max_attempts
        && c.run_attempts >= cap
    {
        return Err(Denied::Attempts { cap });
    }

    let money = |account: &str, cap: Option<MicroUsd>, committed: MicroUsd, extra: MicroUsd| {
        let requested = MicroUsd(r.money.0.saturating_add(extra.0));
        match cap {
            Some(cap) if committed.0.saturating_add(requested.0) > cap.0 => Err(Denied::Money {
                account: account.into(),
                cap,
                committed,
                requested,
            }),
            _ => Ok(()),
        }
    };
    money(
        "run",
        limits.run.max_cost_micro_usd,
        c.run_money,
        MicroUsd(0),
    )?;
    money("day", limits.daily_money, c.day_money, MicroUsd(0))?;
    money("month", limits.monthly_money, c.month_money, c.fee_due)?;
    if let Some(cap) = limits.run.max_input_tokens
        && c.run_input_tokens + r.input_tokens > u64::from(cap)
    {
        return Err(Denied::InputTokens {
            cap,
            committed: c.run_input_tokens,
            requested: r.input_tokens,
        });
    }

    if let (Some(cap), Some(requested)) = (limits.run.max_output_tokens, r.output_tokens)
        && c.run_output_tokens + requested > u64::from(cap)
    {
        return Err(Denied::OutputTokens {
            cap,
            committed: c.run_output_tokens,
            requested,
        });
    }

    Ok(())
}

/// How an attempt ends in the ledger; applied once per attempt id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Settlement {
    /// The request never left Sych; the reservation is freed.
    Released,
    /// `charge` is the provider-reported or price-computed money; `None` on a subscription that
    /// reports none. A charge above the reservation is recorded as a billing mismatch.
    Settled {
        usage: ObservedUsage,
        charge: Option<MicroUsd>,
        reference: Option<MicroUsd>,
        resolved_model: Option<String>,
        generation_id: Option<String>,
    },
    /// Dispatched without a known outcome: the reservation stays held (`UNKNOWN_CHARGE`).
    Unknown { usage: ObservedUsage },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("ledger: {0}")]
pub struct LedgerError(pub String);

/// Durable, atomic money and attempt accounting shared by every route.
#[allow(async_fn_in_trait)] // single-threaded runtime; no Send bound is needed
pub trait Ledger {
    /// Checks [`admit`] and records the reservation in one atomic step.
    async fn reserve(
        &self,
        reservation: &Reservation,
        limits: &Limits,
    ) -> Result<Result<(), Denied>, LedgerError>;

    /// Idempotent: a settled attempt is not changed again; an unknown one can still be
    /// reconciled to a known charge.
    async fn settle(
        &self,
        attempt: AttemptId,
        settlement: &Settlement,
        at: Timestamp,
    ) -> Result<(), LedgerError>;

    /// Keeps an accepted answer for successor runs; the default keeps none. A failure is
    /// recorded with the task and never fails it: the answer itself stands.
    async fn checkpoint(&self, _checkpoint: &super::Checkpoint) -> Result<(), LedgerError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_rounds_up_and_never_overflows() {
        let card = PriceCard {
            input_per_mtok: 1,
            output_per_mtok: 3,
        };
        assert_eq!(card.cost(1, 0), MicroUsd(1));
        assert_eq!(card.cost(1_000_000, 1_000_000), MicroUsd(4));
        let huge = PriceCard {
            input_per_mtok: u64::MAX,
            output_per_mtok: u64::MAX,
        };
        assert_eq!(huge.cost(u64::MAX, u64::MAX), MicroUsd(u64::MAX));
    }
}
