//! One structured model task: reserve → dispatch → settle → validate, with at most one
//! repair (spec §16.4, §17.2). Fallback hops belong to the [`Router`](super::Router).

use schemars::generate::SchemaSettings;
use schemars::transform::RecursiveTransform;
use schemars::{JsonSchema, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{
    AttemptKind, Denied, ErrorKind, Finish, Ledger, Limits, Message, ModelProvider, ModelReply,
    ModelRequest, ObservedUsage, PriceCard, Prompt, ProviderError, Purpose, Reservation, Route,
    Settlement, input_token_bound, parse_output, repair_messages,
};
use crate::config::{BillingKind, Enforcement, FixedFee};
use crate::{AttemptId, MicroUsd, Redactor, RunId, TaskId, Timestamp};

/// Why a task produced no validated output; each maps to a gap, never to an empty success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "failure", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskFailure {
    Budget {
        denied: Denied,
    },
    Provider {
        kind: ErrorKind,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_seconds: Option<u32>,
    },
    Schema {
        error: String,
    },
    Truncated,
    Refused,
    Ledger {
        error: String,
    },
    /// Every profile of the task's chain was skipped; each reason names one.
    NoEligibleRoute {
        reasons: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChargeState {
    Released,
    Settled,
    Unknown,
}

/// Run-artifact view of one attempt; no prompt text and no chain-of-thought.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub attempt_id: AttemptId,
    pub kind: AttemptKind,
    pub template: String,
    pub schema_name: String,
    pub profile: String,
    pub requested_model: String,
    pub resolved_model: Option<String>,
    pub upstream_provider: Option<String>,
    pub generation_id: Option<String>,
    pub billing: BillingKind,
    pub enforcement: Enforcement,
    pub reserved: MicroUsd,
    pub charge_state: ChargeState,
    pub charge: Option<MicroUsd>,
    /// List-price estimate from complete usage or the provider's own; not a charge.
    pub reference: Option<MicroUsd>,
    pub usage: ObservedUsage,
    /// Access type the provider reports and the subscription quota consumed, when it does.
    pub billing_source: Option<String>,
    pub allowance: Option<MicroUsd>,
    pub failure: Option<TaskFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome<T> {
    pub value: Result<T, TaskFailure>,
    pub attempts: Vec<AttemptRecord>,
}

/// Everything one run needs to spend on a single route.
pub struct Gateway<'a, P, L> {
    pub provider: &'a P,
    pub ledger: &'a L,
    pub route: Route,
    pub price: Option<PriceCard>,
    pub fixed_fee: Option<FixedFee>,
    pub tenant_id: String,
    pub run_id: RunId,
    pub purpose: Purpose,
    pub limits: Limits,
    pub deadline: Option<Timestamp>,
    pub now: fn() -> Timestamp,
    /// Applied to every data block before it leaves for the provider.
    pub redactor: &'a Redactor,
}

impl<P: ModelProvider, L: Ledger> Gateway<'_, P, L> {
    pub async fn structured<T: DeserializeOwned + JsonSchema>(
        &self,
        prompt: &Prompt,
        schema_name: &str,
    ) -> TaskOutcome<T> {
        let schema = serde_json::to_value(output_schema::<T>()).unwrap_or_default();
        let base = prompt.messages(self.redactor);
        let task = TaskId::new_v7();
        let mut attempts = Vec::new();
        let first = self
            .attempt(
                task,
                AttemptKind::Initial,
                prompt,
                &base,
                schema_name,
                &schema,
                &mut attempts,
            )
            .await;
        let value = match first.map(|reply| (parse_output::<T>(&reply.text), reply)) {
            Err(failure) => Err(failure),
            Ok((Ok(value), _)) => Ok(value),
            Ok((Err(error), reply)) => {
                fail_last(
                    &mut attempts,
                    TaskFailure::Schema {
                        error: error.clone(),
                    },
                );
                let messages = repair_messages(&base, &reply.text, &error);
                let kind = AttemptKind::Repair;
                match self
                    .attempt(
                        task,
                        kind,
                        prompt,
                        &messages,
                        schema_name,
                        &schema,
                        &mut attempts,
                    )
                    .await
                {
                    Err(failure) => Err(failure),
                    Ok(reply) => parse_output::<T>(&reply.text).map_err(|error| {
                        let failure = TaskFailure::Schema { error };
                        fail_last(&mut attempts, failure.clone());
                        failure
                    }),
                }
            }
        };
        TaskOutcome { value, attempts }
    }

    #[allow(clippy::too_many_arguments)] // one attempt's whole context
    async fn attempt(
        &self,
        task_id: TaskId,
        kind: AttemptKind,
        prompt: &Prompt,
        messages: &[Message],
        schema_name: &str,
        schema: &serde_json::Value,
        attempts: &mut Vec<AttemptRecord>,
    ) -> Result<ModelReply, TaskFailure> {
        let route = &self.route;
        let reservation = self.reservation(kind, prompt, messages)?;
        let denied = |denied| TaskFailure::Budget { denied };
        if self.deadline.is_some_and(|d| reservation.at >= d) {
            return Err(denied(Denied::Deadline));
        }

        let ledger_error = |e: super::LedgerError| TaskFailure::Ledger { error: e.0 };
        self.ledger
            .reserve(&reservation, &self.limits)
            .await
            .map_err(ledger_error)?
            .map_err(denied)?;
        let request = ModelRequest {
            attempt_id: reservation.attempt_id,
            task_id,
            model: route.model.clone(),
            messages: messages.to_vec(),
            schema_name: schema_name.to_owned(),
            schema: schema.clone(),
            max_output_tokens: route
                .capabilities
                .output_cap_enforced
                .then_some(self.limits.request_output_tokens()),
            deadline: self.deadline,
        };
        let reply = self.dispatch(&request).await;
        let settlement = match &reply {
            Ok(reply) => self.settlement(reply),
            Err(e) if !e.dispatched => Settlement::Released,
            Err(_) => Settlement::Unknown {
                usage: ObservedUsage::default(),
            },
        };
        let mut record = AttemptRecord {
            attempt_id: reservation.attempt_id,
            kind,
            template: prompt.template.clone(),
            schema_name: schema_name.to_owned(),
            profile: route.profile.clone(),
            requested_model: route.model.clone(),
            resolved_model: reply.as_ref().ok().and_then(|r| r.resolved_model.clone()),
            upstream_provider: reply
                .as_ref()
                .ok()
                .and_then(|r| r.upstream_provider.clone()),
            generation_id: reply.as_ref().ok().and_then(|r| r.generation_id.clone()),
            billing: route.billing,
            enforcement: route.enforcement,
            reserved: reservation.money,
            charge_state: ChargeState::Unknown,
            charge: None,
            reference: None,
            usage: ObservedUsage::default(),
            billing_source: reply.as_ref().ok().and_then(|r| r.billing_source.clone()),
            allowance: reply.as_ref().ok().and_then(|r| r.allowance),
            failure: None,
        };
        match &settlement {
            Settlement::Released => record.charge_state = ChargeState::Released,
            Settlement::Settled {
                usage,
                charge,
                reference,
                ..
            } => {
                record.charge_state = ChargeState::Settled;
                record.charge = *charge;
                record.reference = *reference;
                record.usage = *usage;
            }
            Settlement::Unknown { usage } => record.usage = *usage,
        }
        let settled = self
            .ledger
            .settle(reservation.attempt_id, &settlement, (self.now)())
            .await;
        let outcome = settled.map_err(ledger_error).and(match reply {
            Err(e) => Err(TaskFailure::Provider {
                kind: e.kind,
                message: e.message,
                retry_after_seconds: e.retry_after_seconds,
            }),
            Ok(reply) => match reply.finish {
                Finish::Complete => Ok(reply),
                Finish::Truncated => Err(TaskFailure::Truncated),
                Finish::Refused => Err(TaskFailure::Refused),
            },
        });
        record.failure = outcome.as_ref().err().cloned();
        attempts.push(record);
        outcome
    }

    /// Reserving can take the rest of the run; then nothing is sent and the money returns.
    async fn dispatch(&self, request: &ModelRequest) -> Result<ModelReply, ProviderError> {
        if self.deadline.is_some_and(|d| (self.now)() >= d) {
            return Err(ProviderError {
                kind: ErrorKind::Timeout,
                dispatched: false,
                message: "run deadline reached before dispatch".into(),
                retry_after_seconds: None,
            });
        }

        self.provider.complete(request).await
    }

    fn reservation(
        &self,
        kind: AttemptKind,
        prompt: &Prompt,
        messages: &[Message],
    ) -> Result<Reservation, TaskFailure> {
        let route = &self.route;
        let input_tokens = input_token_bound(messages);
        let output = u64::from(self.limits.request_output_tokens());
        let denied = |denied| TaskFailure::Budget { denied };
        let (money, fixed_fee) = match route.billing {
            BillingKind::ApiUsage => {
                let price = self
                    .price
                    .filter(|_| route.capabilities.output_cap_enforced)
                    .ok_or(denied(Denied::NoPriceBound))?;
                (price.cost(input_tokens, output), None)
            }
            // Overage stays off, so a request adds no money beyond the period fee.
            BillingKind::SubscriptionQuota => (
                MicroUsd(0),
                Some(self.fixed_fee.ok_or(denied(Denied::FixedFeeUnknown))?),
            ),
        };
        Ok(Reservation {
            attempt_id: AttemptId::new_v7(),
            tenant_id: self.tenant_id.clone(),
            run_id: self.run_id,
            purpose: self.purpose,
            kind,
            template: prompt.template.clone(),
            route: route.clone(),
            money,
            input_tokens,
            output_tokens: (route.enforcement == Enforcement::ApiHard).then_some(output),
            fixed_fee,
            at: (self.now)(),
        })
    }

    fn settlement(&self, reply: &ModelReply) -> Settlement {
        let u = reply.usage;
        let computed = self
            .price
            .zip(u.input_tokens.zip(u.output_tokens))
            .map(|(p, (i, o))| p.cost(i, o));
        let charge = match self.route.billing {
            BillingKind::ApiUsage => match reply.reported_cost.or(computed) {
                Some(charge) => Some(charge),
                None => return Settlement::Unknown { usage: u },
            },
            BillingKind::SubscriptionQuota => reply.reported_cost,
        };
        Settlement::Settled {
            usage: u,
            charge,
            // A provider list price is a reference too, never the charge.
            reference: computed.or(reply.list_cost),
            resolved_model: reply.resolved_model.clone(),
            generation_id: reply.generation_id.clone(),
        }
    }
}

