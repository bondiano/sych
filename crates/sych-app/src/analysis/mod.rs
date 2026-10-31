//! Model passes of one review: required assessments first, so discovery can
//! never spend the verification reserve, though they and the audit leave discovery a share
//! of the attempts; then the independent high-risk audit, discovery
//! with typed context asks, and verification of every candidate. A discovery batch that
//! shows an obligation whole assesses it in the same call (`review@1`), which comes first.
//! Each pass goes through the gateway and ledger; a failed or denied pass leaves an
//! explicit gap.

use std::cmp::Reverse;

use serde::Serialize;
use sych_adapters::git::LocalSnapshot;
use sych_core::analysis::{
    self, AssessmentRecord, Context, Discovered, merge_overlapping, prompts,
};
use sych_core::broker::{EvidencePacket, OpStatus, REQUIRED_OVER_BUDGET};
use sych_core::config::{BillingKind, Enforcement, ReasoningEffort, Role};
use sych_core::model::{
    AttemptRecord, Denied, HopRecord, Ledger, ModelProvider, RoutePlan, Router, TaskFailure,
};
use sych_core::{
    Assessment, Candidate, CodeRef, Evidence, FeeShare, Finding, NOT_ASSESSED, ObligationId,
    ObligationStatus, PlannedObligation, ReviewPlan, ReviewUnitId, RiskLevel, RunFacts,
    SchemaVersion,
};

mod parts;
mod passes;

use parts::{Combined, Reviewed, Task};

/// Answers kept per context ask; each is a span the verifier sees.
const MAX_REFS_PER_ANSWER: usize = 3;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PassStatus {
    Done,
    Failed { failure: TaskFailure },
    Skipped { reason: String },
}

/// One planned model call and what happened to it; no prompt text.
#[derive(Debug, Clone, Serialize)]
pub struct Pass {
    pub template: String,
    /// Packets shown in the call; a discovery batch shows several.
    pub packets: Vec<usize>,
    pub obligation_id: Option<ObligationId>,
    pub status: PassStatus,
    pub attempt_ids: Vec<sych_core::AttemptId>,
    /// The parent run whose checkpoint answered; no call was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reused_from: Option<sych_core::RunId>,
    /// Why the accepted answer was not checkpointed; a successor asks again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AskAnswer {
    pub packet: usize,
    pub question: String,
    pub operation: analysis::AskOperation,
    pub status: Option<OpStatus>,
    pub refs: usize,
    pub limitations: Vec<String>,
}

/// One profile the run's plan may dispatch to.
#[derive(Debug, Clone, Serialize)]
pub struct PlannedRoute {
    pub profile: String,
    pub model: String,
    /// Profile settings sent with each request; the app fills them from the resolved profile.
    pub reasoning_effort: Option<ReasoningEffort>,
    pub upstreams: Vec<String>,
    pub billing: BillingKind,
    pub enforcement: Enforcement,
    /// The subscription fee of the month the run fell in; API routes have none.
    pub fixed_fee: Option<FeeShare>,
}

/// The `analysis` artifact: routes, passes, accepted and refused model output, attempts.
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisRecord {
    pub schema_version: SchemaVersion,
    /// Chains per role and their revision, fixed for the run.
    pub plan: RoutePlan,
    /// The resolved profiles of the plan, primary of the reviewer first.
    pub routes: Vec<PlannedRoute>,
    pub passes: Vec<Pass>,
    pub assessments: Vec<AssessmentRecord>,
    pub discovered: Vec<Discovered>,
    pub answers: Vec<AskAnswer>,
    pub candidates: Vec<Candidate>,
    pub attempts: Vec<AttemptRecord>,
    /// Every profile each task visited, with why it moved on or stopped.
    pub hops: Vec<HopRecord>,
}

pub struct Outcome {
    pub assessments: Vec<Assessment>,
    pub candidates: Vec<Candidate>,
    pub findings: Vec<Finding>,
    pub record: AnalysisRecord,
    /// A budget, deadline, attempt or output limit stopped some pass.
    pub limit_reached: bool,
    /// Failed passes and dropped model output, for the result's unresolved questions.
    pub gaps: Vec<String>,
    /// Open model questions: reported, but they do not lower coverage.
    pub questions: Vec<String>,
}

