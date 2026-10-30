use std::time::Duration;

use serde_json::{Value, json};
use sqlx::AnyPool;
use sych_core::model::Denied;
use sych_core::{MicroUsd, PolicySnapshotId, SourceSnapshotId, sha256};
use sych_storage::{ArtifactKind, Partition, Storage, StorageError};
use tokio::task::yield_now;
use tokio::time::timeout;

use super::import_support::PgFixture;
use super::support::{at, change, claim, finish, inputs, result_for};
use super::{T0, inspect, limits, reservation};

async fn install_write_gate(pool: &AnyPool, trigger: &'static str) {
    sqlx::raw_sql(
        "CREATE FUNCTION hold_test_write() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(1937335145);
             RETURN NEW;
         END;
         $$;",
    )
    .execute(pool)
    .await
    .expect("install test transaction gate");
    sqlx::raw_sql(trigger)
        .execute(pool)
        .await
        .expect("install gated write trigger");
}

async fn hold_write_gate(pool: &AnyPool) -> sqlx::Transaction<'_, sqlx::Any> {
    let mut tx = pool.begin().await.expect("test gate transaction");
    sqlx::query("SELECT pg_advisory_xact_lock(1937335145)")
        .execute(&mut *tx)
        .await
        .expect("hold test gate");
    tx
}

