//! Shared setup of the storage contract tests: a migrated store, a bound change, claims and a
//! fenced finish.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

use serde_json::json;
use sych_core::schedule::{RunInputs, Trigger};
use sych_core::scm::{ChangeSignal, RepositoryRef, WebhookEvent};
use sych_core::{
    PolicySnapshotId, ReviewResult, SourceSnapshotId, Timestamp, canonical_json, decode, sha256,
};
use sych_storage::{Admitted, ArtifactKind, Claim, Connection, Storage, StorageError, StoreConfig};
use tempfile::TempDir;

pub const T0: i64 = 1_800_000_000;

pub fn at(offset: i64) -> Timestamp {
    Timestamp::from_second(T0 + offset).expect("timestamp")
}

pub async fn migrated() -> (TempDir, Storage) {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = StoreConfig {
        database: dir.path().join("sych.db"),
        artifacts: dir.path().join("artifacts"),
    };
    Storage::migrate(&config).await.expect("migrate");
    let storage = Storage::open(&config).await.expect("open");
    (dir, storage)
}

/// A bound change created through admission, as the server creates it.
pub async fn change(s: &Storage) -> i64 {
    let event = WebhookEvent {
        event: "pull_request".into(),
        action: Some("opened".into()),
        repository: Some(RepositoryRef {
            external_id: "42".into(),
            full_name: "o/r".into(),
        }),
        installation_id: Some(9),
        change_numbers: vec![7],
        fork: false,
        sender: Some("dev".into()),
        signal: Some(ChangeSignal::Opened),
    };
    let conn = Connection {
        tenant_id: "local".into(),
        connection_id: "gh".into(),
        provider_instance: "github.com".into(),
    };
    let admitted = s
        .admit(&conn, "d-1", &sha256(b"b"), &event, true, at(0))
        .await
        .expect("admit");
    assert!(matches!(admitted, Admitted::Stored { .. }));
    s.open_intents().await.expect("intents")[0].change_id
}

pub fn inputs(head: &str) -> RunInputs {
    RunInputs {
        head_sha: head.into(),
        base_sha: "base".into(),
        base_ref: "main".into(),
        fork: false,
        trigger: Trigger::Changed,
        publish: false,
        checks: false,
        targeted: Vec::new(),
    }
}

pub async fn claim(s: &Storage, owner: &str, now: i64) -> Claim {
    s.claim(owner, 60, at(now))
        .await
        .expect("claim")
        .expect("a ready job")
}

pub fn result_for(claim: &Claim) -> ReviewResult {
    let text = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core/v1/review-result.json"),
    )
    .expect("fixture");
    let mut result: ReviewResult = decode(&text).expect("decode");
    result.run_id = claim.run.id;
    result
}

pub async fn finish(s: &Storage, c: &Claim, now: i64) -> Result<(), StorageError> {
    let snap = SourceSnapshotId(sha256(b"snap"));
    let policy = PolicySnapshotId(sha256(b"policy"));
    // A stale fence pins nothing here; the writes below report it.
    s.prepare_run(&c.fence, &c.run, &snap, &policy, &json!({}), None, at(now))
        .await?;
    s.freeze(&c.fence, &c.run.id, &json!({}), at(now)).await?;
    let result = result_for(c);
    let digest = s
        .put_artifact(
            &c.run,
            "review_result",
            ArtifactKind::ReviewResult,
            &result,
            at(now),
        )
        .await?;
    assert_eq!(digest, sha256(&canonical_json(&result).expect("json")));
    s.complete_run(&c.run, &result, &digest, at(now)).await
}