pub struct Engine<'a, P, L> {
    pub router: Router<'a, P, L>,
    pub snap: &'a LocalSnapshot,
    pub plan: &'a ReviewPlan,
    pub packets: &'a [EvidencePacket],
    /// Source evidence of each packet, aligned with `packets`.
    pub packet_evidence: &'a [Evidence],
    pub facts: &'a RunFacts,
    pub evidence: &'a [Evidence],
    /// Locations existing diagnostics already report.
    pub known: Vec<CodeRef>,
    /// Shown tokens one discovery request may batch (`context.max_tokens_per_request`).
    pub batch_tokens: u32,
    /// The independent high-risk search; off only for an evaluation baseline.
    pub risk_audit: bool,
    /// Verification of candidates; off only for evaluation baseline A (one reviewer).
    pub verify: bool,
    /// Model calls in flight at once (`budgets.analysis.max_parallel_calls`), at least 1.
    pub parallel: usize,
    /// `review.language`: every prompt asks for prose in it.
    pub language: sych_core::ReviewLanguage,
}

/// The search's discovery was denied by the run's input-token cap, so it made no call.
fn discovery_out_of_tokens(part: &Outcome) -> bool {
    part.record.passes.iter().any(|p| {
        p.template == prompts::DISCOVERY
            && matches!(
                &p.status,
                PassStatus::Failed {
                    failure: TaskFailure::Budget {
                        denied: Denied::InputTokens { .. }
                    }
                }
            )
    })
}

