//! The coverage audit after the planned passes: one call over the change when its facts call
//! for it, then an assessment of each accepted proposal. Neither lowers coverage: what does not
//! run, or is refused, is an open question.

use sych_core::analysis::coverage::{self, COVERAGE_AUDIT, CoverageOutput};
use sych_core::analysis::{AssessmentOutput, prompts, validate_assessment};
use sych_core::config::Role;
use sych_core::model::{Ledger, ModelProvider, RoutedOutcome};
use sych_core::{ObligationId, ObligationStatus, PlannedObligation};

use super::{Engine, Outcome, Pass, PassStatus};

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// Audits the view of `batch`, the first discovery batch, when a signal calls for it.
    pub(super) async fn coverage_audit(&self, batch: Option<&Vec<usize>>, out: &mut Outcome) {
        let signals = coverage::signals(self.facts, self.plan);
        let Some(batch) = batch.filter(|_| self.risk_audit && self.verify && !signals.is_empty())
        else {
            return;
        };
        let (context, _) = self.batch_view(batch);
        let planned: Vec<(&PlannedObligation, &ObligationStatus)> = self
            .plan
            .obligations
            .iter()
            .map(|o| {
                let now = out
                    .assessments
                    .iter()
                    .find(|a| a.rule_id == o.obligation.rule_id)
                    .map_or(&o.obligation.status, |a| &a.status);
                (o, now)
            })
            .collect();
        let found: Vec<String> = out.findings.iter().map(|f| f.title.clone()).collect();
        let prompt = coverage::prompt(&context, &planned, &signals, &found);
        let outcome = self
            .router
            .leaving::<CoverageOutput>(Role::RiskAuditor, &prompt, "coverage_proposals", 1)
            .await;
        quietly(out, COVERAGE_AUDIT, batch, None, &outcome);
        let output = match outcome.value {
            Ok(output) => output,
            Err(failure) => {
                out.questions
                    .push(format!("coverage audit did not run: {failure:?}"));
                return;
            }
        };
        let (accepted, refused) = coverage::accept(
            output,
            &context,
            self.snap,
            self.plan,
            &self.snap.snapshot.id,
        );
        out.questions.extend(refused);
        for proposed in accepted {
            let prompt = prompts::assessment(&context, &proposed);
            let outcome = self
                .router
                .leaving::<AssessmentOutput>(Role::Reviewer, &prompt, "obligation_assessment", 0)
                .await;
            let id = proposed.obligation.id;
            quietly(out, prompts::ASSESSMENT, batch, Some(id), &outcome);
            match outcome.value {
                Ok(output) => {
                    let record = self.checked(validate_assessment(
                        output, &proposed, "ob-1", &context, self.snap,
                    ));
                    out.assessments.push(record.assessment.clone());
                    out.record.assessments.push(record);
                    out.extra_obligations.push(proposed);
                }
                Err(failure) => out.questions.push(format!(
                    "coverage audit: {} (not checked: {failure:?})",
                    proposed.obligation.question
                )),
            }
        }
    }
}

/// `record` without its consequences: the attempts are accounted, but a failure is no gap and
/// no failed pass, since nothing the plan required went unchecked.
fn quietly<T>(
    out: &mut Outcome,
    template: &str,
    packets: &[usize],
    obligation: Option<ObligationId>,
    outcome: &RoutedOutcome<T>,
) {
    out.record.attempts.extend(outcome.attempts.iter().cloned());
    out.record.hops.extend(outcome.hops.iter().cloned());
    out.record.passes.push(Pass {
        template: template.to_owned(),
        packets: packets.to_vec(),
        obligation_id: obligation,
        status: match &outcome.value {
            Ok(_) => PassStatus::Done,
            Err(failure) => PassStatus::Skipped {
                reason: format!("an optional check did not complete: {failure:?}"),
            },
        },
        attempt_ids: outcome.attempts.iter().map(|a| a.attempt_id).collect(),
        reused_from: outcome.reused_from,
        checkpoint_error: outcome.unkept.clone(),
    });
}