async fn wait_for_blocked_writers(pool: &AnyPool, expected: i64) {
    timeout(Duration::from_secs(10), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname = current_database() AND state = 'active'
                     AND wait_event_type = 'Lock' AND pid <> pg_backend_pid()",
            )
            .fetch_one(pool)
            .await
            .expect("observe independent database writers");
            if blocked == expected {
                break;
            }

            yield_now().await;
        }
    })
    .await
    .expect("writers must reach the database gate, not a timing sleep");
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn independent_pools_cannot_overspend_an_empty_tenant_ledger_or_null_run_scope() {
    let dir = tempfile::tempdir().expect("artifacts");
    let pg = PgFixture::new(dir.path()).await;
    let first = Storage::open(&pg.config).await.expect("first pool");
    let second = Storage::open(&pg.config).await.expect("independent pool");
    let partition = Partition {
        tenant_id: "local".into(),
        access_scope: "local:repo".into(),
    };
    let snapshot = SourceSnapshotId(sha256(b"snapshot"));
    let policy = PolicySnapshotId(sha256(b"policy"));
    let first_run = first
        .begin_run(&partition, "first", &snapshot, &policy, at(0))
        .await
        .expect("first run");
    let second_run = second
        .begin_run(&partition, "second", &snapshot, &policy, at(0))
        .await
        .expect("second run");
    install_write_gate(
        &pg.pool,
        "CREATE TRIGGER ledger_gate BEFORE INSERT ON model_attempts
         FOR EACH ROW EXECUTE FUNCTION hold_test_write()",
    )
    .await;
    let gate_pool = inspect(&pg.config).await;
    let gate = hold_write_gate(&gate_pool).await;
    let first_request = reservation(&first_run, 100);
    let first_writer = tokio::spawn(async move {
        first
            .reserve_attempt(&first_request, &limits(100, Some(100)))
            .await
            .expect("first ledger")
    });
    wait_for_blocked_writers(&pg.pool, 1).await;
    let second_request = reservation(&second_run, 100);
    let second_writer = tokio::spawn(async move {
        second
            .reserve_attempt(&second_request, &limits(100, Some(100)))
            .await
            .expect("second ledger")
    });
    // Without the storage lock both read the empty ledger before their INSERT gate.
    wait_for_blocked_writers(&pg.pool, 2).await;
    gate.commit().await.expect("release writers");
    let first_result = first_writer.await.expect("first writer");
    let second_result = second_writer.await.expect("second writer");
    assert_eq!(
        usize::from(first_result.is_ok()) + usize::from(second_result.is_ok()),
        1
    );
    let (winner, denied) = if first_result.is_ok() {
        (&first_run, second_result)
    } else {
        (&second_run, first_result)
    };
    assert!(
        matches!(denied, Err(Denied::Money { ref account, committed: MicroUsd(100), .. })
        if account == "month"),
        "{denied:?}"
    );
    let reader = Storage::open(&pg.config).await.expect("consumer pool");
    let denied = reader
        .reserve_attempt(&reservation(winner, 1), &limits(100, None))
        .await
        .expect("NULL-scope run cap");
    assert!(
        matches!(denied, Err(Denied::Money { ref account, committed: MicroUsd(100), .. })
        if account == "run"),
        "{denied:?}"
    );
    let metrics = reader.metrics(at(2)).await.expect("ledger metrics");
    assert_eq!(metrics.reserved_micro_usd, 100);
    assert_eq!(metrics.attempts_by_state, [("RESERVED".into(), 1)]);
    drop(reader);
    gate_pool.close().await;
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn independent_pools_competing_for_one_job_have_exactly_one_winner() {
    let dir = tempfile::tempdir().expect("artifacts");
    let pg = PgFixture::new(dir.path()).await;
    let first = Storage::open(&pg.config).await.expect("first pool");
    let second = Storage::open(&pg.config).await.expect("independent pool");
    let id = change(&first).await;
    let run = first
        .generate(id, &inputs("head"), T0 + 1_000, 3, at(0))
        .await
        .expect("one queued job");
    install_write_gate(
        &pg.pool,
        "CREATE TRIGGER claim_gate BEFORE UPDATE ON jobs
         FOR EACH ROW WHEN (NEW.state = 'CLAIMED') EXECUTE FUNCTION hold_test_write()",
    )
    .await;
    let gate_pool = inspect(&pg.config).await;
    let gate = hold_write_gate(&gate_pool).await;
    let first_writer =
        tokio::spawn(async move { first.claim("first", 60, at(1)).await.expect("first claim") });
    wait_for_blocked_writers(&pg.pool, 1).await;
    let second_writer = tokio::spawn(async move {
        second
            .claim("second", 60, at(1))
            .await
            .expect("second claim")
    });
    // A missing write lock leaves the second UPDATE waiting on the first row version.
    wait_for_blocked_writers(&pg.pool, 2).await;
    gate.commit().await.expect("release claim race");
    let claims = [
        first_writer.await.expect("first writer"),
        second_writer.await.expect("second writer"),
    ];
    let winners: Vec<_> = claims.into_iter().flatten().collect();
    assert_eq!(winners.len(), 1, "one job is never handed to two workers");
    assert_eq!(winners[0].run.id, run.id);
    assert_eq!((winners[0].fence.epoch, winners[0].attempts), (1, 1));
    let reader = Storage::open(&pg.config).await.expect("consumer pool");
    let jobs = reader.jobs_of(&run.id.to_string()).await.expect("jobs");
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        (jobs[0].0.as_str(), jobs[0].1, jobs[0].2),
        ("CLAIMED", 1, 1)
    );
    drop(reader);
    gate_pool.close().await;
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn recovered_same_owner_rejects_old_epoch_pins_and_finalization() {
    let dir = tempfile::tempdir().expect("artifacts");
    let pg = PgFixture::new(dir.path()).await;
    let old_pool = Storage::open(&pg.config).await.expect("old process pool");
    let recovery = Storage::open(&pg.config)
        .await
        .expect("restarted process pool");
    let id = change(&old_pool).await;
    old_pool
        .generate(id, &inputs("head"), T0 + 1_000, 3, at(0))
        .await
        .expect("generate");
    let old = claim(&old_pool, "worker", 1).await;
    assert!(
        old_pool
            .prepare_run(
                &old.fence,
                &old.run,
                &SourceSnapshotId(sha256(b"snap")),
                &PolicySnapshotId(sha256(b"policy")),
                &json!({}),
                None,
                at(2),
            )
            .await
            .expect("prepare before crash")
    );
    assert!(
        old_pool
            .freeze(&old.fence, &old.run.id, &json!({}), at(2))
            .await
            .expect("freeze before crash")
    );
    let result = result_for(&old);
    let digest = old_pool
        .put_artifact(
            &old.run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(2),
        )
        .await
        .expect("result bytes survive process death");
    assert_eq!(recovery.reap(at(62)).await.expect("recover expired job"), 1);
    let current = claim(&recovery, "worker", 63).await;
    assert_eq!(current.run.id, old.run.id);
    assert_eq!(current.fence.owner, old.fence.owner);
    assert!(current.fence.epoch > old.fence.epoch);
    let stale_pin = old_pool
        .put_artifact(
            &old.run,
            "stale",
            ArtifactKind::Facts,
            &json!({"stale": true}),
            at(64),
        )
        .await;
    assert!(
        matches!(stale_pin, Err(StorageError::Conflict(_))),
        "{stale_pin:?}"
    );
    assert!(
        recovery
            .load_artifact::<Value>("local", current.run.id, "stale")
            .await
            .is_err()
    );
    let stale_commit = old_pool
        .complete_run(&old.run, &result, &digest, at(64))
        .await;
    assert!(
        matches!(stale_commit, Err(StorageError::Conflict(_))),
        "{stale_commit:?}"
    );
    assert!(matches!(
        recovery.load_result("local", current.run.id).await,
        Err(StorageError::NotCompleted { .. })
    ));
    let jobs = recovery
        .jobs_of(&current.run.id.to_string())
        .await
        .expect("recovered job");
    assert_eq!(
        (jobs[0].0.as_str(), jobs[0].1, jobs[0].2),
        ("CLAIMED", current.fence.epoch, 2)
    );
    finish(&recovery, &current, 65)
        .await
        .expect("only current epoch commits");
    assert_eq!(
        recovery
            .load_result("local", current.run.id)
            .await
            .expect("saved result"),
        result
    );
    drop((old_pool, recovery));
    pg.close().await;
}

#[tokio::test]
#[ignore = "requires disposable local PostgreSQL 18.6 via SYCH_TEST_POSTGRES_URL; run explicitly"]
async fn gc_rechecks_a_pin_committed_after_its_candidate_listing_from_an_independent_pool() {
    let dir = tempfile::tempdir().expect("artifacts");
    let pg = PgFixture::new(dir.path()).await;
    let writer = Storage::open(&pg.config).await.expect("writer pool");
    let gc = Storage::open(&pg.config)
        .await
        .expect("independent GC pool");
    let partition = Partition {
        tenant_id: "local".into(),
        access_scope: "local:repo".into(),
    };
    let snapshot = SourceSnapshotId(sha256(b"snapshot"));
    let policy = PolicySnapshotId(sha256(b"policy"));
    let failed = writer
        .begin_run(&partition, "failed", &snapshot, &policy, at(0))
        .await
        .expect("old run");
    let document = json!({"schema_version": 1, "shared": true});
    let digest = writer
        .put_artifact(&failed, "facts", ArtifactKind::Facts, &document, at(0))
        .await
        .expect("orphan bytes");
    writer
        .fail_run(&failed, "failed", at(1))
        .await
        .expect("orphan the artifact");
    let listed = gc.prune(at(7_200), true).await.expect("eligible candidate");
    assert_eq!(listed.candidates.len(), 1);
    assert_eq!(listed.candidates[0].sha256, digest);
    let active = writer
        .begin_run(&partition, "active", &snapshot, &policy, at(7_200))
        .await
        .expect("active run");
    install_write_gate(
        &pg.pool,
        "CREATE TRIGGER pin_gate AFTER INSERT ON run_artifacts
         FOR EACH ROW EXECUTE FUNCTION hold_test_write()",
    )
    .await;
    let gate_pool = inspect(&pg.config).await;
    let gate = hold_write_gate(&gate_pool).await;
    let active_id = active.id;
    let pin = tokio::spawn(async move {
        writer
            .put_artifact(&active, "facts", ArtifactKind::Facts, &document, at(7_200))
            .await
            .expect("new pin")
    });
    wait_for_blocked_writers(&pg.pool, 1).await;
    let pruning = tokio::spawn(async move { gc.prune(at(7_200), false).await.expect("GC race") });
    // GC lists the still-uncommitted pin as orphan, then waits to re-check under the lock.
    wait_for_blocked_writers(&pg.pool, 2).await;
    gate.commit()
        .await
        .expect("commit pin after candidate listing");
    assert_eq!(pin.await.expect("pin writer"), digest);
    let report = pruning.await.expect("GC writer");
    assert_eq!(
        report.deleted, 0,
        "a newly pinned artifact must not be unlinked"
    );
    assert!(report.candidates.is_empty());
    let reader = Storage::open(&pg.config).await.expect("consumer pool");
    assert_eq!(
        reader
            .load_artifact::<Value>("local", active_id, "facts")
            .await
            .expect("pinned bytes remain readable"),
        json!({"schema_version": 1, "shared": true})
    );
    assert!(
        reader
            .prune(at(7_200), true)
            .await
            .expect("relist")
            .candidates
            .is_empty()
    );
    drop(reader);
    gate_pool.close().await;
    pg.close().await;
}
