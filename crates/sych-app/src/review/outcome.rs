//! Packets, model passes and the reduced result of one run.

use std::cell::RefCell;
use std::time::Duration;

use anyhow::Context as _;

use sych_core::analysis::packet_evidence;
use sych_core::broker::{
    BrokerInput, EvidencePacket, PacketResolution, build_packets, context_gaps, context_limited,
};
use sych_core::config::BillingKind;
use sych_core::model::{ChargeState, Hop, Limits as ModelLimits, Purpose, Router};
use sych_core::{
    DEFAULT_ATTENTION_THRESHOLD, Evidence, EvidenceManifest, ExecutionOutcome,
    ExternalInputManifest, MicroUsd, Provenance, ReduceInput, ResultId, ReviewResult, RouteUsage,
    RunInfo, Timestamp, UsageSummary, finalize, reduce,
};
use sych_storage::{RunHandle, RunLedger, Storage, TENANT_LOCAL};

use crate::analysis::{AnalysisRecord, Engine, Outcome, PassStatus};
use crate::backend::Routing;

use super::Session;
use super::checkpoints;

impl Session<'_> {
    pub(super) fn packets(&self, run: &RunHandle) -> Vec<EvidencePacket> {
        let head_compiled = self
            .facts
            .ci
            .iter()
            .any(|r| r.compiled(&self.snap.snapshot));
        let build = |limits| {
            build_packets(
                &BrokerInput {
                    run_id: run.id,
                    snapshot_id: &self.snap.snapshot.id,
                    policy_snapshot_id: &self.policy.id,
                    access_scope: &self.snap.snapshot.repository,
                    plan: self.plan,
                    manifest: &self.snap.manifest,
                    facts: &self.facts.semantic,
                    syntax: &self.facts.syntax,
                    ci: &self.facts.ci,
                    head_compiled,
                    ledger: &self.facts.ledger,
                    lint: &self.facts.lint,
                    limits,
                },
                self.snap,
            )
        };
        let mut packets = build(self.limits);
        let cap = self.budgets.analysis.max_input_tokens;
        if let Some(fitted) = cap.and_then(|cap| self.limits.fit_run(&packets, cap)) {
            tracing::info!(
                request_tokens = fitted.request_tokens,
                unit_tokens = fitted.unit_tokens,
                "context compressed to fit the run input budget"
            );
            packets = build(fitted);
        }

        tracing::info!(
            packets = packets.len(),
            partial = packets
                .iter()
                .filter(|p| p.resolution != PacketResolution::Satisfied)
                .count(),
            "context packets built"
        );
        packets
    }

    /// Model passes over the packets; the source each packet shows becomes evidence.
    pub(super) async fn analyze(
        &self,
        storage: &Storage,
        run: &RunHandle,
        model: &Routing,
        packets: &[EvidencePacket],
    ) -> anyhow::Result<ModelAnalysis> {
        let packet_evidence = packets
            .iter()
            .map(|p| packet_evidence(p, &self.snap.snapshot, self.now))
            .collect::<Result<Vec<_>, _>>()
            .context("cannot hash packets")?;
        let limits = ModelLimits::new(self.budgets, Purpose::Analysis);
        let deadline = self
            .budgets
            .analysis
            .active_deadline_seconds
            .and_then(|s| self.now.checked_add(Duration::from_secs(s.into())).ok());
        let evidence: Vec<Evidence> = self.evidence().chain(&packet_evidence).cloned().collect();
        let reuse = checkpoints::parent(storage, run, self.parent).await;
        tracing::info!(reusable = reuse.len(), "parent checkpoints read");
        let ledger = RunLedger::new(storage, run, Timestamp::now);
        let engine = Engine {
            router: Router {
                hops: model
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
                plan: model.plan.clone(),
                on: model.on.clone(),
                cooldown_seconds: model.cooldown_seconds,
                ledger: &ledger,
                tenant_id: TENANT_LOCAL.into(),
                run_id: run.id,
                purpose: Purpose::Analysis,
                limits,
                deadline,
                now: Timestamp::now,
                redactor: &model.redactor,
                cooldowns: RefCell::default(),
                reuse,
                models_revision: model.models_revision.clone(),
                eligible: Some(&*model.recheck),
            },
            snap: self.snap,
            plan: self.plan,
            packets,
            packet_evidence: &packet_evidence,
            facts: self.facts,
            evidence: &evidence,
            known: self
                .external
                .iter()
                .flat_map(|e| e.code_refs.iter().cloned())
                .collect(),
            batch_tokens: self.limits.request_tokens,
            risk_audit: self.risk_audit,
            verify: self.verify,
            parallel: self
                .budgets
                .analysis
                .max_parallel_calls
                .map_or(1, |n| usize::try_from(n).unwrap_or(1)),
            language: self.language,
        };
        let mut outcome = engine.run().await;
        for route in &mut outcome.record.routes {
            if let Some(Ok(r)) = model.profiles.get(&route.profile) {
                route.reasoning_effort = r.reasoning_effort;
                route.upstreams.clone_from(&r.upstreams);
            }
        }
        tracing::info!(
            attempts = outcome.record.attempts.len(),
            assessments = outcome.assessments.len(),
            candidates = outcome.candidates.len(),
            findings = outcome.findings.len(),
            limit_reached = outcome.limit_reached,
            reused = outcome
                .record
                .passes
                .iter()
                .filter(|p| p.reused_from.is_some())
                .count(),
            "model analysis finished"
        );
        for pass in &outcome.record.passes {
            if let Some(error) = &pass.checkpoint_error {
                tracing::warn!(run = %run.id, template = %pass.template, %error, "checkpoint not kept");
            }
        }
        Ok(ModelAnalysis {
            outcome,
            packet_evidence,
        })
    }

    pub(super) fn result(
        &self,
        run: &RunHandle,
        parent_result_id: Option<ResultId>,
        packets: &[EvidencePacket],
        analysis: Option<&ModelAnalysis>,
    ) -> anyhow::Result<(
        ExternalInputManifest,
        EvidenceManifest,
        ReviewResult,
        Vec<Evidence>,
    )> {
        let (plan, policy, now) = (self.plan, self.policy, self.now);
        let outcome = match analysis.map(|a| &a.outcome) {
            _ if plan.limit_reached => ExecutionOutcome::LimitReached,
            Some(a) if a.limit_reached => ExecutionOutcome::LimitReached,
            Some(a)
                if a.record
                    .passes
                    .iter()
                    .any(|p| matches!(p.status, PassStatus::Failed { .. })) =>
            {
                ExecutionOutcome::RetriesExhausted
            }
            _ => ExecutionOutcome::PlanExecuted,
        };
        let external = ExternalInputManifest::freeze(
            &plan.source_snapshot_id,
            &policy.id,
            &self.external,
            now,
        )?;
        let mut all: Vec<Evidence> = self
            .evidence()
            .chain(analysis.iter().flat_map(|a| &a.packet_evidence))
            .cloned()
            .collect();
        // The manifest's order, which `show` reads the evidence back in.
        all.sort_by_key(|e| e.id);
        let evidence = EvidenceManifest::freeze(&external, &all, now)?;
        let (assessments, findings, candidates, usage, adapters) = match analysis {
            Some(a) => (
                a.outcome.assessments.clone(),
                a.outcome.findings.as_slice(),
                a.outcome.candidates.as_slice(),
                usage(&a.outcome.record),
                a.outcome
                    .record
                    .routes
                    .iter()
                    .map(|r| format!("{}:{}", r.profile, r.model))
                    .collect(),
            ),
            None => (
                self.script.assessments.clone(),
                &[][..],
                &[][..],
                UsageSummary::default(),
                vec!["fake".into()],
            ),
        };
        let assessments = context_limited(plan, packets, assessments);
        let mut task_gaps = analysis.map_or_else(Vec::new, |a| a.outcome.gaps.clone());
        task_gaps.extend(context_gaps(packets));
        task_gaps.extend(self.ci_gaps.iter().cloned());
        let reduced = reduce(&ReduceInput {
            plan,
            assessments: &assessments,
            evidence: &all,
            findings,
            candidates,
            task_gaps: &task_gaps,
            questions: analysis.map_or(&[][..], |a| &a.outcome.questions),
            outcome,
            attention_threshold: DEFAULT_ATTENTION_THRESHOLD,
            now,
        });
        let info = RunInfo {
            run_id: run.id,
            result_id: ResultId::new_v7(),
            review_revision: run.generation,
            parent_result_id,
            usage,
            provenance: Provenance {
                engine_version: format!("{}+{}", env!("CARGO_PKG_VERSION"), crate::BUILD_COMMIT),
                model_adapters: adapters,
            },
            created_at: now,
            language: self.language,
        };
        let result = finalize(info, plan, policy, &evidence, outcome, reduced);
        Ok((external, evidence, result, all))
    }
}

