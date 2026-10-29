//! The router over the real `SQLite` ledger: a released attempt fails honestly and still
//! counts against the run's attempt cap, and a session cap survives the UTC month boundary.

use std::cell::{Cell, RefCell};

use serde_json::Value;
use sych_core::config::{Budget, Budgets, Role};
use sych_core::model::{
    ChargeState, ErrorKind, Limits, ModelReply, ModelRequest, Prompt, ProviderError, Purpose,
    TaskFailure,
};
use sych_core::{PolicySnapshotId, SourceSnapshotId, Timestamp, sha256};
use sych_storage::{Partition, RunLedger, Storage, StoreConfig, TENANT_LOCAL};

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

thread_local! {
    static CLOCK: Cell<i64> = const { Cell::new(0) };
}

fn clock() -> Timestamp {
    Timestamp::from_second(CLOCK.with(Cell::get)).expect("timestamp")
}

fn set_clock(at: &str) {
    let at: Timestamp = at.parse().expect("timestamp");
    CLOCK.with(|c| c.set(at.as_second()));
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "scripted answers share the provider signature"
)]
fn empty_object(_: &ModelRequest) -> Result<ModelReply, ProviderError> {
    Ok(reply("{}"))
}

#[tokio::test(flavor = "current_thread")]
async fn a_session_attempt_cap_survives_the_month_boundary_through_the_router() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = StoreConfig {
        database: dir.path().join("sych.db"),
        artifacts: dir.path().join("artifacts"),
    };
    Storage::migrate(&store).await.expect("migrate");
    let storage = Storage::open(&store).await.expect("open");
    set_clock("2026-10-31T23:59:58Z");
    let run = storage
        .begin_run(
            &Partition {
                tenant_id: TENANT_LOCAL.into(),
                access_scope: "local:repo".into(),
            },
            "change",
            &SourceSnapshotId(sha256(b"s")),
            &PolicySnapshotId(sha256(b"p")),
            clock(),
        )
        .await
        .expect("run");
    let ledger = RunLedger::new(&storage, &run, clock).scoped("followup:t:a".into());
    let provider = Scripted {
        answer: empty_object,
        calls: RefCell::new(Vec::new()),
    };
    let mut router = router(&provider, &ledger, run.id, &Budgets::default());
    router.now = clock;
    router.purpose = Purpose::Followup;
    router.limits = Limits {
        run: Budget {
            max_attempts: Some(1),
            ..Budget::default()
        },
        daily_money: None,
        monthly_money: None,
    };
    let prompt = Prompt {
        template: "followup@1".into(),
        role: "r".into(),
        output_contract: "{}".into(),
        rules: Vec::new(),
        task: "t".into(),
        data: Vec::new(),
    };

    let october = router
        .structured::<Value>(Role::Reviewer, &prompt, "answer")
        .await;
    assert!(october.value.is_ok(), "{:?}", october.value);
    set_clock("2026-11-01T00:00:01Z");
    let november = router
        .structured::<Value>(Role::Reviewer, &prompt, "answer")
        .await;
    assert!(
        matches!(november.value, Err(TaskFailure::Budget { .. })),
        "a new month does not give the session another attempt: {:?}",
        november.value
    );
    assert_eq!(provider.calls.borrow().len(), 1);
}