fn needs_assessment(planned: &PlannedObligation) -> bool {
    matches!(&planned.obligation.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == NOT_ASSESSED)
}

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// Resolved profiles of the plan in chain order, the reviewer's primary first.
    fn routes(&self) -> Vec<PlannedRoute> {
        let r = &self.router;
        let period = (r.now)().strftime("%Y-%m").to_string();
        let mut names: Vec<&String> = Vec::new();
        for name in r.plan.chains.values().flatten() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if let Some(primary) = r.plan.chains.get(&Role::Reviewer).and_then(|c| c.first()) {
            names.retain(|n| *n != primary);
            names.insert(0, primary);
        }

        names
            .into_iter()
            .filter_map(|name| r.hops.get(name)?.as_ref().ok())
            .map(|hop| PlannedRoute {
                profile: hop.route.profile.clone(),
                model: hop.route.model.clone(),
                reasoning_effort: None,
                upstreams: Vec::new(),
                billing: hop.route.billing,
                enforcement: hop.route.enforcement,
                fixed_fee: hop
                    .fixed_fee
                    .filter(|_| hop.route.billing == BillingKind::SubscriptionQuota)
                    .map(|fee| FeeShare {
                        period: period.clone(),
                        amount: fee.amount_micro_usd,
                        allocated: fee.allocated_micro_usd,
                    }),
            })
            .collect()
    }

    pub async fn run(&self) -> Outcome {
        let mut out = self.part();
        out.record.routes = self.routes();
        let mut planned: Vec<&PlannedObligation> = self
            .plan
            .obligations
            .iter()
            .filter(|o| needs_assessment(o))
            .collect();
        planned.sort_by_key(|o| (!o.obligation.required, Reverse(o.obligation.risk_level)));

        // Obligations a discovery batch shows whole are assessed in that batch's one call.
        let batches = self.discovery_batches();
        let reviews = self.reviews(&batches, &planned);
        // The passes before discovery leave it a share of the attempts; with a share they also
        // leave the verifier its attempt, which discovery itself cannot spend.
        let share = self
            .router
            .limits
            .discovery_share(batches.len() - reviews.len());
        let (assessing_keeps, audit_keeps) = (share + u32::from(share > 0), share + 1);
        // Combined and standalone calls share one queue in the obligations' order, so a tight
        // budget is spent on the required and riskier ones first.
        let rank = |id: ObligationId| planned.iter().position(|p| p.obligation.id == id);
        let mut assessing: Vec<(usize, Task<'_, Option<Reviewed<'_>>>)> = Vec::new();
        let mut assessed_in_review = Vec::new();
        for (batch, context, linked, assessed) in reviews {
            assessed_in_review.extend(assessed.iter().map(|&i| linked[i].obligation.id));
            let first = assessed
                .iter()
                .filter_map(|&i| rank(linked[i].obligation.id))
                .min()
                .unwrap_or(usize::MAX);
            assessing.push((
                first,
                Box::pin(async move {
                    let mut part = self.part();
                    let (context, found) = self
                        .review(
                            batch,
                            &linked,
                            &assessed,
                            context,
                            assessing_keeps,
                            &mut part,
                        )
                        .await;
                    (part, Some((batch, linked, context, found)))
                }),
            ));
        }
        for (i, obligation) in planned
            .iter()
            .enumerate()
            .filter(|(_, o)| !assessed_in_review.contains(&o.obligation.id))
        {
            assessing.push((
                i,
                Box::pin(async move {
                    let mut part = self.part();
                    self.assess(obligation, assessing_keeps, &mut part).await;
                    (part, None)
                }),
            ));
        }
        assessing.sort_by_key(|(rank, _)| *rank);
        let assessing = assessing.into_iter().map(|(_, task)| task).collect();
        let mut reviewed = Vec::new();
        for (part, review) in self.each(assessing).await {
            out.absorb(part);
            reviewed.extend(review);
        }

        let done: Vec<&Vec<usize>> = reviewed.iter().map(|(batch, ..)| *batch).collect();
        let mut searching = self.audits(&planned, audit_keeps);
        for (batch, linked, context, found) in reviewed {
            searching.push(Box::pin(async move {
                let mut part = self.part();
                self.verify(batch, &linked, context, found, &mut part).await;
                (part, None)
            }));
        }
        for batch in batches.iter().filter(|b| !done.contains(b)) {
            searching.push(Box::pin(
                async move { (self.search(batch).await, Some(batch)) },
            ));
        }
        let mut retry = Vec::new();
        for (part, batch) in self.each(searching).await {
            match batch {
                Some(batch) if discovery_out_of_tokens(&part) => {
                    out.record.attempts.extend(part.record.attempts);
                    out.record.hops.extend(part.record.hops);
                    retry.push(batch);
                }
                _ => out.absorb(part),
            }
        }
        // A sibling in flight held its whole byte bound; settled, it may have left room.
        for batch in retry {
            out.absorb(self.search(batch).await);
        }
        out.findings = merge_overlapping(out.findings);
        out.record.candidates.clone_from(&out.candidates);
        out
    }

    /// The batches that show some of `planned` whole and assess them in their one call.
    fn reviews<'s>(
        &'s self,
        batches: &'s [Vec<usize>],
        planned: &[&PlannedObligation],
    ) -> Vec<Combined<'s>> {
        batches
            .iter()
            .filter_map(|batch| {
                let (context, linked) = self.batch_view(batch);
                let assessed: Vec<usize> = (0..linked.len())
                    .filter(|&i| {
                        planned
                            .iter()
                            .any(|p| p.obligation.id == linked[i].obligation.id)
                            && self.shown_whole(linked[i].obligation.id, batch)
                    })
                    .collect();
                (!assessed.is_empty()).then_some((batch, context, linked, assessed))
            })
            .collect()
    }

    /// The independent searches of the high-risk obligations, one per verification chunk;
    /// each leaves `kept` attempts to discovery and the verifier.
    fn audits<'s>(
        &'s self,
        planned: &[&'s PlannedObligation],
        kept: u32,
    ) -> Vec<Task<'s, Option<&'s Vec<usize>>>> {
        planned
            .iter()
            .filter(|o| self.risk_audit && o.obligation.risk_level >= RiskLevel::High)
            .flat_map(|&o| {
                self.verification_packets(o.obligation.id)
                    .map(move |i| -> Task<'s, _> {
                        Box::pin(async move { (self.audit(o, i, kept).await, None) })
                    })
            })
            .collect()
    }

    /// The independent search for one high-risk obligation and verification of what it found.
    async fn audit(&self, obligation: &PlannedObligation, index: usize, kept: u32) -> Outcome {
        let mut part = self.part();
        let mut context = self.context(index);
        let prompt = prompts::risk_audit(&context, obligation);
        let ids = [obligation.obligation.id];
        let found = self
            .discover(
                &[index],
                Some(obligation.obligation.id),
                &prompt,
                &context,
                &ids,
                kept,
                &mut part,
            )
            .await;
        if let Some(found) = &found {
            self.answer(index, &found.asks, &mut context, &mut part);
        }

        self.verify(&[index], &[obligation], context, found, &mut part)
            .await;
        part
    }

    /// Discovery of one batch, its context asks answered, and verification.
    async fn search(&self, batch: &[usize]) -> Outcome {
        let mut part = self.part();
        let (mut context, linked) = self.batch_view(batch);
        let ids: Vec<ObligationId> = linked.iter().map(|o| o.obligation.id).collect();
        let prompt = prompts::discovery(&context, &linked, &self.title(batch));
        let found = self
            .discover(batch, None, &prompt, &context, &ids, 1, &mut part)
            .await;
        if let Some(found) = &found {
            self.answer(batch[0], &found.asks, &mut context, &mut part);
        }

        self.verify(batch, &linked, context, found, &mut part).await;
        part
    }

    /// Consecutive discovery packets whose shown tokens fit one request: one call for a small
    /// PR instead of one per review unit.
    fn discovery_batches(&self) -> Vec<Vec<usize>> {
        let mut batches: Vec<(Vec<usize>, u32)> = Vec::new();
        for (i, packet) in self.packets.iter().enumerate() {
            if packet.request.obligation_id.is_some() {
                continue;
            }

            let tokens = packet.usage.tokens;
            match batches.last_mut() {
                Some((batch, used)) if *used + tokens <= self.batch_tokens => {
                    batch.push(i);
                    *used += tokens;
                }
                _ => batches.push((vec![i], tokens)),
            }
        }
        batches.into_iter().map(|(batch, _)| batch).collect()
    }

    /// The packet of an obligation, or its chunks when its required spans did not fit one.
    fn verification_packets(&self, obligation: ObligationId) -> impl Iterator<Item = usize> {
        (0..self.packets.len())
            .filter(move |&i| self.packets[i].request.obligation_id == Some(obligation))
    }

    fn context(&self, index: usize) -> Context {
        let mut context = Context::new(self.language);
        context.add_packet(
            &self.packets[index],
            &self.packet_evidence[index],
            self.snap,
        );
        context.add_changes(&self.snap.manifest);
        let obligation = self.packets[index].request.obligation_id;
        if self
            .plan
            .obligations
            .iter()
            .any(|o| Some(o.obligation.id) == obligation && o.about_tests)
        {
            context.add_test_removals(&self.snap.manifest, self.snap);
        }

        context
    }

    /// The spans of a discovery batch and the obligations linked to its units.
    fn batch_view(&self, batch: &[usize]) -> (Context, Vec<&PlannedObligation>) {
        let mut context = Context::new(self.language);
        for &i in batch {
            context.add_packet(&self.packets[i], &self.packet_evidence[i], self.snap);
        }
        context.add_changes(&self.snap.manifest);

        let units: Vec<ReviewUnitId> = batch
            .iter()
            .flat_map(|&i| self.packets[i].request.review_unit_ids.iter().copied())
            .collect();
        let linked = self
            .plan
            .obligations
            .iter()
            .filter(|o| {
                o.obligation
                    .review_unit_ids
                    .iter()
                    .any(|u| units.contains(u))
            })
            .collect::<Vec<&PlannedObligation>>();
        if linked.iter().any(|o| o.about_tests) {
            context.add_test_removals(&self.snap.manifest, self.snap);
        }

        (context, linked)
    }

    fn title(&self, batch: &[usize]) -> String {
        let mut questions: Vec<&str> = batch
            .iter()
            .map(|&i| self.packets[i].request.question.as_str())
            .collect();
        // The chunks of one unit ask the same question.
        questions.dedup();
        questions.join("\n")
    }

    /// Whether every span of the obligation's verification packet lies inside the batch's
    /// spans, with no required span left over the budget.
    fn shown_whole(&self, obligation: ObligationId, batch: &[usize]) -> bool {
        let chunks: Vec<&EvidencePacket> = self
            .verification_packets(obligation)
            .map(|i| &self.packets[i])
            .collect();
        let shown: Vec<&CodeRef> = batch
            .iter()
            .flat_map(|&i| &self.packets[i].fragments)
            .map(|f| &f.code_ref)
            .collect();
        let inside = |r: &CodeRef| {
            shown.iter().any(|s| {
                s.side == r.side
                    && s.path == r.path
                    && s.blob_oid == r.blob_oid
                    && s.start_line <= r.start_line
                    && r.end_line <= s.end_line
            })
        };
        !chunks.is_empty()
            && chunks.iter().all(|packet| {
                packet
                    .excluded
                    .iter()
                    .all(|e| e.reason != REQUIRED_OVER_BUDGET)
                    && packet.fragments.iter().all(|f| inside(&f.code_ref))
            })
    }
}
