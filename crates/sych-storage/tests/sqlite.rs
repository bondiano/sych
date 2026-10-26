//! Storage contract on a real `SQLite` file and artifact directory (roadmap M0.5).
// shortcut: written against the concrete SQLite API; extract a trait when PostgreSQL lands (M4).

use std::fs;
use std::path::{Path, PathBuf};

use sych_core::{
    PolicySnapshotId, ReviewResult, RunId, SourceSnapshotId, Timestamp, decode, sha256,
};
use sych_storage::{
    ArtifactKind, Partition, PruneReason, RunHandle, Storage, StorageError, StoreConfig,
    TENANT_LOCAL,
};
use tempfile::TempDir;

const T0: i64 = 1_800_000_000;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn at(offset: i64) -> Timestamp {
    Timestamp::from_second(T0 + offset).expect("timestamp")
}

fn config(dir: &Path) -> StoreConfig {
    StoreConfig {
        database: dir.join("db/sych.db"),
        artifacts: dir.join("artifacts"),
    }
}

fn partition() -> Partition {
    Partition {
        tenant_id: TENANT_LOCAL.into(),
        access_scope: "local:repo".into(),
    }
}

async fn migrated() -> (TempDir, Storage) {
    let dir = tempfile::tempdir().expect("tempdir");
    Storage::migrate(&config(dir.path()))
        .await
        .expect("migrate");
    let storage = Storage::open(&config(dir.path())).await.expect("open");
    (dir, storage)
}

fn result_for(run: &RunHandle) -> ReviewResult {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json"),
    )
    .expect("fixture");
    let mut result: ReviewResult = decode(&text).expect("fixture decodes");
    result.run_id = run.id;
    result.review_revision = run.generation;
    result
}

async fn begin(storage: &Storage, change: &str) -> RunHandle {
    storage
        .begin_run(
            &partition(),
            change,
            &SourceSnapshotId(sha256(b"snapshot")),
            &PolicySnapshotId(sha256(b"policy")),
            at(0),
        )
        .await
        .expect("begin")
}

async fn completed(storage: &Storage) -> (RunHandle, ReviewResult) {
    let run = begin(storage, "main...feature").await;
    let result = result_for(&run);
    storage
        .put_artifact(
            &run,
            "source_snapshot",
            ArtifactKind::SourceSnapshot,
            &"snapshot",
            at(0),
        )
        .await
        .expect("put snapshot");
    let digest = storage
        .put_artifact(
            &run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(0),
        )
        .await
        .expect("put result");
    storage
        .complete_run(&run, &result, &digest, at(1))
        .await
        .expect("complete");
    (run, result)
}

#[tokio::test]
async fn schema_changes_only_through_migrate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config(dir.path());
    assert!(matches!(
        Storage::open(&cfg).await,
        Err(StorageError::NotMigrated(_))
    ));
    assert!(!cfg.database.exists(), "open never creates a database");
    let files = fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/sqlite"))
        .expect("migrations")
        .count();
    let all: Vec<i64> = (1..=i64::try_from(files).expect("few")).collect();
    assert_eq!(Storage::migrate(&cfg).await.expect("migrate"), all);
    assert!(Storage::migrate(&cfg).await.expect("again").is_empty());
    Storage::open(&cfg).await.expect("open after migrate");
}

#[tokio::test]
async fn saved_result_survives_reopen_and_generations_increase() {
    let (dir, storage) = migrated().await;
    let (run, result) = completed(&storage).await;
    assert_eq!(run.generation, 1);
    assert_eq!(begin(&storage, "main...feature").await.generation, 2);
    drop(storage);

    let reopened = Storage::open(&config(dir.path())).await.expect("reopen");
    assert_eq!(
        reopened
            .load_result(TENANT_LOCAL, run.id)
            .await
            .expect("load"),
        result
    );
    assert_eq!(
        reopened
            .run_state(TENANT_LOCAL, run.id)
            .await
            .expect("state"),
        "COMPLETED"
    );
    assert!(matches!(
        reopened.load_result("other-tenant", run.id).await,
        Err(StorageError::UnknownRun(_))
    ));
}

#[tokio::test]
async fn pointer_commit_fails_without_durable_result_bytes() {
    let (dir, storage) = migrated().await;
    let run = begin(&storage, "c").await;
    let result = result_for(&run);
    let missing = sha256(b"never stored");
    assert!(matches!(
        storage.complete_run(&run, &result, &missing, at(1)).await,
        Err(StorageError::Conflict(_))
    ));
    let digest = storage
        .put_artifact(
            &run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(0),
        )
        .await
        .expect("put");
    // Bytes lost after the row was written: the pointer must not be committed.
    for entry in walk(&dir.path().join("artifacts")) {
        fs::remove_file(entry).expect("remove");
    }
    assert!(matches!(
        storage.complete_run(&run, &result, &digest, at(1)).await,
        Err(StorageError::ArtifactUnavailable(_))
    ));
    assert_eq!(
        storage
            .run_state(TENANT_LOCAL, run.id)
            .await
            .expect("state"),
        "RUNNING"
    );
    storage
        .fail_run(&run, "result not durable", at(2))
        .await
        .expect("fail");
    assert!(matches!(
        storage.load_result(TENANT_LOCAL, run.id).await,
        Err(StorageError::NotCompleted { state, .. }) if state == "FAILED"
    ));
}

