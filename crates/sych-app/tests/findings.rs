//! Finding lifecycle on the server: lineage after each committed revision,
//! feedback and waivers only from actors with the needed repository permission, and a
//! dismissal that keeps its author and survives later revisions.

use sych_app::server::findings::Refused;
use sych_core::Timestamp;
use sych_core::config::RuleException;

mod fake_github;
mod world;

use fake_github::lock;
use world::{commit_result, world};

#[tokio::test(flavor = "current_thread")]
async fn feedback_dismisses_with_its_author_and_a_later_revision_does_not_reopen() {
    let w = world(None).await;
    let head = commit_result(&w, 1).await;
    assert_eq!(w.engine.lineage_once().await.expect("lineage"), 1);
    let findings = w.app.storage.findings_of(1).await.expect("findings");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].state, "OPEN");
    let id = findings[0].id;

    let refused = w
        .engine
        .feedback(1, id, "stranger", "FALSE_POSITIVE", "finding", "nope")
        .await;
    assert!(
        matches!(refused, Err(Refused::Permission { .. })),
        "{refused:?}"
    );
    lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    w.engine
        .feedback(
            1,
            id,
            "dev",
            "FALSE_POSITIVE",
            "finding",
            "the guard is in the caller",
        )
        .await
        .expect("feedback");
    let history = w.app.storage.finding_history(id).await.expect("history");
    assert_eq!(
        history.last().expect("event"),
        &(
            "DISMISSED".to_owned(),
            "FALSE_POSITIVE: the guard is in the caller".to_owned(),
            Some("dev".to_owned())
        )
    );

    // A rerun finds it again; it stays dismissed and the history is not rewritten.
    w.comment("d-9", "dev", "/sych rerun").await;
    w.schedule(1).await;
    world::commit_claimed(&w, &head, 1).await;
    w.engine.lineage_once().await.expect("lineage");
    let again = w.app.storage.findings_of(1).await.expect("findings");
    assert_eq!((again.len(), again[0].state.as_str()), (1, "DISMISSED"));
    assert_eq!(
        w.app
            .storage
            .finding_history(id)
            .await
            .expect("history")
            .len(),
        history.len()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn only_a_maintainer_waives_and_the_waiver_is_scoped_and_dated() {
    let w = world(None).await;
    commit_result(&w, 1).await;
    let waiver = |author: &str| RuleException {
        paths: vec!["src/**".into()],
        reason: "migration window".into(),
        owner: "backend".into(),
        author: author.into(),
        expires_at: Some(Timestamp::from_second(world::T0 + 86_400).expect("time")),
        until: None,
    };
    lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    lock(&w.fake)
        .permissions
        .insert("lead".into(), "maintain".into());
    let refused = w.engine.waive(1, "EFFECT-01", &waiver("dev")).await;
    assert!(
        matches!(refused, Err(Refused::Permission { .. })),
        "{refused:?}"
    );
    w.engine
        .waive(1, "EFFECT-01", &waiver("lead"))
        .await
        .expect("waive");
    let active = w
        .app
        .storage
        .active_waivers(1, world::T0)
        .await
        .expect("active");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].rule_id, "EFFECT-01");
    let expired = w
        .app
        .storage
        .active_waivers(1, world::T0 + 86_401)
        .await
        .expect("active");
    assert!(expired.is_empty(), "an expired waiver is not in force");
}

/// `/sych dismiss N reason` dismisses the summary's problem N as a false positive: the summary is
/// rendered again without it, and a later revision that finds it again does not publish it. A
/// line naming no problem is refused and logged.
#[tokio::test(flavor = "current_thread")]
async fn a_dismiss_command_hides_the_problem_from_this_and_later_summaries() {
    let w = world(None).await;
    let head = commit_result(&w, 2).await;
    while w.engine.publish_once().await.expect("publish") {}
    let titles = |w: &world::World| {
        let summary = lock(&w.fake).comments[&1].1.clone();
        ["Defect 0", "Defect 1"].map(|t| summary.contains(&format!(": {t}\n")))
    };
    assert_eq!(titles(&w), [true, true]);

    lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    w.comment(
        "d-8",
        "dev",
        "/sych dismiss 1 the guard is in the caller\n/sych dismiss 9 no such",
    )
    .await;
    w.schedule(1).await;
    let rows = w.app.storage.findings_of(1).await.expect("findings");
    let dismissed: Vec<_> = rows.iter().filter(|r| r.state == "DISMISSED").collect();
    assert_eq!(dismissed.len(), 1, "{rows:?}");
    let shown = titles(&w);
    assert_eq!(shown.iter().filter(|s| **s).count(), 1, "{shown:?}");
    let events = w.events().await;
    assert_eq!(
        events.iter().filter(|e| *e == "DISMISS_REFUSED").count(),
        1,
        "{events:?}"
    );
    assert!(
        events.contains(&"SUMMARY_REFRESHED".to_owned()),
        "{events:?}"
    );

    // A rerun finds both again; the dismissed one stays out of the new summary.
    w.comment("d-9", "dev", "/sych rerun").await;
    w.schedule(1).await;
    world::commit_claimed(&w, &head, 2).await;
    while w.engine.publish_once().await.expect("publish") {}
    assert_eq!(titles(&w), shown);
}
