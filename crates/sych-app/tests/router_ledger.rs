//! The router over the real `SQLite` ledger: a released attempt fails honestly and still
//! counts against the run's attempt cap.

use sych_core::config::Budgets;
use sych_core::model::{ChargeState, ErrorKind, ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

fn unsupported(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Err(ProviderError {
        kind: ErrorKind::Unsupported,
        dispatched: false,
        message: "no upstream supports the schema".into(),
        retry_after_seconds: None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn unsupported_route_is_a_released_failure_not_an_empty_success() {
    let (outcome, _) = run(unsupported, Budgets::default()).await;
    assert_eq!(reason(&outcome.assessments[0].status), "MODEL_FAILED");
    assert!(
        outcome
            .record
            .attempts
            .iter()
            .all(|a| a.charge_state == ChargeState::Released)
    );
    assert_eq!(outcome.gaps.len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn a_released_attempt_uses_up_the_attempt_cap_of_the_sqlite_ledger() {
    let mut budgets = Budgets::default();
    budgets.analysis.max_attempts = Some(1);
    let (outcome, _) = run(unsupported, budgets).await;
    let attempts = &outcome.record.attempts;
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(attempts[0].charge_state, ChargeState::Released);
    assert!(outcome.limit_reached);
}
