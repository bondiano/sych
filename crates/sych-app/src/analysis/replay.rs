//! The verifier alone, over candidates frozen from an earlier run of the same snapshot: an old
//! and a new verifier judge identical candidates, whatever discovery would find this time.

use sych_core::analysis::{Discovered, merge_overlapping};
use sych_core::model::{Ledger, ModelProvider};
use sych_core::{PlannedObligation, VerificationStatus};

use super::{Engine, Outcome};

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// Verifies each frozen discovery on the context its pass built: the batch view, or the
    /// audited obligation's packet, with the discovery's own context asks answered again.
    /// Nothing is assessed, so only candidates and findings mean anything in the result.
    pub async fn replay(&self, frozen: Vec<Discovered>) -> Outcome {
        let mut out = self.part();
        out.record.routes = self.routes();
        for mut found in frozen {
            // A record written before discoveries named their packets cannot be rebuilt.
            let Some(&first) = found.batch.first() else {
                continue;
            };
            for candidate in &mut found.candidates {
                candidate.verification_status = VerificationStatus::Pending;
                candidate.limitations.clear();
            }
            let batch = found.batch.clone();
            let (mut context, linked): (_, Vec<&PlannedObligation>) = match found.audit {
                Some(id) => (
                    self.context(first),
                    self.plan
                        .obligations
                        .iter()
                        .filter(|o| o.obligation.id == id)
                        .collect(),
                ),
                None => self.batch_view(&batch),
            };
            self.answer(first, &found.asks, &mut context, &mut out);
            self.verify(&batch, &linked, context, Some(found), &mut out)
                .await;
        }
        out.findings = merge_overlapping(out.findings);
        out.record.candidates.clone_from(&out.candidates);
        out
    }
}
