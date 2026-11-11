//! Advisory publication on the GitHub fake: a committed result with findings
//! becomes one summary, one `COMMENT` review of inline comments at explicit
//! commit/path/line/side, and an advisory neutral check run; a refused position moves to the
//! summary; nothing approves or merges.

use serde_json::{Value, json};

mod fake_github;
mod world;

use fake_github::lock;
use world::{World, now, world};

async fn drain(w: &World) {
    while w.engine.publish_once().await.expect("publish") {}
}

fn writes(w: &World, prefix: &str) -> Vec<Value> {
    lock(&w.fake)
        .log
        .iter()
        .filter_map(|l| l.strip_prefix(prefix))
        .map(|json| serde_json::from_str(json.trim()).expect("payload"))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn findings_become_one_comment_review_at_exact_positions_and_one_summary() {
    let w = world(None).await;
    let head = world::commit_result(&w, 2).await;
    drain(&w).await;
    assert!(writes(&w, "POST /repos/o/r/pulls/7/comments ").is_empty());
    let reviews = writes(&w, "POST /repos/o/r/pulls/7/reviews ");
    assert_eq!(reviews.len(), 1, "one review carries every new comment");
    assert_eq!(
        (
            reviews[0]["event"].as_str(),
            reviews[0]["commit_id"].as_str()
        ),
        (Some("COMMENT"), Some(head.as_str())),
        "never approves or requests changes"
    );
    let inline = reviews[0]["comments"].as_array().expect("comments");
    assert_eq!(inline.len(), 2);
    for c in inline {
        assert_eq!(
            (c["path"].as_str(), c["line"].as_u64(), c["side"].as_str()),
            (Some("src/b.rs"), Some(1), Some("RIGHT"))
        );
        assert!(c.get("position").is_none(), "never the deprecated position");
        assert!(
            c["body"]
                .as_str()
                .expect("body")
                .starts_with("<!-- sych:inline:")
        );
    }
    let summary = &lock(&w.fake).comments[&1].1.clone();
    assert!(
        summary.contains(
            "High · Rule violation: Defect 0\n\n`EFFECT-01` · `src/b.rs:1` · ↳ shown inline"
        ),
        "{summary}"
    );
    assert!(!lock(&w.fake).log.join("\n").contains("/merge"), "no merge");
    // Each comment is found in the review by its marker, so its id is stored.
    assert_eq!(inline_states(&w).await, ["DELIVERED", "DELIVERED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_review_sends_each_comment_alone() {
    let w = world(None).await;
    lock(&w.fake).fail_next.insert(
        "POST /repos/o/r/pulls/7/reviews".into(),
        (422, json!({"message": "line must be part of the diff"})),
    );
    world::commit_result(&w, 2).await;
    drain(&w).await;
    assert_eq!(writes(&w, "POST /repos/o/r/pulls/7/comments ").len(), 2);
    assert_eq!(inline_states(&w).await, ["DELIVERED", "DELIVERED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_review_with_an_unknown_outcome_is_reconciled_never_resent() {
    let w = world(None).await;
    lock(&w.fake).fail_next.insert(
        "POST /repos/o/r/pulls/7/reviews".into(),
        (502, json!({"message": "Bad Gateway"})),
    );
    world::commit_result(&w, 2).await;
    drain(&w).await;
    assert!(writes(&w, "POST /repos/o/r/pulls/7/comments ").is_empty());
    assert_eq!(inline_states(&w).await, ["UNCERTAIN", "UNCERTAIN"]);
}

async fn inline_states(w: &World) -> Vec<String> {
    let ops = w.app.storage.operations(1).await.expect("ops");
    ops.into_iter()
        .filter(|o| o.slot.starts_with("inline"))
        .map(|o| o.state)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_position_moves_the_finding_into_the_refreshed_summary() {
    let w = world(None).await;
    lock(&w.fake).reject_inline_line = Some(1);
    world::commit_result(&w, 1).await;
    drain(&w).await;
    let summary = lock(&w.fake).comments[&1].1.clone();
    assert!(
        summary.contains("not inline: the provider rejected the inline position"),
        "{summary}"
    );
    let ops = w.app.storage.operations(1).await.expect("ops");
    let states: Vec<_> = ops
        .iter()
        .map(|o| (o.slot.starts_with("inline"), o.state.as_str()))
        .collect();
    assert_eq!(states, [(false, "DELIVERED"), (true, "FAILED")]);
}

#[tokio::test(flavor = "current_thread")]
async fn checks_when_enabled_are_neutral_and_pinned_to_the_head() {
    let w = world(None).await;
    let head = committed_with_checks(&w).await;
    drain(&w).await;
    let runs = lock(&w.fake).check_runs.clone();
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (runs[0]["head_sha"].as_str(), runs[0]["conclusion"].as_str()),
        (Some(head.as_str()), Some("neutral"))
    );
}

/// Like [`committed`], with `checks` in the run inputs as a checks-enabled connection sets them.
async fn committed_with_checks(w: &World) -> String {
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let s = &w.app.storage;
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    let latest = s.latest(1).await.expect("latest").expect("run");
    let mut inputs = latest.inputs.clone();
    inputs.checks = true;
    inputs.publish = true;
    s.generate(1, &inputs, now().as_second() + 600, 3, now())
        .await
        .expect("gen");
    assert!(w.engine.work_once().await.expect("work"));
    head
}
