//! The individual model passes; each records its attempts and leaves a gap when it fails.

use sych_core::analysis::{
    AssessmentOutput, AssessmentRecord, CANDIDATE_OVERFLOW, Context, ContextAsk, Discovered,
    DiscoveryOutput, ReviewOutput, Verified, VerifyOutput, context_asks, drop_fixture_claims,
    effect_asks, not_assessed, prompts, refuting_asks, unseen_violation, validate_assessment,
    validate_discovery, validate_verdicts,
};
use sych_core::broker::{Ops, REQUIRED_OVER_BUDGET, unseen_spans};
use sych_core::config::Role;
use sych_core::model::{
    Ledger, MODEL_QUOTA_EXHAUSTED, ModelProvider, Prompt, RoutedOutcome, TaskFailure,
};
use sych_core::{Candidate, CodeRef, ObligationId, PlannedObligation, VerificationStatus};

use super::{AskAnswer, Engine, MAX_REFS_PER_ANSWER, Outcome, Pass, PassStatus};

const QUOTA_MESSAGE: &str = "Model usage limit reached. Increase the provider limit or top up your balance, then rerun the review.";

/// The finalization of an obligation whose assessment call failed.
fn failed(planned: &PlannedObligation, failure: &TaskFailure) -> AssessmentRecord {
    match failure {
        failure if failure.quota_exhausted() => {
            not_assessed(planned, MODEL_QUOTA_EXHAUSTED, QUOTA_MESSAGE)
        }
        TaskFailure::Budget { denied } => not_assessed(
            planned,
            "BUDGET_EXHAUSTED",
            &format!("assessment not run: {denied:?}"),
        ),
        failure => not_assessed(
            planned,
            "MODEL_FAILED",
            &format!("assessment failed: {failure:?}"),
        ),
    }
}

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// One assessment per chunk of the obligation, each leaving `kept` attempts to later
    /// passes; `context_limited` merges them.
    pub(super) async fn assess(&self, planned: &PlannedObligation, kept: u32, out: &mut Outcome) {
        let id = planned.obligation.id;
        let chunks: Vec<usize> = self.verification_packets(id).collect();
        for index in chunks {
            self.assess_chunk(planned, index, kept, out).await;
        }
    }

    async fn assess_chunk(
        &self,
        planned: &PlannedObligation,
        index: usize,
        kept: u32,
        out: &mut Outcome,
    ) {
        let id = planned.obligation.id;
        let packet = &self.packets[index];
        if packet
            .excluded
            .iter()
            .any(|e| e.reason == REQUIRED_OVER_BUDGET)
        {
            // `context_limited` makes it UNKNOWN anyway; no call is spent on it.
            out.record.passes.push(Pass {
                template: prompts::ASSESSMENT.into(),
                packets: vec![index],
                obligation_id: Some(id),
                status: PassStatus::Skipped {
                    reason: "a required span is over the context budget".into(),
                },
                attempt_ids: Vec::new(),
                reused_from: None,
                checkpoint_error: None,
            });
            return;
        }

        let context = self.context(index);
        let prompt = prompts::assessment(&context, planned);
        let outcome = self
            .router
            .leaving::<AssessmentOutput>(Role::Reviewer, &prompt, "obligation_assessment", kept)
            .await;
        record(out, prompts::ASSESSMENT, &[index], Some(id), &outcome);
        let record = match outcome.value {
            Ok(output) => self.checked(validate_assessment(
                output, planned, "ob-1", &context, self.snap,
            )),
            Err(failure) => failed(planned, &failure),
        };
        out.assessments.push(record.assessment.clone());
        out.record.assessments.push(record);
    }

    /// `record` with a violation that rests only on unseen changed code made UNKNOWN.
    fn checked(&self, record: AssessmentRecord) -> AssessmentRecord {
        unseen_violation(record, &unseen_spans(&self.facts.syntax))
    }

    /// One call for discovery over `batch` and the assessments of `linked[assessed]`, leaving
    /// `kept` attempts to later passes; the asks are answered into the returned context for
    /// the verifier.
    pub(super) async fn review(
        &self,
        batch: &[usize],
        linked: &[&PlannedObligation],
        assessed: &[usize],
        mut context: Context,
        kept: u32,
        out: &mut Outcome,
    ) -> (Context, Option<Discovered>) {
        let prompt = prompts::review(&context, linked, assessed, &self.title(batch));
        // A cut multi-unit review is split by the engine instead of asked again.
        let router = &self.router;
        let outcome = if batch.len() > 1 {
            router
                .splittable::<ReviewOutput>(Role::Reviewer, &prompt, "review", kept)
                .await
        } else {
            router
                .leaving::<ReviewOutput>(Role::Reviewer, &prompt, "review", kept)
                .await
        };
        record(out, prompts::REVIEW, batch, None, &outcome);
        let (assessments, discovery) = match outcome.value {
            Ok(output) => {
                let (assessments, discovery) = output.split();
                (Ok(assessments), Some(discovery))
            }
            Err(failure) => (Err(failure), None),
        };
        for &i in assessed {
            let (planned, ob) = (linked[i], format!("ob-{}", i + 1));
            let record = match &assessments {
                Ok(list) => {
                    let mut found = list.iter().filter(|a| a.obligation_id == ob);
                    match (found.next(), found.next()) {
                        (Some(a), None) => self.checked(validate_assessment(
                            a.clone(),
                            planned,
                            &ob,
                            &context,
                            self.snap,
                        )),
                        // Naming what was answered tells an omission from a repeat or a wrong id.
                        _ => not_assessed(
                            planned,
                            "MODEL_FAILED",
                            &format!(
                                "{ob} has no single assessment; the answer assessed {:?}",
                                list.iter()
                                    .map(|a| a.obligation_id.as_str())
                                    .collect::<Vec<_>>()
                            ),
                        ),
                    }
                }
                Err(failure) => failed(planned, failure),
            };
            out.assessments.push(record.assessment.clone());
            out.record.assessments.push(record);
        }
        let ids: Vec<ObligationId> = linked.iter().map(|o| o.obligation.id).collect();
        let key = format!("{}:{batch:?}", prompts::REVIEW);
        let found = discovery.map(|d| Discovered {
            batch: batch.to_vec(),
            ..drop_fixture_claims(
                validate_discovery(d, &key, &context, &ids, self.snap),
                self.fixture_paths,
            )
        });
        if let Some(found) = &found {
            note(found, out);
            self.answer(batch[0], &found.asks, &mut context, out);
        }

        (context, found)
    }

    /// A search over `batch` that leaves `kept` attempts to later passes.
    #[allow(clippy::too_many_arguments)] // one search with its scope and reserve
    pub(super) async fn discover(
        &self,
        batch: &[usize],
        obligation: Option<ObligationId>,
        prompt: &Prompt,
        context: &Context,
        obligations: &[ObligationId],
        kept: u32,
        out: &mut Outcome,
    ) -> Option<Discovered> {
        // The obligation-bound search is the high-risk audit; it may escalate to premium.
        let role = if obligation.is_some() {
            Role::RiskAuditor
        } else {
            Role::Reviewer
        };
        let router = &self.router;
        // A cut multi-unit search is split by the engine instead of asked again.
        let outcome = if batch.len() > 1 {
            router
                .splittable::<DiscoveryOutput>(role, prompt, "discovery", kept)
                .await
        } else {
            router
                .leaving::<DiscoveryOutput>(role, prompt, "discovery", kept)
                .await
        };
        record(out, &prompt.template, batch, obligation, &outcome);
        let key = format!("{}:{batch:?}", prompt.template);
        let found = Discovered {
            batch: batch.to_vec(),
            audit: obligation,
            ..drop_fixture_claims(
                validate_discovery(outcome.value.ok()?, &key, context, obligations, self.snap),
                self.fixture_paths,
            )
        };
        note(&found, out);
        Some(found)
    }

    /// Answers valid asks through the broker's typed operations; answers become spans.
    pub(super) fn answer(
        &self,
        index: usize,
        asks: &[ContextAsk],
        context: &mut Context,
        out: &mut Outcome,
    ) {
        let generated = self.snap.manifest.hidden_paths();
        let ops = Ops {
            facts: &self.facts.semantic,
            evidence: self.evidence,
            reader: self.snap,
            generated: &generated,
        };
        for ask in asks {
            let answered = ask.answer(&ops, context, MAX_REFS_PER_ANSWER);
            out.record.answers.push(AskAnswer {
                packet: index,
                question: ask.question.clone(),
                operation: ask.operation.clone(),
                status: answered.status,
                refs: answered.refs,
                limitations: answered.limitations,
            });
        }
    }

    /// Verdicts for `found`, shown the names their removed BASE lines lose and where an effect
    /// gone from their files happens now; the verifier's
    /// context asks are answered once and the candidates it left UNRESOLVED are verified again
    /// with the answers.
    pub(super) async fn verify(
        &self,
        batch: &[usize],
        obligations: &[&PlannedObligation],
        mut context: Context,
        found: Option<Discovered>,
        out: &mut Outcome,
    ) {
        let Some(found) = found.filter(|f| !f.candidates.is_empty()) else {
            return;
        };
        if !self.verify {
            out.candidates.extend(found.candidates);
            return;
        }

        let anchors: Vec<&CodeRef> = found.candidates.iter().flat_map(|c| &c.anchors).collect();
        context.add_lost_names(&anchors, &self.snap.manifest, self.snap);
        context.add_moves(&self.snap.manifest, self.snap);
        // Searches, not model calls: where an effect gone from these files happens now.
        let moved = effect_asks(&self.facts.ledger.entries, &found.candidates);
        self.answer(batch[0], &moved, &mut context, out);
        let (asks, mut verified) = match self
            .verify_round(batch, obligations, &context, found.candidates.clone(), out)
            .await
        {
            Ok(round) => round,
            Err(failure) => {
                for mut candidate in found.candidates {
                    candidate.verification_status = VerificationStatus::Unresolved;
                    candidate
                        .limitations
                        .push(format!("verification failed: {failure:?}"));
                    out.candidates.push(candidate);
                }
                return;
            }
        };
        // As the first round left them, so the second appends to their verdict history.
        let open: Vec<Candidate> = verified
            .candidates
            .iter()
            .filter(|c| c.verification_status == VerificationStatus::Unresolved)
            .cloned()
            .collect();
        // Left open without a question, a claim is still searched for what refutes it.
        let asks = if asks.is_empty() {
            refuting_asks(&open)
        } else {
            asks
        };
        if !asks.is_empty() && !open.is_empty() {
            self.answer(batch[0], &asks, &mut context, out);
            // A failed second round keeps the first verdicts.
            if let Ok((_, again)) = self
                .verify_round(batch, obligations, &context, open, out)
                .await
            {
                verified
                    .candidates
                    .retain(|c| !again.candidates.iter().any(|a| a.id == c.id));
                verified.candidates.extend(again.candidates);
                verified.findings.extend(again.findings);
            }
        }

        out.candidates.extend(verified.candidates);
        out.findings.extend(verified.findings);
    }

    /// One verifier call over `candidates`: the answered asks and the validated verdicts.
    async fn verify_round(
        &self,
        batch: &[usize],
        obligations: &[&PlannedObligation],
        context: &Context,
        candidates: Vec<Candidate>,
        out: &mut Outcome,
    ) -> Result<(Vec<ContextAsk>, Verified), TaskFailure> {
        let prompt = prompts::verify(context, &candidates, obligations);
        let outcome = self
            .router
            .structured::<VerifyOutput>(Role::Verifier, &prompt, "candidate_verdicts")
            .await;
        record(out, prompts::VERIFY, batch, None, &outcome);
        let scope = batch
            .iter()
            .flat_map(|&i| &self.packets[i].request.review_unit_ids)
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",");
        // The last attempt is the accepted one; a checkpoint answer has none.
        let attempt = outcome.attempts.last().map(|a| a.attempt_id);
        let output = outcome.value?;
        let known: Vec<usize> = candidates.iter().map(|c| c.verdicts.len()).collect();
        let mut verified =
            validate_verdicts(&output, candidates, context, self.snap, &self.known, &scope);
        for (candidate, before) in verified.candidates.iter_mut().zip(known) {
            if candidate.verdicts.len() > before
                && let Some(record) = candidate.verdicts.last_mut()
            {
                record.attempt_id = attempt;
            }
        }
        let (asks, _) = context_asks(output.context_requests);
        Ok((asks, verified))
    }
}

