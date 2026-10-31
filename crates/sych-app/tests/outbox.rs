//! Durable outbox and reconciliation traces on the GitHub fake: one CREATE per summary whatever is
//! lost, updates by the stored id, reconciliation only of the bot's own comment, CREATE again only
//! after a complete empty read, a lost close stops the write, and a late older write is repaired.

use serde_json::json;

mod fake_github;
mod world;

use fake_github::lock;
use world::{World, advance, world};

/// Opens PR 7 at a fresh head and lets the worker produce a committed result.
async fn reviewed(w: &World, file: &str, delivery: &str) -> String {
    let head = w.branch(file);
    w.open_pull(&head, &w.base, false);
    w.pr_event(delivery, "synchronize", &head).await;
    w.schedule(6).await;
    assert!(w.engine.work_once().await.expect("work"));
    head
}

fn creates(w: &World) -> usize {
    lock(&w.fake)
        .log
        .iter()
        .filter(|l| l.starts_with("POST /repos/o/r/issues/7/comments"))
        .count()
}

async fn states(w: &World) -> Vec<String> {
    w.app
        .storage
        .operations(1)
        .await
        .expect("ops")
        .into_iter()
        .map(|o| o.state)
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn the_summary_is_created_once_and_then_updated_by_its_id() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    assert!(w.engine.publish_once().await.expect("publish"));
    reviewed(&w, "src/c.rs", "d-2").await;
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(creates(&w), 1);
    let comments = lock(&w.fake).comments.clone();
    assert_eq!(comments.len(), 1);
    let (author, body) = &comments[&1];
    assert_eq!(author, "sych[bot]");
    assert!(
        body.starts_with("<!-- sych:summary:1 -->") && body.contains("revision 2"),
        "{body}"
    );
    assert_eq!(states(&w).await, ["DELIVERED", "DELIVERED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_is_uncertain_and_reconciled_by_marker_across_pages() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    {
        let mut f = lock(&w.fake);
        // A full first page of other comments, and a human copying the marker.
        for i in 0..100 {
            f.comments
                .insert(1_000 + i, ("dev".into(), format!("comment {i}")));
        }
        f.comments.insert(
            5_000,
            ("dev".into(), "<!-- sych:summary:1 --> copied".into()),
        );
        f.next_id = 9_000;
        f.lose_next_write_reply = true;
    }
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["UNCERTAIN"]);
    assert!(!w.engine.publish_once().await.expect("nothing else to send"));
    w.engine
        .reconcile_publications_once()
        .await
        .expect("reconcile");
    assert_eq!(states(&w).await, ["DELIVERED"]);
    assert_eq!(creates(&w), 1, "no second CREATE");
    let ops = w.app.storage.operations(1).await.expect("ops");
    assert_eq!(ops.len(), 1);
    // Reconciled to the bot's comment, not the human copy.
    reviewed(&w, "src/c.rs", "d-2").await;
    w.engine.publish_once().await.expect("publish");
    assert!(lock(&w.fake).comments[&9_001].1.contains("revision 2"));
    assert!(lock(&w.fake).comments[&5_000].1.ends_with("copied"));
}

/// Claims, marks PUBLISHING and dies before sending; the reaper makes it UNCERTAIN.
async fn crashed(w: &World) {
    let s = &w.app.storage;
    let c = s
        .claim_publication("dead", 60, world::now())
        .await
        .expect("claim")
        .expect("op");
    assert!(
        s.begin_publishing(&c, "body", world::now())
            .await
            .expect("begin")
            .is_some()
    );
    advance(61);
    w.engine.reap_once().await.expect("reap");
    assert_eq!(
        states(w).await,
        ["UNCERTAIN"],
        "a lost lease is never PENDING"
    );
}

async fn reconcile(w: &World) {
    w.engine
        .reconcile_publications_once()
        .await
        .expect("reconcile");
}

#[tokio::test(flavor = "current_thread")]
async fn a_first_publish_5xx_is_created_again_once_a_complete_read_after_the_grace_is_empty() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    lock(&w.fake).fail_next.insert(
        "POST /repos/o/r/issues/7/comments".into(),
        (502, json!({"message": "Bad Gateway"})),
    );
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["UNCERTAIN"]);
    reconcile(&w).await;
    assert_eq!(
        states(&w).await,
        ["UNCERTAIN"],
        "inside the grace an empty read proves nothing"
    );

    advance(301);
    reconcile(&w).await;
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["DELIVERED"]);
    assert_eq!(creates(&w), 2, "the refused POST and one CREATE");
    assert_eq!(lock(&w.fake).comments.len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_crash_while_publishing_is_created_again_only_after_a_complete_empty_read() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    crashed(&w).await;
    advance(301);
    // A failed read is not absence.
    lock(&w.fake).fail_next.insert(
        "GET /repos/o/r/issues/7/comments".into(),
        (502, json!({"message": "Bad Gateway"})),
    );
    reconcile(&w).await;
    assert_eq!(states(&w).await, ["UNCERTAIN"]);
    assert!(!w.engine.publish_once().await.expect("publish"));

    advance(31);
    reconcile(&w).await;
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["DELIVERED"]);
    assert_eq!(creates(&w), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn without_a_complete_read_it_waits_for_the_operator_and_never_creates_twice() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    crashed(&w).await;
    advance(3_601);
    lock(&w.fake).fail_next.insert(
        "GET /repos/o/r/issues/7/comments".into(),
        (502, json!({"message": "Bad Gateway"})),
    );
    reconcile(&w).await;
    advance(31);
    reconcile(&w).await;
    assert_eq!(
        states(&w).await,
        ["UNCERTAIN"],
        "past the window it waits for the operator"
    );
    assert!(!w.engine.publish_once().await.expect("publish"));
    assert_eq!(creates(&w), 0);

    let s = &w.app.storage;
    let op = s.operations(1).await.expect("ops")[0].id;
    assert!(
        s.authorize_recreate(op, "local", "owner", "checked the PR by hand", world::now())
            .await
            .expect("authorize")
    );
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["DELIVERED"]);
    assert_eq!(creates(&w), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_lost_close_stops_the_write_and_a_late_old_update_is_repaired() {
    let w = world(None).await;
    reviewed(&w, "src/b.rs", "d-1").await;
    w.engine.publish_once().await.expect("publish");
    // Someone (or a late request of an older revision) rewrites the bot's comment.
    lock(&w.fake).comments.get_mut(&1).expect("comment").1 = "<!-- sych:summary:1 --> old".into();
    assert_eq!(w.engine.verify_published_once().await.expect("verify"), 1);
    assert!(lock(&w.fake).comments[&1].1.contains("revision 1"));

    reviewed(&w, "src/c.rs", "d-2").await;
    // Closed on GitHub, no webhook arrived.
    lock(&w.fake).pulls.get_mut(&7).expect("pull")["state"] = json!("closed");
    assert!(w.engine.publish_once().await.expect("publish"));
    assert_eq!(states(&w).await, ["DELIVERED", "CANCELLED"]);
    assert!(
        !lock(&w.fake).comments[&1].1.contains("revision 2"),
        "no write after close"
    );
}