/// The output schema with every subschema inlined: structured-output upstreams do not all
/// resolve `$ref`, and an unresolved one silently empties arrays of nested objects. `oneOf`
/// becomes `anyOf`: `Anthropic` and strict `OpenAI` reject `oneOf`, and serde still decodes one
/// variant.
pub(super) fn output_schema<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .with(|s| s.inline_subschemas = true)
        .with_transform(RecursiveTransform(|schema: &mut Schema| {
            if let Some(variants) = schema.remove("oneOf") {
                schema.insert("anyOf".into(), variants);
            }
        }))
        .into_generator()
        .into_root_schema_for::<T>()
}

fn fail_last(attempts: &mut [AttemptRecord], failure: TaskFailure) {
    if let Some(last) = attempts.last_mut() {
        last.failure = Some(failure);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{AssessmentOutput, DiscoveryOutput, ReviewOutput, VerifyOutput};

    #[test]
    fn output_schemas_have_no_references() {
        for schema in [
            output_schema::<DiscoveryOutput>(),
            output_schema::<ReviewOutput>(),
            output_schema::<AssessmentOutput>(),
            output_schema::<VerifyOutput>(),
        ] {
            let text = serde_json::to_string(&schema).expect("json");
            assert!(!text.contains("$ref") && !text.contains("$defs"), "{text}");
            assert!(!text.contains("\"oneOf\""), "{text}");
        }
    }

    /// Strict structured outputs need every property required; an optional one is nullable.
    #[test]
    fn a_suggested_change_is_required_and_nullable() {
        let schema = serde_json::to_value(output_schema::<VerifyOutput>()).expect("json");
        let verdict = &schema["properties"]["verdicts"]["items"];
        let required = verdict["required"].as_array().expect("required");
        assert!(
            required.iter().any(|r| r == "suggested_change"),
            "{verdict}"
        );
        let change = verdict["properties"]["suggested_change"].to_string();
        assert!(change.contains("\"null\""), "{change}");
    }

    #[test]
    fn a_candidate_schema_demands_an_anchor() {
        let schema = serde_json::to_value(output_schema::<DiscoveryOutput>()).expect("json");
        let anchors = &schema["properties"]["candidates"]["items"]["properties"]["anchors"];
        assert_eq!(anchors["minItems"], 1, "{anchors}");
    }
}
