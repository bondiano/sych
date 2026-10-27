//! Migration obligations bound to the deployment model (spec §13, roadmap M3.5): each MIG-*
//! rule is assessed in `PLAN_REVIEW` mode over the rollout states it is about. A state is
//! dropped only by a sourced constraint; an owner input the rule depends on that is missing
//! keeps it `UNKNOWN` with that gap, never filled in by an assumption.

use crate::deploy::{Reachability, TransitionMatrix, TransitionState};
use crate::{Applicability, EvaluationMode, ObligationStatus, PlannedObligation};

/// Missing deployment input the owner must provide.
pub const DEPLOYMENT_INPUT_MISSING: &str = "DEPLOYMENT_INPUT_MISSING";

/// The rollout states and the owner inputs a migration rule depends on.
fn scope(rule_id: &str) -> Option<(&'static [TransitionState], &'static [&'static str])> {
    use TransitionState::{
        BackfillRetry, ContractPhase, MixedApps, OldAppExpandedSchema, OldFrontendNewBackend,
        PartialBackfillConcurrentWrites, RecoveryAfterNewWrites,
    };
    Some(match rule_id {
        "MIG-COMPAT" => (
            &[OldAppExpandedSchema, MixedApps, OldFrontendNewBackend],
            &[],
        ),
        "MIG-RESTART" => (&[PartialBackfillConcurrentWrites, BackfillRetry], &[]),
        "MIG-CONTRACT" => (
            &[ContractPhase],
            &["COMPATIBILITY_WINDOW_UNKNOWN", "EXTERNAL_CONSUMERS_UNKNOWN"],
        ),
        "MIG-RECOVERY" => (&[RecoveryAfterNewWrites], &["RECOVERY_UNKNOWN"]),
        _ => return None,
    })
}

/// Binds a MIG-* obligation to the matrix; any other obligation is left as it is.
pub(crate) fn bind(planned: &mut PlannedObligation, matrix: &TransitionMatrix) {
    let o = &mut planned.obligation;
    let Some((states, needs)) = scope(&o.rule_id) else {
        return;
    };
    o.evaluation_mode = Some(EvaluationMode::PlanReview);
    let rows = matrix.rows.iter().filter(|r| states.contains(&r.state));
    let (reachable, excluded): (Vec<_>, Vec<_>) =
        rows.partition(|r| r.reachability == Reachability::Reachable);
    o.limitations
        .extend(excluded.iter().filter_map(|r| match &r.reachability {
            Reachability::Excluded { source } => {
                Some(format!("{:?} excluded by: {source}", r.state))
            }
            Reachability::Reachable => None,
        }));
    if reachable.is_empty() {
        o.applicability = Applicability::DoesNotApply;
        o.status = ObligationStatus::NotApplicable {
            basis: format!(
                "every rollout state it covers is excluded: {}",
                o.limitations.join("; ")
            ),
        };
        return;
    }

    planned.criteria.extend(
        reachable
            .iter()
            .map(|r| format!("In state {:?}: {}.", r.state, r.obligation)),
    );
    planned.criteria.push(
        "PLAN_REVIEW: judge the proposed order and the mechanisms that enforce its preconditions \
         (for example a gate on backfill completion); SUPPORTED means the plan is conditionally \
         sound, not that it has run."
            .into(),
    );
    planned.obligation.acceptance_criterion = planned.criteria.join("; ");
    let o = &mut planned.obligation;
    let (blocking, other): (Vec<&String>, Vec<&String>) = matrix
        .missing_inputs
        .iter()
        .partition(|g| needs.iter().any(|n| g.starts_with(n)));
    o.limitations.extend(other.into_iter().cloned());
    if !blocking.is_empty() && o.applicability == Applicability::Applies {
        o.status = ObligationStatus::Unknown {
            reason_code: DEPLOYMENT_INPUT_MISSING.into(),
            next_evidence_needed: blocking
                .iter()
                .map(|g| g.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        };
    }
}
