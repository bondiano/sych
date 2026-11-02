//! Independent passes run `parallel` at a time, each into its own part of the outcome; the
//! parts are merged in the order the passes were planned, so the result does not depend on
//! which call returns first. The ledger reserves every attempt atomically, so concurrent
//! calls share the run budget as sequential ones do.

use futures_util::future::LocalBoxFuture;
use futures_util::stream::{self, StreamExt};
use sych_core::PlannedObligation;
use sych_core::SchemaVersion;
use sych_core::analysis::{Context, Discovered};
use sych_core::model::{Ledger, ModelProvider};

use super::{AnalysisRecord, Engine, Outcome, PassStatus};

/// A pass and its part of the outcome.
pub(super) type Task<'a, T> = LocalBoxFuture<'a, (Outcome, T)>;

/// A discovery batch that assesses obligations it shows whole: its context, the obligations
/// linked to it and the indices of those it assesses.
pub(super) type Combined<'a> = (
    &'a Vec<usize>,
    Context,
    Vec<&'a PlannedObligation>,
    Vec<usize>,
);

/// A discovery batch that also assessed obligations, waiting for verification.
pub(super) type Reviewed<'a> = (
    &'a Vec<usize>,
    Vec<&'a PlannedObligation>,
    Context,
    Option<Discovered>,
);

/// A review and the obligations it assessed, which are assessed alone when it is cut short;
/// a standalone assessment has none.
pub(super) type Assessing<'a> = Option<(Reviewed<'a>, Vec<&'a PlannedObligation>)>;

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// An empty outcome for one pass.
    pub(super) fn part(&self) -> Outcome {
        Outcome {
            assessments: Vec::new(),
            candidates: Vec::new(),
            findings: Vec::new(),
            record: AnalysisRecord {
                schema_version: SchemaVersion,
                plan: self.router.plan.clone(),
                routes: Vec::new(),
                passes: Vec::new(),
                assessments: Vec::new(),
                discovered: Vec::new(),
                answers: Vec::new(),
                candidates: Vec::new(),
                attempts: Vec::new(),
                hops: Vec::new(),
            },
            limit_reached: false,
            gaps: Vec::new(),
            questions: Vec::new(),
        }
    }

    /// Runs `tasks` at most `parallel` at a time, results in task order; tasks start in order,
    /// so earlier ones still reserve the budget first.
    pub(super) async fn each<T>(&self, tasks: Vec<Task<'_, T>>) -> Vec<(Outcome, T)> {
        stream::iter(tasks)
            .buffered(self.parallel.max(1))
            .collect()
            .await
    }
}

impl Outcome {
    /// Appends a pass cut short that is done again over smaller batches: its calls stay
    /// accounted, its gaps and unfinished assessments do not.
    pub(super) fn absorb_cut(&mut self, part: Outcome) {
        let (r, p) = (&mut self.record, part.record);
        r.passes.extend(p.passes.into_iter().map(|mut pass| {
            if let PassStatus::Failed { failure } = pass.status {
                pass.status = PassStatus::Split { failure };
            }

            pass
        }));
        r.attempts.extend(p.attempts);
        r.hops.extend(p.hops);
    }

    /// Appends one pass's part; same-location findings are merged at the end, most severe kept.
    pub(super) fn absorb(&mut self, part: Outcome) {
        self.assessments.extend(part.assessments);
        self.candidates.extend(part.candidates);
        self.findings.extend(part.findings);
        let (r, p) = (&mut self.record, part.record);
        r.passes.extend(p.passes);
        r.assessments.extend(p.assessments);
        r.discovered.extend(p.discovered);
        r.answers.extend(p.answers);
        r.attempts.extend(p.attempts);
        r.hops.extend(p.hops);
        self.limit_reached |= part.limit_reached;
        self.gaps.extend(part.gaps);
        self.questions.extend(part.questions);
    }
}
