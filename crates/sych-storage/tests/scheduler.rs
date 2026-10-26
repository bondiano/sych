//! Generations, jobs and fences on a real `SQLite` file (roadmap M2.2/M2.4): supersede revokes the
//! old lease, every claim gets a new epoch, the reaper returns expired leases, and a late worker
//! cannot commit a result.

use serde_json::json;
use sych_core::{PolicySnapshotId, SourceSnapshotId, sha256};
use sych_storage::StorageError;

mod support;

use support::{T0, at, change, claim, finish, inputs, migrated};

#[tokio::test(flavor = "current_thread")]
async fn a_push_supersedes_the_running_generation_and_its_late_commit_is_refused() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    let first = s
        .generate(id, &inputs("h1"), T0 + 300, 2, at(0))
        .await
        .expect("gen");
    assert_eq!(first.generation, 1);
    let old = claim(&s, "w1", 1).await;
    assert_eq!(old.fence.epoch, 1);

    let second = s
        .generate(id, &inputs("h2"), T0 + 300, 2, at(2))
        .await
        .expect("gen");
    assert_eq!(second.generation, 2);
    assert!(
        !s.heartbeat(&old.fence, 60, at(3)).await.expect("hb"),
        "lease revoked"
    );
    let late = finish(&s, &old, 4).await;
    assert!(matches!(late, Err(StorageError::Conflict(_))), "{late:?}");
    let (state, ..) = s
        .run_row("local", &old.run.id.to_string())
        .await
        .expect("row");
    assert_eq!(state, "SUPERSEDED");

    let current = claim(&s, "w1", 5).await;
    assert_eq!(current.run.id, second.id);
    finish(&s, &current, 6)
        .await
        .expect("current generation commits");
    let (state, ..) = s
        .run_row("local", &second.id.to_string())
        .await
        .expect("row");
    assert_eq!(state, "COMPLETED");
    let events: Vec<String> = s
        .change_events(id)
        .await
        .expect("events")
        .into_iter()
        .map(|(kind, ..)| kind)
        .collect();
    assert_eq!(events, ["GENERATED", "SUPERSEDED", "GENERATED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn an_expired_lease_is_reaped_with_a_new_epoch_and_the_old_owner_is_fenced() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    s.generate(id, &inputs("h"), T0 + 1_000, 2, at(0))
        .await
        .expect("gen");
    let crashed = claim(&s, "w1", 1).await;
    assert!(
        s.claim("w2", 60, at(2)).await.expect("claim").is_none(),
        "leased"
    );
    assert_eq!(s.reap(at(30)).await.expect("reap"), 0, "lease still alive");
    assert_eq!(s.reap(at(62)).await.expect("reap"), 1);
    let restarted = claim(&s, "w2", 63).await;
    assert!(restarted.fence.epoch > crashed.fence.epoch);
    assert_eq!(restarted.attempts, 2);
    let late = finish(&s, &crashed, 64).await;
    assert!(late.is_err(), "the crashed worker's fence is stale");
    finish(&s, &restarted, 65).await.expect("new owner commits");
}

#[tokio::test(flavor = "current_thread")]
async fn reaping_past_the_attempt_limit_fails_the_run() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    let run = s
        .generate(id, &inputs("h"), T0 + 10_000, 1, at(0))
        .await
        .expect("gen");
    claim(&s, "w", 0).await;
    s.reap(at(61)).await.expect("reap");
    claim(&s, "w", 62).await;
    s.reap(at(123)).await.expect("reap");
    let (state, ..) = s.run_row("local", &run.id.to_string()).await.expect("row");
    assert_eq!(state, "FAILED");
    assert!(s.claim("w", 60, at(124)).await.expect("claim").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_stops_active_runs_but_keeps_a_committed_result() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    let done = s
        .generate(id, &inputs("h1"), T0 + 300, 2, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    finish(&s, &c, 2).await.expect("commit");
    let active = s
        .generate(id, &inputs("h2"), T0 + 300, 2, at(3))
        .await
        .expect("gen");
    let cancelled = s.cancel_change(id, "CLOSED", at(4)).await.expect("cancel");
    assert_eq!(cancelled, [active.id.to_string()]);
    let (state, ..) = s.run_row("local", &done.id.to_string()).await.expect("row");
    assert_eq!(state, "COMPLETED", "a committed result is never rewritten");
    assert!(s.claim("w", 60, at(5)).await.expect("claim").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_waiting_run_is_released_until_its_deadline_and_woken_early() {
    let (_dir, s) = migrated().await;
    let id = change(&s).await;
    let run = s
        .generate(id, &inputs("h"), T0 + 1_000, 2, at(0))
        .await
        .expect("gen");
    let c = claim(&s, "w", 1).await;
    let snap = SourceSnapshotId(sha256(b"snap"));
    let policy = PolicySnapshotId(sha256(b"policy"));
    let waiting = s
        .prepare_run(
            &c.fence,
            &c.run,
            &snap,
            &policy,
            &json!({"expected": ["tests"]}),
            Some(T0 + 181),
            at(1),
        )
        .await
        .expect("prepare");
    assert!(waiting);
    assert!(
        s.release(&c.fence, T0 + 181, None, at(1))
            .await
            .expect("release")
    );
    assert!(
        s.claim("w", 60, at(2)).await.expect("claim").is_none(),
        "waits"
    );
    let latest = s.latest(id).await.expect("latest").expect("run");
    assert!(
        latest.active && !latest.frozen,
        "inputs pinned, evidence not frozen yet"
    );
    assert!(s.wake(&run.id.to_string(), at(3)).await.expect("wake"));
    let again = claim(&s, "w", 3).await;
    assert_eq!(again.evidence_deadline, Some(T0 + 181));
}
