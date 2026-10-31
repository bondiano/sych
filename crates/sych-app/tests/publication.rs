//! Advisory publication on the GitHub fake: a committed result with findings
//! becomes one summary, inline comments at explicit commit/path/line/side, and an advisory
//! neutral check run; a refused position moves to the summary; nothing approves or merges.

use serde_json::Value;

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
async fn findings_become_inline_comments_at_exact_positions_and_one_summary() {
    let w = world(None).await;
    let head = world::commit_result(&w, 2).await;
    drain(&w).await;
    let inline = writes(&w, "POST /repos/o/r/pulls/7/comments ");
    assert_eq!(inline.len(), 2);
    for c in &inline {
        assert_eq!(
            (
                c["commit_id"].as_str(),
                c["path"].as_str(),
                c["line"].as_u64(),
                c["side"].as_str()
            ),
            (
                Some(head.as_str()),
                Some("src/b.rs"),
                Some(1),
                Some("RIGHT")
            )
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
    let log = lock(&w.fake).log.join("\n");
    assert!(
        !log.contains("/reviews") && !log.contains("/merge"),
        "no approve, request-changes or merge"
    );
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
