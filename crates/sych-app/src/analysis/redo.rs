//! Passes done again: a search denied its input tokens runs alone once more, a wide review or
//! discovery cut short is redone over smaller batches, and one the provider turned away is
//! asked once more after the cooldown, instead of leaving a gap.

use std::time::Duration;

use tokio::time::sleep;

use sych_core::PlannedObligation;
use sych_core::analysis::prompts;
use sych_core::model::{Denied, ErrorKind, Ledger, ModelProvider, TaskFailure};

use super::parts::Task;
use super::{Engine, Outcome, PassStatus};

/// The search's discovery was denied by the run's input-token cap, so it made no call.
pub(super) fn discovery_out_of_tokens(part: &Outcome) -> bool {
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

/// A review or discovery the provider turned away this time: rate limited, a transient error,
/// or every route of the chain cooling down. The same call may succeed a little later.
pub(super) fn refused(part: &Outcome) -> bool {
    part.record.passes.iter().any(|p| {
        (p.template == prompts::REVIEW || p.template == prompts::DISCOVERY)
            && matches!(
                &p.status,
                PassStatus::Failed {
                    failure: TaskFailure::NoEligibleRoute { .. }
                        | TaskFailure::Provider {
                            kind: ErrorKind::RateLimited | ErrorKind::Transient,
                            ..
                        }
                }
            )
    })
}

/// A multi-unit review or discovery whose reply was cut or timed out: smaller batches may fit.
pub(super) fn cut_short(part: &Outcome) -> bool {
    part.record.passes.iter().any(|p| {
        (p.template == prompts::REVIEW || p.template == prompts::DISCOVERY)
            && p.packets.len() > 1
            && matches!(
                &p.status,
                PassStatus::Failed {
                    failure: TaskFailure::Truncated
                        | TaskFailure::Provider {
                            kind: ErrorKind::Timeout,
                            ..
                        }
                }
            )
    })
}

pub(super) fn halves(batch: &[usize]) -> [Vec<usize>; 2] {
    let (a, b) = batch.split_at(batch.len() / 2);
    [a.to_vec(), b.to_vec()]
}

impl<P: ModelProvider, L: Ledger> Engine<'_, P, L> {
    /// Obligations of a review cut short, each in assessments of its own.
    pub(super) async fn assess_alone(
        &self,
        obligations: Vec<&PlannedObligation>,
        kept: u32,
        out: &mut Outcome,
    ) {
        let tasks: Vec<Task<'_, ()>> = obligations
            .into_iter()
            .map(|obligation| -> Task<'_, ()> {
                Box::pin(async move {
                    let mut part = self.part();
                    self.assess(obligation, kept, &mut part).await;
                    (part, ())
                })
            })
            .collect();
        for (part, ()) in self.each(tasks).await {
            out.absorb(part);
        }
    }

    /// The audits, verifications and searches; a search denied its input tokens runs again
    /// alone, one cut short runs again in halves, and one the provider turned away runs once
    /// more after the cooldown.
    pub(super) async fn searches<'s>(
        &'s self,
        searching: Vec<Task<'s, Option<&'s Vec<usize>>>>,
        out: &mut Outcome,
    ) {
        let mut retry = Vec::new();
        let mut smaller = Vec::new();
        let mut refusals = Vec::new();
        for (part, batch) in self.each(searching).await {
            match batch {
                Some(batch) if discovery_out_of_tokens(&part) => {
                    out.record.attempts.extend(part.record.attempts);
                    out.record.hops.extend(part.record.hops);
                    retry.push(batch);
                }
                Some(batch) if cut_short(&part) => {
                    out.absorb_cut(part);
                    smaller.extend(halves(batch));
                }
                Some(batch) if refused(&part) => refusals.push((part, batch)),
                _ => out.absorb(part),
            }
        }
        // A sibling in flight held its whole byte bound; settled, it may have left room.
        for batch in retry {
            out.absorb(self.search(batch).await);
        }
        self.search_smaller(smaller, out).await;
        self.search_again(refusals, out).await;
    }

    /// Searches the provider turned away, once more after the shortest cooldown; when the
    /// deadline comes first, each stays the gap it is. What fails again is a gap.
    async fn search_again<'s>(
        &'s self,
        refusals: Vec<(Outcome, &'s Vec<usize>)>,
        out: &mut Outcome,
    ) {
        if refusals.is_empty() {
            return;
        }

        if !self.rested().await {
            for (part, _) in refusals {
                out.absorb(part);
            }
            return;
        }

        let again: Vec<Task<'_, ()>> = refusals
            .into_iter()
            .map(|(part, batch)| -> Task<'_, ()> {
                out.absorb_cut(part);
                Box::pin(async move { (self.search(batch).await, ()) })
            })
            .collect();
        for (part, ()) in self.each(again).await {
            out.absorb(part);
        }
    }

    /// Waits until the route that rests the shortest may take tasks again; false when the run's
    /// deadline has passed or comes first, so the refusal stays the gap it is.
    async fn rested(&self) -> bool {
        let router = &self.router;
        let now = (router.now)();
        if router.deadline.is_some_and(|d| now >= d) {
            return false;
        }

        let until = router
            .cooldowns
            .borrow()
            .values()
            .copied()
            .filter(|t| *t > now)
            .min();
        let Some(until) = until else {
            return true;
        };
        if router.deadline.is_some_and(|d| until >= d) {
            return false;
        }

        let wait = until
            .duration_since(now)
            .try_into()
            .unwrap_or(Duration::ZERO);
        sleep(wait).await;
        true
    }

    /// Batches cut short, searched again in halves down to single units.
    async fn search_smaller(&self, mut smaller: Vec<Vec<usize>>, out: &mut Outcome) {
        while !smaller.is_empty() {
            let round: Vec<Task<'_, Vec<usize>>> = smaller
                .drain(..)
                .map(|batch| -> Task<'_, Vec<usize>> {
                    Box::pin(async move { (self.search(&batch).await, batch) })
                })
                .collect();
            for (part, batch) in self.each(round).await {
                if cut_short(&part) {
                    out.absorb_cut(part);
                    smaller.extend(halves(&batch));
                } else {
                    out.absorb(part);
                }
            }
        }
    }
}
