//! Model ledger on the selected backend: concurrent reservations, idempotent
//! settlement, unknown charges, monthly subscription fees and billing mismatch.

use std::sync::Arc;

use sych_core::config::{BillingKind, Budget, Enforcement, FixedFee};
use sych_core::model::{
    AttemptKind, Capabilities, Denied, Limits, ObservedUsage, Purpose, Reservation, Route,
    Settlement,
};
use sych_core::{
    AttemptId, MicroUsd, PolicySnapshotId, RunId, SourceSnapshotId, Timestamp, sha256,
};
use sych_storage::{Partition, Storage, TENANT_LOCAL};
use tokio::task::JoinSet;

#[path = "support/database.rs"]
mod database;

use database::TestDir;

const T0: i64 = 1_800_000_000;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_second(T0 + offset).expect("timestamp")
}

async fn setup() -> (TestDir, Storage, RunId) {
    let dir = TestDir::new().await;
    Storage::migrate(dir.config()).await.expect("migrate");
    let storage = Storage::open(dir.config()).await.expect("open");
    let partition = Partition {
        tenant_id: TENANT_LOCAL.into(),
        access_scope: "local:repo".into(),
    };
    let run = storage
        .begin_run(
            &partition,
            "change",
            &SourceSnapshotId(sha256(b"s")),
            &PolicySnapshotId(sha256(b"p")),
            at(0),
        )
        .await
        .expect("begin");
    (dir, storage, run.id)
}

fn route(billing: BillingKind) -> Route {
    let subscription = billing == BillingKind::SubscriptionQuota;
    Route {
        profile: "p".into(),
        provider: if subscription { "gpt" } else { "openrouter" }.into(),
        adapter: "test".into(),
        model: "m".into(),
        billing,
        enforcement: if subscription {
            Enforcement::SubscriptionObservable
        } else {
            Enforcement::ApiHard
        },
        capabilities: Capabilities {
            structured_output: true,
            context_tokens: None,
            output_cap_enforced: !subscription,
            usage_reported: true,
            cost_reported: true,
            hidden_calls: false,
        },
    }
}

fn reservation(run: RunId, money: u64) -> Reservation {
    Reservation {
        attempt_id: AttemptId::new_v7(),
        tenant_id: TENANT_LOCAL.into(),
        run_id: run,
        purpose: Purpose::Analysis,
        kind: AttemptKind::Initial,
        template: "discovery@1".into(),
        route: route(BillingKind::ApiUsage),
        money: MicroUsd(money),
        input_tokens: 10,
        output_tokens: Some(10),
        fixed_fee: None,
        at: at(60),
    }
}

fn subscription(run: RunId) -> Reservation {
    Reservation {
        route: route(BillingKind::SubscriptionQuota),
        output_tokens: None,
        fixed_fee: Some(FixedFee {
            amount_micro_usd: MicroUsd(2_000),
            allocated_micro_usd: MicroUsd(800),
        }),
        ..reservation(run, 0)
    }
}

fn limits(run_money: u64, monthly: u64) -> Limits {
    Limits {
        run: Budget {
            max_cost_micro_usd: Some(MicroUsd(run_money)),
            ..Budget::default()
        },
        daily_money: Some(MicroUsd(run_money)),
        monthly_money: Some(MicroUsd(monthly)),
    }
}

fn settled(charge: u64) -> Settlement {
    Settlement::Settled {
        usage: ObservedUsage {
            input_tokens: Some(5),
            output_tokens: Some(5),
            reasoning_tokens: None,
        },
        charge: Some(MicroUsd(charge)),
        reference: None,
        resolved_model: Some("m-2026".into()),
        generation_id: None,
    }
}

#[tokio::test]
async fn concurrent_reservations_never_exceed_the_cap() {
    let (_dir, storage, run) = setup().await;
    let storage = Arc::new(storage);
    let mut tasks = JoinSet::new();
    for _ in 0..20 {
        let storage = Arc::clone(&storage);
        tasks.spawn(async move {
            storage
                .reserve_attempt(&reservation(run, 100), &limits(1_000, 10_000))
                .await
                .expect("ledger")
                .is_ok()
        });
    }
    let admitted = tasks.join_all().await.into_iter().filter(|ok| *ok).count();
    assert_eq!(admitted, 10);
}

