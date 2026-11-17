//! Further discovery rounds over one batch (`context.expansion_rounds`). The context asks of
//! the last round are answered first, each operation once per batch; while that added spans,
//! discovery is asked again with them and with what it already proposed. A round is one more search attempt that leaves the
//! verifier its reserve, and a round that fails or finds nothing new keeps what came before.

use sych_core::analysis::{AskOperation, Context, Discovered, expansion};
use sych_core::model::{Ledger, ModelProvider};
use sych_core::{ObligationId, PlannedObligation};

use super::{Engine, Outcome};

/// `more`'s output joins `found`; a claim already proposed in so many words is dropped.
fn join(found: &mut Discovered, more: Discovered) {
    for candidate in more.candidates {
        if !found.candidates.iter().any(|c| c.claim == candidate.claim) {
            found.candidates.push(candidate);
        }
    }
    found.rejected.extend(more.rejected);
    found.rejected_asks.extend(more.rejected_asks);
    for question in more.unresolved_questions {
        if !found.unresolved_questions.contains(&question) {
            found.unresolved_questions.push(question);
        }
    }
    found.asks = more.asks;
}

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// `found`'s rounds, then verification of everything they found.
    pub(super) async fn expand_and_verify(
        &self,
        batch: &[usize],
        linked: &[&PlannedObligation],
        mut context: Context,
        found: Option<Discovered>,
        out: &mut Outcome,
    ) {
        let found = self.expand(batch, linked, &mut context, found, out).await;
        self.verify(batch, linked, context, found, out).await;
    }

    /// Answers `found`'s asks into `context` and runs the further rounds they earn.
    async fn expand(
        &self,
        batch: &[usize],
        linked: &[&PlannedObligation],
        context: &mut Context,
        found: Option<Discovered>,
        out: &mut Outcome,
    ) -> Option<Discovered> {
        let mut found = found?;
        let ids: Vec<ObligationId> = linked.iter().map(|o| o.obligation.id).collect();
        let mut asks = found.asks.clone();
        let mut asked: Vec<AskOperation> = Vec::new();
        for round in 0..=self.expansion_rounds {
            // An ask answered in an earlier round shows nothing new.
            asks.retain(|a| !asked.contains(&a.operation));
            asked.extend(asks.iter().map(|a| a.operation.clone()));
            let shown = context.spans();
            self.answer(batch[0], &asks, context, out);
            if round == self.expansion_rounds || context.spans() == shown {
                break;
            }

            let prompt =
                expansion::expansion(context, linked, &self.title(batch), &found.candidates);
            let Some(more) = self
                .discover(batch, None, &prompt, context, &ids, 1, out)
                .await
            else {
                break;
            };
            asks.clone_from(&more.asks);
            join(&mut found, more);
        }
        Some(found)
    }
}
