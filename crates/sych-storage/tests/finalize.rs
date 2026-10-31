//! Fenced finalization and its fault matrix. The authoritative commit predicate:
//! the run is RUNNING with frozen inputs (`stage = ANALYZE`), its job is CLAIMED by the same
//! owner at the same `lease_epoch` with a live lease, `run.generation = change.desired_generation`,
//! and the result bytes are already stored under the run. Result pointer, terminal state, usage,
//! the job's DONE and the outbox intent then commit in one transaction.

use std::fs;

use serde_json::json;
use sych_core::schedule::RunInputs;
use sych_core::{PolicySnapshotId, SourceSnapshotId, sha256};
use sych_storage::{ArtifactKind, StorageError};

mod support;

use support::{at, change, claim, finish, inputs, migrated, result_for};

fn publishing(head: &str) -> RunInputs {
    RunInputs {
        publish: true,
        ..inputs(head)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn commit_and_outbox_intent_are_one_step_and_death_after_it_reruns_nothing() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    s.generate(id, &publishing("h"), 1_800_001_000, 3, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    finish(&s, &c, 2).await.expect("commit");
    let ops = s.operations(id).await.expect("ops");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        (
            ops[0].slot.as_str(),
            ops[0].state.as_str(),
            ops[0].generation
        ),
        ("summary", "PENDING", 1)
    );

    // The process dies right after commit: the lease expires, but the job is DONE.
    assert_eq!(s.reap(at(500)).await.expect("reap"), 0);
    assert!(
        s.claim("w2", 60, at(501)).await.expect("claim").is_none(),
        "no second analysis"
    );
    let jobs = s.jobs_of(&c.run.id.to_string()).await.expect("jobs");
    assert_eq!((jobs[0].0.as_str(), jobs[0].2), ("DONE", 1), "one attempt");
}

#[tokio::test(flavor = "current_thread")]
async fn a_newer_generation_supersedes_the_unsent_revision_and_quiet_runs_queue_nothing() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    s.generate(id, &publishing("h1"), 1_800_001_000, 3, at(0))
        .await
        .expect("gen");
    let first = claim(&s, "w", 1).await;
    finish(&s, &first, 2).await.expect("commit");
    s.generate(id, &publishing("h2"), 1_800_001_000, 3, at(3))
        .await
        .expect("gen");
    let second = claim(&s, "w", 4).await;
    finish(&s, &second, 5).await.expect("commit");
    let states: Vec<(String, i64)> = s
        .operations(id)
        .await
        .expect("ops")
        .into_iter()
        .map(|o| (o.state, o.generation))
        .collect();
    assert_eq!(states, [("SUPERSEDED".into(), 1), ("PENDING".into(), 2)]);

    s.generate(id, &inputs("h3"), 1_800_001_000, 3, at(6))
        .await
        .expect("gen");
    let third = claim(&s, "w", 7).await;
    finish(&s, &third, 8).await.expect("commit");
    assert_eq!(
        s.operations(id).await.expect("ops").len(),
        2,
        "publish off: no intent"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lost_bytes_unfrozen_inputs_and_cancel_before_commit_leave_no_result_or_intent() {
    let (dir, s) = migrated().await;
    let id = change(&s).await;
    let snap = SourceSnapshotId(sha256(b"snap"));
    let policy = PolicySnapshotId(sha256(b"policy"));

    // Bytes failure: the pointer may only reference bytes that are durable.
    s.generate(id, &publishing("h1"), 1_800_001_000, 3, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    s.prepare_run(&c.fence, &c.run, &snap, &policy, &json!({}), None, at(1))
        .await
        .expect("prepare");
    s.freeze(&c.fence, &c.run.id, &json!({}), at(1))
        .await
        .expect("freeze");
    let result = result_for(&c);
    let digest = s
        .put_artifact(
            &c.run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(2),
        )
        .await
        .expect("put");
    fs::remove_dir_all(dir.path().join("artifacts")).expect("lose the bytes");
    let lost = s.complete_run(&c.run, &result, &digest, at(3)).await;
    assert!(lost.is_err(), "{lost:?}");
    let (state, ..) = s
        .run_row("local", &c.run.id.to_string())
        .await
        .expect("row");
    assert_eq!(state, "RUNNING", "never a false COMPLETED");

    // Inputs not frozen yet: a result cannot be authoritative.
    s.generate(id, &publishing("h2"), 1_800_001_000, 3, at(4))
        .await
        .expect("gen");
    let c = claim(&s, "w", 5).await;
    s.prepare_run(&c.fence, &c.run, &snap, &policy, &json!({}), None, at(5))
        .await
        .expect("prepare");
    let result = result_for(&c);
    let digest = s
        .put_artifact(
            &c.run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(6),
        )
        .await
        .expect("put");
    let unfrozen = s.complete_run(&c.run, &result, &digest, at(6)).await;
    assert!(
        matches!(unfrozen, Err(StorageError::Conflict(_))),
        "{unfrozen:?}"
    );

    // Cancelled before commit: the late commit changes nothing.
    s.cancel_change(id, "STOP_COMMAND", at(7))
        .await
        .expect("cancel");
    let late = finish(&s, &c, 8).await;
    assert!(late.is_err());
    assert!(
        s.operations(id).await.expect("ops").is_empty(),
        "no intent from a failed commit"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_database_failure_inside_finalization_rolls_back_the_result_too() {
    let (dir, s) = migrated().await;
    let id = change(&s).await;
    s.generate(id, &publishing("h"), 1_800_001_000, 3, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    // The outbox insert, the last statement of the commit, fails.
    let pool = support::database::inspect(dir.config()).await;
    let fault = match &dir.config().database {
        sych_storage::Database::Sqlite(_) => "ALTER TABLE publication_objects RENAME TO broken",
        sych_storage::Database::Postgres(_) => {
            "ALTER TABLE publication_objects ADD CONSTRAINT finalization_fault CHECK (1 = 0)"
        }
    };
    sqlx::query(fault)
        .execute(&pool)
        .await
        .expect("break outbox independently of the finalization transaction");
    pool.close().await;
    let failed = finish(&s, &c, 2).await;
    assert!(matches!(failed, Err(StorageError::Db(_))), "{failed:?}");
    let (state, ..) = s
        .run_row("local", &c.run.id.to_string())
        .await
        .expect("row");
    assert_eq!(state, "RUNNING", "no pointer without its intent");
    let jobs = s.jobs_of(&c.run.id.to_string()).await.expect("jobs");
    assert_eq!(jobs[0].0, "CLAIMED", "the job is not DONE either");
}