/// A server engine and a review thread each open their own pool on one database; reservations
/// from both must wait for the writer lock.
#[tokio::test]
async fn reservations_from_two_connections_wait_for_the_lock() {
    let (dir, first, run) = setup().await;
    let second = Storage::open(dir.config()).await.expect("open");
    let pools = [Arc::new(first), Arc::new(second)];
    let mut tasks = JoinSet::new();
    for i in 0..40 {
        let storage = Arc::clone(&pools[i % 2]);
        tasks.spawn(async move {
            storage
                .reserve_attempt(&reservation(run, 100), &limits(1_000, 10_000))
                .await
                .map(|admitted| admitted.is_ok())
        });
    }
    let results = tasks.join_all().await;
    let admitted = results
        .into_iter()
        .map(|r| r.expect("no SQLITE_BUSY"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(admitted, 10);
}

#[tokio::test]
async fn settlement_is_idempotent_and_unknown_holds_the_reservation() {
    let (_dir, storage, run) = setup().await;
    let caps = limits(1_000, 10_000);
    let a = reservation(run, 600);
    assert_eq!(
        storage.reserve_attempt(&a, &caps).await.expect("ledger"),
        Ok(())
    );
    let unknown = Settlement::Unknown {
        usage: ObservedUsage::default(),
    };
    storage
        .settle_attempt(a.attempt_id, &unknown, at(61))
        .await
        .expect("settle");
    // The unknown charge still counts, so a second large reservation is denied.
    let denied = storage
        .reserve_attempt(&reservation(run, 600), &caps)
        .await
        .expect("ledger");
    assert!(matches!(denied, Err(Denied::Money { .. })));
    // Reconciliation to a known charge frees the difference; repeated settlement changes nothing.
    storage
        .settle_attempt(a.attempt_id, &settled(100), at(62))
        .await
        .expect("settle");
    storage
        .settle_attempt(a.attempt_id, &settled(900), at(63))
        .await
        .expect("settle");
    storage
        .settle_attempt(a.attempt_id, &unknown, at(64))
        .await
        .expect("settle");
    let fits = storage
        .reserve_attempt(&reservation(run, 900), &caps)
        .await
        .expect("ledger");
    assert_eq!(fits, Ok(()), "spent 100 + 900 = cap");

    let released = reservation(run, 0);
    storage
        .reserve_attempt(&released, &caps)
        .await
        .expect("ledger")
        .expect("fits");
    storage
        .settle_attempt(released.attempt_id, &Settlement::Released, at(65))
        .await
        .expect("settle");
    let missing = storage
        .settle_attempt(AttemptId::new_v7(), &settled(1), at(66))
        .await;
    assert!(missing.is_err(), "unknown attempt ids are rejected");
}

#[tokio::test]
async fn a_released_attempt_still_counts_against_the_attempt_cap() {
    let (_dir, storage, run) = setup().await;
    let caps = Limits {
        run: Budget {
            max_attempts: Some(1),
            ..Budget::default()
        },
        ..limits(1_000, 10_000)
    };
    let first = reservation(run, 100);
    storage
        .reserve_attempt(&first, &caps)
        .await
        .expect("ledger")
        .expect("fits");
    storage
        .settle_attempt(first.attempt_id, &Settlement::Released, at(61))
        .await
        .expect("settle");
    let second = storage
        .reserve_attempt(&reservation(run, 100), &caps)
        .await
        .expect("ledger");
    assert_eq!(second, Err(Denied::Attempts { cap: 1 }));
}

#[tokio::test]
async fn subscription_fee_is_charged_once_per_month_against_the_pool() {
    let (_dir, storage, run) = setup().await;
    let caps = limits(1_000, 1_000);
    for _ in 0..3 {
        let s = subscription(run);
        assert_eq!(
            storage.reserve_attempt(&s, &caps).await.expect("ledger"),
            Ok(())
        );
    }
    // 800 allocated fee + 300 metered would exceed the 1000 monthly pool.
    let denied = storage
        .reserve_attempt(&reservation(run, 300), &caps)
        .await
        .expect("ledger");
    let Err(Denied::Money {
        account, committed, ..
    }) = denied
    else {
        panic!("{denied:?}")
    };
    assert_eq!((account.as_str(), committed), ("month", MicroUsd(800)));
    let fits = storage
        .reserve_attempt(&reservation(run, 200), &caps)
        .await
        .expect("ledger");
    assert_eq!(fits, Ok(()));
}

#[tokio::test]
async fn unexpected_subscription_charge_blocks_further_subscription_calls() {
    let (_dir, storage, run) = setup().await;
    let caps = limits(10_000, 10_000);
    let s = subscription(run);
    storage
        .reserve_attempt(&s, &caps)
        .await
        .expect("ledger")
        .expect("fits");
    storage
        .settle_attempt(s.attempt_id, &settled(50), at(61))
        .await
        .expect("settle");
    let next = storage
        .reserve_attempt(&subscription(run), &caps)
        .await
        .expect("ledger");
    assert_eq!(next, Err(Denied::BillingMismatch));
    let api = storage
        .reserve_attempt(&reservation(run, 10), &caps)
        .await
        .expect("ledger");
    assert_eq!(api, Ok(()), "a separate metered route is not blocked");
}

#[tokio::test]
async fn ledger_survives_reopen() {
    let (dir, storage, run) = setup().await;
    let caps = limits(1_000, 10_000);
    storage
        .reserve_attempt(&reservation(run, 700), &caps)
        .await
        .expect("ledger")
        .expect("fits");
    drop(storage);
    let storage = Storage::open(dir.config()).await.expect("reopen");
    let denied = storage
        .reserve_attempt(&reservation(run, 700), &caps)
        .await
        .expect("ledger");
    assert!(matches!(denied, Err(Denied::Money { .. })));
}

#[tokio::test]
async fn a_follow_up_cap_is_per_session() {
    let (_dir, storage, run) = setup().await;
    let caps = Limits {
        run: Budget {
            max_cost_micro_usd: Some(MicroUsd(500)),
            ..Budget::default()
        },
        daily_money: None,
        monthly_money: Some(MicroUsd(10_000)),
    };
    let ask = || Reservation {
        purpose: Purpose::Followup,
        ..reservation(run, 400)
    };
    let reserve = |r: Reservation, scope: Option<&'static str>| {
        let storage = &storage;
        async move {
            storage
                .reserve_scoped(&r, &caps, scope)
                .await
                .expect("ledger")
        }
    };
    assert_eq!(reserve(ask(), Some("followup:t:a")).await, Ok(()));
    let spent = reserve(ask(), Some("followup:t:a")).await;
    assert!(matches!(spent, Err(Denied::Money { .. })), "{spent:?}");
    assert_eq!(
        reserve(ask(), Some("followup:t:b")).await,
        Ok(()),
        "another reader's session has its own cap"
    );
    assert_eq!(
        reserve(reservation(run, 400), None).await,
        Ok(()),
        "analysis spends outside every session"
    );
}

#[tokio::test]
async fn a_session_cap_survives_the_month_boundary() {
    let (_dir, storage, run) = setup().await;
    let caps = Limits {
        run: Budget {
            max_cost_micro_usd: Some(MicroUsd(500)),
            ..Budget::default()
        },
        daily_money: None,
        monthly_money: Some(MicroUsd(10_000)),
    };
    let ask = |at: &str| Reservation {
        purpose: Purpose::Followup,
        at: at.parse().expect("timestamp"),
        ..reservation(run, 400)
    };
    let scope = Some("followup:t:a");
    let october = storage
        .reserve_scoped(&ask("2026-10-31T23:59:59Z"), &caps, scope)
        .await
        .expect("ledger");
    assert_eq!(october, Ok(()));
    let november = storage
        .reserve_scoped(&ask("2026-11-01T00:00:01Z"), &caps, scope)
        .await
        .expect("ledger");
    assert!(
        matches!(november, Err(Denied::Money { .. })),
        "{november:?}"
    );
    assert_eq!(
        storage
            .reserve_scoped(&ask("2026-11-01T00:00:02Z"), &caps, Some("followup:t:b"))
            .await
            .expect("ledger"),
        Ok(()),
        "the monthly pool itself starts again"
    );
}