#[tokio::test]
async fn result_of_another_run_is_rejected() {
    let (_dir, storage) = migrated().await;
    let run = begin(&storage, "c").await;
    let mut result = result_for(&run);
    result.run_id = RunId::new_v7();
    let digest = storage
        .put_artifact(
            &run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(0),
        )
        .await
        .expect("put");
    assert!(matches!(
        storage.complete_run(&run, &result, &digest, at(1)).await,
        Err(StorageError::Conflict(_))
    ));
}

#[tokio::test]
async fn gc_keeps_pins_and_retention_and_removes_orphans_after_grace() {
    let (_dir, storage) = migrated().await;
    let (done, _) = completed(&storage).await;

    let active = begin(&storage, "active").await;
    storage
        .put_artifact(
            &active,
            "scratch",
            ArtifactKind::EvidenceManifest,
            &"active",
            at(0),
        )
        .await
        .expect("put");
    let failed = begin(&storage, "failed").await;
    storage
        .put_artifact(
            &failed,
            "scratch",
            ArtifactKind::EvidenceManifest,
            &"failed",
            at(0),
        )
        .await
        .expect("put");
    storage
        .fail_run(&failed, "boom", at(1))
        .await
        .expect("fail");

    let within_grace = storage.prune(at(HOUR - 1), false).await.expect("prune");
    assert!(within_grace.candidates.is_empty(), "{within_grace:?}");

    let dry = storage.prune(at(2 * HOUR), true).await.expect("dry run");
    assert_eq!(dry.deleted, 0);
    assert_eq!(dry.candidates.len(), 1);
    assert_eq!(dry.candidates[0].reason, PruneReason::Orphan);
    let again = storage.prune(at(2 * HOUR), false).await.expect("prune");
    assert_eq!((again.deleted, again.candidates.len()), (1, 1));

    // Source expires after 7 days, the result after 30; the active pin never does.
    let week = storage.prune(at(8 * DAY), false).await.expect("prune");
    let kinds: Vec<_> = week
        .candidates
        .iter()
        .map(|c| (c.kind.as_str(), c.reason))
        .collect();
    assert_eq!(kinds, [("source_snapshot", PruneReason::RetentionExpired)]);
    storage
        .load_result(TENANT_LOCAL, done.id)
        .await
        .expect("result still there");

    let month = storage.prune(at(31 * DAY), false).await.expect("prune");
    assert_eq!(month.candidates.len(), 1);
    assert!(matches!(
        storage.load_result(TENANT_LOCAL, done.id).await,
        Err(StorageError::ArtifactUnavailable(_))
    ));
    assert!(
        storage
            .prune(at(365 * DAY), false)
            .await
            .expect("prune")
            .candidates
            .is_empty()
    );
}

#[tokio::test]
async fn tampered_artifact_fails_its_hash_check() {
    let (dir, storage) = migrated().await;
    let (run, _) = completed(&storage).await;
    for entry in walk(&dir.path().join("artifacts")) {
        fs::write(entry, b"{}").expect("tamper");
    }
    assert!(matches!(
        storage.load_result(TENANT_LOCAL, run.id).await,
        Err(StorageError::Corrupt(_))
    ));
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[tokio::test]
async fn artifacts_load_by_role_and_cancelled_run_has_no_result() {
    let (_dir, storage) = migrated().await;
    let (run, result) = completed(&storage).await;
    let loaded: ReviewResult = storage
        .load_artifact(TENANT_LOCAL, run.id, "review_result")
        .await
        .expect("load by role");
    assert_eq!(loaded, result);
    assert!(matches!(
        storage
            .load_artifact::<ReviewResult>("other-tenant", run.id, "review_result")
            .await,
        Err(StorageError::Corrupt(_))
    ));

    let cancelled = begin(&storage, "main...cancelled").await;
    storage.cancel_run(&cancelled, at(2)).await.expect("cancel");
    assert_eq!(
        storage
            .run_state(TENANT_LOCAL, cancelled.id)
            .await
            .expect("state"),
        "CANCELLED"
    );
    assert!(matches!(
        storage.load_result(TENANT_LOCAL, cancelled.id).await,
        Err(StorageError::NotCompleted { .. })
    ));
}

#[tokio::test]
async fn documents_load_by_role_prefix_only_in_their_partition() {
    let (_dir, storage) = migrated().await;
    let run = begin(&storage, "main...checkpoints").await;
    for (role, value) in [
        ("checkpoint/a", "first"),
        ("checkpoint/b", "second"),
        ("facts", "other"),
    ] {
        let document = serde_json::json!({ "schema_version": 1, "value": value });
        storage
            .put_artifact(&run, role, ArtifactKind::Checkpoint, &document, at(0))
            .await
            .expect("put");
    }
    let found: Vec<serde_json::Value> = storage
        .load_artifacts(&run.partition, run.id, "checkpoint/")
        .await
        .expect("load");
    let mut found: Vec<&str> = found.iter().filter_map(|d| d["value"].as_str()).collect();
    found.sort_unstable();
    assert_eq!(found, ["first", "second"]);

    let elsewhere = Partition {
        access_scope: "local:other-repo".into(),
        ..partition()
    };
    let none: Vec<serde_json::Value> = storage
        .load_artifacts(&elsewhere, run.id, "checkpoint/")
        .await
        .expect("load");
    assert!(none.is_empty(), "another access scope sees nothing");
}
