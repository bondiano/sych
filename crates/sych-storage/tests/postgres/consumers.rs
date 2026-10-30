use serde_json::Value;
use sych_core::config::{BillingKind, Enforcement};
use sych_core::model::Denied;
use sych_storage::{Idempotency, Storage, StorageError};

use super::import_support::PgFixture;
use super::support::at;
use super::{Source, limits, reservation};

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn imported_nulls_large_integers_and_flags_reach_real_storage_consumers() {
    let source = Source::seeded().await;
    let pg = PgFixture::new(&source.config.artifacts).await;
    Storage::import_sqlite(source.path(), &source.target(&pg))
        .await
        .expect("import");
    let s = Storage::open(&source.target(&pg))
        .await
        .expect("consumer pool");
    assert_eq!(
        s.load_result("import-tenant", source.run.id)
            .await
            .expect("saved result"),
        source.result
    );
    assert!(matches!(
        s.load_artifact::<Value>("import-tenant", source.run.id, "pruned")
            .await,
        Err(StorageError::ArtifactUnavailable(_))
    ));
    assert_eq!(
        s.change(1).await.expect("change binding").installation_id,
        Some(9_007_199_254_740_993)
    );
    assert_eq!(
        s.idempotency_begin(
            ("import-tenant", "dev", "/v1/reviews"),
            "request-key",
            "body-digest",
            at(1),
        )
        .await
        .expect("NULL response fields"),
        Idempotency::InProgress
    );
    let metrics = s
        .metrics(at(1))
        .await
        .expect("BIGINT aggregate and NULL usage decoding");
    assert_eq!(metrics.reserved_micro_usd, 9_223_372_036_854_775_000);
    assert_eq!(
        (
            metrics.input_tokens,
            metrics.output_tokens,
            metrics.unknown_obligations
        ),
        (0, 0, 0)
    );
    assert_eq!(metrics.attempts_by_state, [("UNKNOWN".into(), 1)]);
    assert!(
        s.lineage_pending()
            .await
            .expect("imported applied flag")
            .is_empty()
    );
    let mut request = reservation(&source.run, 0);
    request.route.provider = "subscription".into();
    request.route.billing = BillingKind::SubscriptionQuota;
    request.route.enforcement = Enforcement::SubscriptionObservable;
    request.route.capabilities.output_cap_enforced = false;
    request.output_tokens = None;
    request.at = "2026-10-30T00:00:00Z"
        .parse()
        .expect("imported accounting month");
    assert_eq!(
        s.reserve_attempt(&request, &limits(100, Some(100)))
            .await
            .expect("imported billing mismatch flag"),
        Err(Denied::BillingMismatch)
    );
    drop(s);
    pg.close().await;
}
