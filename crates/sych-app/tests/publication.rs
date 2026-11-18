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
    assert!(
        writes(&w, "POST /repos/o/r/pulls/7/comments ").is_empty(),
        "{:?}",
        writes(&w, "POST /repos/o/r/pulls/7/comments ")
    );
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
    let head = world::commit_result(&w, 2).await;
    drain(&w).await;
    let alone = writes(&w, "POST /repos/o/r/pulls/7/comments ");
    assert_eq!(alone.len(), 2);
    for c in &alone {
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
    assert!(
        writes(&w, "POST /repos/o/r/pulls/7/comments ").is_empty(),
        "{:?}",
        writes(&w, "POST /repos/o/r/pulls/7/comments ")
    );
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

/// `review.block_on` named a rule with an open finding: the check run fails instead of staying
/// neutral, and the summary says it blocks.
#[tokio::test(flavor = "current_thread")]
async fn a_result_blocked_by_block_on_fails_the_check() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    let s = &w.app.storage;
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    let mut inputs = s.latest(1).await.expect("latest").expect("run").inputs;
    (inputs.checks, inputs.publish) = (true, true);
    s.generate(1, &inputs, now().as_second() + 600, 3, now())
        .await
        .expect("gen");
    world::commit_claimed_with(&w, &head, 1, b"policy", |r| {
        r.blocked_by = vec![r.findings[0].id];
    })
    .await;
    drain(&w).await;
    let runs = lock(&w.fake).check_runs.clone();
    assert_eq!(runs[0]["conclusion"].as_str(), Some("failure"));
    let comments = lock(&w.fake).comments.clone();
    let summary = comments
        .values()
        .map(|(_, body)| body)
        .find(|body| body.starts_with("<!-- sych:summary:"))
        .expect("summary");
    assert!(summary.contains("Blocking:"), "{summary}");
}

/// A run that read a policy with `review.block_on` and then failed fails its check closed, as a
/// required check passes on neutral; a later run that fails before reading the policy keeps the
/// change's last known gate. Without one the check stays neutral.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_run_fails_its_check_when_the_policy_has_block_on() {
    for (recorded, conclusion) in [
        (Some(true), "failure"),
        (Some(false), "neutral"),
        (None, "neutral"),
    ] {
        let w = world(None).await;
        let head = w.branch("src/b.rs");
        w.open_pull(&head, &w.base, false);
        let s = &w.app.storage;
        w.pr_event("d-1", "opened", &head).await;
        w.schedule(6).await;
        let mut inputs = s.latest(1).await.expect("latest").expect("run").inputs;
        (inputs.checks, inputs.publish) = (true, true);
        let fail = async |block_on: Option<bool>| {
            s.generate(1, &inputs, now().as_second() + 600, 3, now())
                .await
                .expect("gen");
            let c = s.claim("w", 60, now()).await.expect("claim").expect("job");
            if let Some(block_on) = block_on {
                let snap = sych_core::SourceSnapshotId(sych_core::sha256(b"snap"));
                let policy = sych_core::PolicySnapshotId(sych_core::sha256(b"policy"));
                s.prepare_run(
                    &c.fence,
                    &c.run,
                    &snap,
                    &policy,
                    block_on,
                    &json!({}),
                    None,
                    now(),
                )
                .await
                .expect("prepare");
            }

            s.enqueue_failure_check(&c.run, now().as_second())
                .await
                .expect("enqueue")
                .expect("an operation");
            drain(&w).await;
        };
        fail(recorded).await;
        fail(None).await;
        let runs = lock(&w.fake).check_runs.clone();
        let last = runs.last().expect("a check run");
        assert_eq!(
            last["conclusion"].as_str(),
            Some(conclusion),
            "{recorded:?}"
        );
        assert_eq!(
            last["output"]["summary"]
                .as_str()
                .is_some_and(|b| b.contains("review.block_on")),
            recorded == Some(true),
            "{last}"
        );
    }
}
