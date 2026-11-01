//! Snapshot lookup over `/v1`: a reader gets the source and policy snapshots a completed
//! result is bound to; a run without a result has none to bind, a pruned source snapshot reads
//! as `null`, and pinned snapshots the result does not name are refused.

use reqwest::StatusCode;
use serde_json::Value;
use sqlx::AnyPool;

mod fake_github;
mod http;
mod world;

use http::{call, listen};
use world::database::inspect;
use world::{commit, commit_claimed, commit_claimed_under, git, world};

async fn get(url: &str, who: &str, path: &str) -> (StatusCode, Value) {
    call(url, "GET", path, Some(who), None, &Value::Null).await
}

#[tokio::test(flavor = "current_thread")]
async fn a_completed_run_serves_the_snapshots_its_result_names() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    let run = w.run_ids().await[0].clone();
    let url = listen(&w).await;
    let path = format!("/v1/runs/{run}/snapshots");
    let (status, body) = get(&url, "reader", &path).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("NO_RESULT")),
        "a queued run has no result to bind"
    );

    commit_claimed(&w, &head, 1).await;
    let result = w.result(0).await;
    let (status, snapshots) = get(&url, "reader", &path).await;
    assert_eq!(status, StatusCode::OK, "{snapshots}");
    assert_eq!(snapshots["run_id"], run);
    let source = &snapshots["source"];
    assert_eq!(source["id"], result.source_snapshot_id.0.to_string());
    assert_eq!(source["head"], format!("sha1:{head}"), "the reviewed head");
    assert_eq!(source["diff_base"], format!("sha1:{}", w.base));
    assert_eq!(source["comparison"], "MERGE_BASE");
    let policy = &snapshots["policy"];
    assert_eq!(policy["id"], result.policy_snapshot_id.0.to_string());
    assert_eq!(policy["source"]["kind"], "engine_defaults");
    let (status, _) = get(&url, "outsider", &path).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "out of scope");

    // Run 1's pins swapped for those of run 2, at another head under another policy: neither
    // is evidence of run 1's result.
    git(&w.origin, &["checkout", "-q", "pr"]);
    let next = commit(&w.origin, "src/c.rs", "pub fn c() {}\n");
    git(&w.origin, &["checkout", "-q", "main"]);
    w.open_pull(&next, &w.base, false);
    w.pr_event("d-2", "synchronize", &next).await;
    w.schedule(6).await;
    commit_claimed_under(&w, &next, 1, b"other rules").await;
    let later = w.run_ids().await[1].clone();
    let db = inspect(w.dir.config()).await;
    for role in ["source_snapshot", "policy_snapshot"] {
        let original = pin(&db, &run, role, &later).await;
        let (status, refused) = get(&url, "reader", &path).await;
        assert_eq!(
            (status, refused["error"]["code"].as_str()),
            (StatusCode::CONFLICT, Some("SNAPSHOT_MISMATCH")),
            "{role}"
        );
        restore(&db, &run, role, &original).await;
    }

    // The source snapshot's retention is shorter than the result's.
    sqlx::query(
        "DELETE FROM artifacts WHERE sha256 IN
            (SELECT sha256 FROM run_artifacts WHERE role = 'source_snapshot')",
    )
    .execute(&db)
    .await
    .expect("prune");
    let (status, pruned) = get(&url, "reader", &path).await;
    assert_eq!(status, StatusCode::OK, "{pruned}");
    assert!(pruned["source"].is_null(), "{pruned}");
    assert_eq!(pruned["policy"], *policy);
}

/// Points run `run`'s `role` at the artifact run `from` pins under it; returns the old digest.
async fn pin(db: &AnyPool, run: &str, role: &str, from: &str) -> String {
    let original: String =
        sqlx::query_scalar("SELECT sha256 FROM run_artifacts WHERE run_id = $1 AND role = $2")
            .bind(run)
            .bind(role)
            .fetch_one(db)
            .await
            .expect("pin");
    let swapped: String =
        sqlx::query_scalar("SELECT sha256 FROM run_artifacts WHERE run_id = $1 AND role = $2")
            .bind(from)
            .bind(role)
            .fetch_one(db)
            .await
            .expect("other pin");
    assert_ne!(original, swapped, "{role} differs between the runs");
    restore(db, run, role, &swapped).await;
    original
}

async fn restore(db: &AnyPool, run: &str, role: &str, digest: &str) {
    sqlx::query("UPDATE run_artifacts SET sha256 = $3 WHERE run_id = $1 AND role = $2")
        .bind(run)
        .bind(role)
        .bind(digest)
        .execute(db)
        .await
        .expect("repin");
}