pub(super) struct ModelAnalysis {
    pub(super) outcome: Outcome,
    pub(super) packet_evidence: Vec<Evidence>,
}

/// Known tokens and money; a charge that is not settled counts as unknown, never zero.
fn usage(record: &AnalysisRecord) -> UsageSummary {
    let attempts = &record.attempts;
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut usage = UsageSummary {
        model_attempts: count(attempts.len()),
        ..UsageSummary::default()
    };
    let mut quota = Vec::new();
    for a in attempts {
        if a.charge_state != ChargeState::Released && !a.usage.complete() {
            usage.unknown_usage_attempts += 1;
        }

        usage.input_tokens += a.usage.input_tokens.unwrap_or(0);
        usage.output_tokens += a.usage.output_tokens.unwrap_or(0);
        usage.reasoning_tokens += a.usage.reasoning_tokens.unwrap_or(0);
        let reference = a.reference.map_or(0, |r| r.0);
        usage.cost.reference_estimate =
            MicroUsd(usage.cost.reference_estimate.0.saturating_add(reference));
        match (a.charge_state, a.charge, a.billing) {
            (ChargeState::Settled, Some(charge), _) => {
                usage.cost.known = MicroUsd(usage.cost.known.0.saturating_add(charge.0));
            }
            (ChargeState::Settled, None, BillingKind::SubscriptionQuota) => {
                quota.push(a.profile.as_str());
            }
            (ChargeState::Released, ..) => {}
            _ => usage.cost.unknown_cost_attempts += 1,
        }
    }
    // Routes the run dispatched to; the reviewer's primary is listed even when unused.
    usage.routes = record
        .routes
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let used = attempts.iter().filter(|a| a.profile == r.profile).count();
            (used > 0 || i == 0).then(|| RouteUsage {
                profile: r.profile.clone(),
                model: r.model.clone(),
                billing: r.billing,
                enforcement: r.enforcement,
                attempts: count(used),
                quota_attempts: count(quota.iter().filter(|p| **p == r.profile).count()),
                fixed_fee: r.fixed_fee.clone(),
            })
        })
        .collect();
    usage
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use sych_core::config::Enforcement;

    use sych_core::model::{AttemptKind, AttemptRecord, ObservedUsage, RoutePlan};
    use sych_core::{AttemptId, FeeShare, SchemaVersion};

    use super::*;
    use crate::analysis::PlannedRoute;

    fn attempt(state: ChargeState, charge: Option<u64>, tokens: Option<u64>) -> AttemptRecord {
        AttemptRecord {
            attempt_id: AttemptId::new_v7(),
            kind: AttemptKind::Initial,
            template: "discovery@1".into(),
            schema_name: "discovery".into(),
            profile: "sub".into(),
            requested_model: "m".into(),
            resolved_model: None,
            upstream_provider: None,
            generation_id: None,
            billing: BillingKind::SubscriptionQuota,
            enforcement: Enforcement::SubscriptionObservable,
            reserved: MicroUsd(0),
            charge_state: state,
            charge: charge.map(MicroUsd),
            reference: tokens.map(|_| MicroUsd(7)),
            usage: ObservedUsage {
                input_tokens: tokens,
                output_tokens: tokens,
                reasoning_tokens: tokens.map(|t| t / 2),
            },
            billing_source: None,
            allowance: None,
            failure: None,
        }
    }

    #[test]
    fn quota_fee_and_unknowns_stay_apart() {
        let fee = FeeShare {
            period: "2026-10".into(),
            amount: MicroUsd(20_000_000),
            allocated: MicroUsd(10_000_000),
        };
        let record = AnalysisRecord {
            schema_version: SchemaVersion,
            plan: RoutePlan {
                chains: BTreeMap::new(),
                revision: "test".into(),
            },
            routes: vec![
                PlannedRoute {
                    profile: "sub".into(),
                    model: "m".into(),
                    reasoning_effort: None,
                    upstreams: Vec::new(),
                    billing: BillingKind::SubscriptionQuota,
                    enforcement: Enforcement::SubscriptionObservable,
                    fixed_fee: Some(fee.clone()),
                },
                PlannedRoute {
                    profile: "unused".into(),
                    model: "m2".into(),
                    reasoning_effort: None,
                    upstreams: Vec::new(),
                    billing: BillingKind::ApiUsage,
                    enforcement: Enforcement::ApiHard,
                    fixed_fee: None,
                },
            ],
            passes: Vec::new(),
            assessments: Vec::new(),
            discovered: Vec::new(),
            answers: Vec::new(),
            candidates: Vec::new(),
            attempts: vec![
                attempt(ChargeState::Settled, None, Some(100)),
                attempt(ChargeState::Settled, Some(5), Some(10)),
                attempt(ChargeState::Unknown, None, None),
                attempt(ChargeState::Released, None, None),
            ],
            hops: Vec::new(),
        };
        let u = usage(&record);
        assert_eq!(
            (u.input_tokens, u.output_tokens, u.reasoning_tokens),
            (110, 110, 55)
        );
        assert_eq!(u.unknown_usage_attempts, 1, "released attempts never ran");
        assert_eq!(u.cost.known, MicroUsd(5), "the fee is not a per-run charge");
        assert_eq!(u.cost.reference_estimate, MicroUsd(14));
        assert_eq!(u.cost.unknown_cost_attempts, 1);
        assert_eq!(u.routes[0].quota_attempts, 1);
        assert_eq!(u.routes[0].fixed_fee, Some(fee));
        assert_eq!(
            u.routes.len(),
            1,
            "a planned route without attempts is not listed"
        );
    }
}