/// Records validated discovery; open questions and candidates over the cap reach the result.
fn note(found: &Discovered, out: &mut Outcome) {
    let overflow = found
        .rejected
        .iter()
        .filter(|r| r.problems.iter().any(|p| p.starts_with(CANDIDATE_OVERFLOW)))
        .count();
    if overflow > 0 {
        out.limit_reached = true;
        out.gaps.push(format!(
            "{CANDIDATE_OVERFLOW}: {overflow} candidates over the cap were dropped"
        ));
    }

    out.questions.extend(
        found
            .unresolved_questions
            .iter()
            .map(|q| format!("open question: {q}")),
    );
    out.record.discovered.push(found.clone());
}

/// Records the pass and its attempts.
pub(super) fn record<T>(
    out: &mut Outcome,
    template: &str,
    packets: &[usize],
    obligation: Option<ObligationId>,
    outcome: &RoutedOutcome<T>,
) {
    out.record.attempts.extend(outcome.attempts.iter().cloned());
    out.record.hops.extend(outcome.hops.iter().cloned());
    let status = match &outcome.value {
        Ok(_) => PassStatus::Done,
        Err(failure) => {
            out.limit_reached |= failure.quota_exhausted()
                || matches!(failure, TaskFailure::Budget { .. } | TaskFailure::Truncated);
            // The router already tried every eligible route, so a failed pass is final.
            let gap = if failure.quota_exhausted() {
                format!("{MODEL_QUOTA_EXHAUSTED}: {QUOTA_MESSAGE}")
            } else {
                format!("{template} pass on packets {packets:?} did not complete: {failure:?}")
            };
            if !out.gaps.contains(&gap) {
                out.gaps.push(gap);
            }

            PassStatus::Failed {
                failure: failure.clone(),
            }
        }
    };
    out.record.passes.push(Pass {
        template: template.to_owned(),
        packets: packets.to_vec(),
        obligation_id: obligation,
        status,
        attempt_ids: outcome.attempts.iter().map(|a| a.attempt_id).collect(),
        reused_from: outcome.reused_from,
        checkpoint_error: outcome.unkept.clone(),
    });
}
