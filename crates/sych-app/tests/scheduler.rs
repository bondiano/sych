//! Durable event → intent → generation → job traces (roadmap M2.2) on the loopback GitHub fake,
//! a local origin fetched over `file://` and the fake model: repeats, a stream of pushes, a
//! target-only move, drafts, commands, close, bot comments, CI waits and successors, a crashed
//! worker and missed webhooks.

use std::time::Duration;

use serde_json::json;
use sych_core::Coverage;
use sych_core::schedule::retry_at;
use sych_core::scm::ScmErrorKind;
use sych_storage::Admitted;
use tokio::time;

mod fake_github;
mod world;

use fake_github::{lock, pull_json};
use sych_app::server::JOB_SLACK_SECONDS;
use sych_core::config::ANALYSIS_DEADLINE_SECONDS;
use world::{advance, commit, now, world, world_server};

#[tokio::test(flavor = "current_thread")]
async fn events_become_bounded_generations_and_only_new_inputs_run_again() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    assert!(matches!(
        w.pr_event("d-1", "opened", &head).await,
        Admitted::Stored { .. }
    ));
    assert!(matches!(
        w.pr_event("d-1", "opened", &head).await,
        Admitted::Duplicate { .. }
    ));
    w.schedule(2).await;
    assert!(w.events().await.is_empty(), "debounced");
    w.schedule(4).await;
    assert!(w.engine.work_once().await.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED"]);

    // A repeat with the same head and target generates nothing new.
    w.pr_event("d-2", "synchronize", &head).await;
    w.schedule(6).await;
    // The target moves under the same head: the integration context is new.
    let moved = commit(&w.origin, "README.md", "x\n");
    w.open_pull(&head, &moved, false);
    w.pr_event("d-3", "edited", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    // Drafts wait; a stranger's command is refused; a writer's rerun runs the same inputs.
    w.open_pull(&head, &moved, true);
    w.pr_event("d-4", "converted_to_draft", &head).await;
    w.schedule(6).await;
    w.comment("d-5", "stranger", "/sych rerun").await;
    w.schedule(1).await;
    lock(&w.fake)
        .permissions
        .insert("dev".into(), "write".into());
    w.comment("d-6", "dev", "/sych rerun").await;
    w.schedule(1).await;
    // The bot's own comment never acts; a close cancels the queued rerun.
    assert!(matches!(w.comment("d-7", "sych[bot]", "/sych rerun").await,
        Admitted::Stored { ref intents, .. } if intents.is_empty()));
    lock(&w.fake).pulls.get_mut(&7).expect("pull")["state"] = json!("closed");
    w.pr_event("d-8", "closed", &head).await;
    w.schedule(1).await;
    assert_eq!(
        w.events().await,
        [
            "GENERATED",
            "SKIPPED:SAME_INPUTS",
            "GENERATED",
            "SKIPPED:DRAFT",
            "SKIPPED:UNAUTHORIZED_COMMAND",
            "GENERATED",
            "CANCELLED:CLOSED",
        ]
    );
    assert_eq!(w.states().await, ["COMPLETED", "COMPLETED", "CANCELLED"]);
    assert!(
        !w.engine.work_once().await.expect("work"),
        "nothing left to claim"
    );
    drop(w.dir);
}

#[tokio::test(flavor = "current_thread")]
async fn a_push_while_queued_supersedes_and_a_stream_waits_at_most_its_bound() {
    let w = world(None).await;
    let first = w.branch("src/b.rs");
    w.open_pull(&first, &w.base, false);
    w.pr_event("d-1", "opened", &first).await;
    w.schedule(6).await;
    let second = w.branch("src/c.rs");
    w.open_pull(&second, &w.base, false);
    for i in 0..10 {
        w.pr_event(&format!("p-{i}"), "synchronize", &second).await;
        w.schedule(4).await;
    }
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["SUPERSEDED", "COMPLETED"]);
}

const JUNIT: &[u8] = br#"<testsuites><testsuite name="s"><testcase name="t" file="src/b.rs"/></testsuite></testsuites>"#;

const CI_POLICY: &str = "schema_version: 1\nci:\n  wait_seconds: 180\n  expected_reports:\n    \
    - id: tests\n      applies_to: [\"src/**\"]\n      format: junit\n";

#[tokio::test(flavor = "current_thread")]
async fn a_long_ci_wait_leaves_room_to_retry_after_it() {
    let w = world(Some(
        &CI_POLICY.replace("wait_seconds: 180", "wait_seconds: 900"),
    ))
    .await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["WAITING_EVIDENCE"]);
    advance(901);
    let claim = w
        .app
        .storage
        .claim("probe", 60, now())
        .await
        .expect("claim")
        .expect("due at the evidence deadline");
    let evidence = claim.evidence_deadline.expect("waited for CI");
    assert!(claim.deadline >= evidence + 300 + 120, "{claim:?}");
    let retry = retry_at(
        ScmErrorKind::Transient,
        1,
        None,
        0,
        now().as_second(),
        claim.deadline,
    );
    assert!(
        retry.is_some(),
        "a transient failure after the wait is retried"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_job_deadline_follows_the_configured_analysis_deadline() {
    let configured = "budgets:\n  analysis:\n    active_deadline_seconds: 900\n";
    assert_eq!(
        job_after_evidence(configured).await,
        900 + JOB_SLACK_SECONDS
    );
    // An analysis budget without its own deadline keeps the default bound.
    let unset = "budgets:\n  analysis:\n    max_attempts: 12\n";
    assert_eq!(
        job_after_evidence(unset).await,
        i64::from(ANALYSIS_DEADLINE_SECONDS) + JOB_SLACK_SECONDS
    );
}

/// How long after its evidence deadline the job of a CI-waiting run may still retry.
async fn job_after_evidence(server: &str) -> i64 {
    let w = world_server(Some(CI_POLICY), None, server).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["WAITING_EVIDENCE"]);
    advance(181);
    let claim = w
        .app
        .storage
        .claim("probe", 60, now())
        .await
        .expect("claim")
        .expect("due at the evidence deadline");
    claim.deadline - claim.evidence_deadline.expect("waited for CI")
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_waits_for_expected_ci_and_a_late_report_makes_a_successor() {
    let w = world(Some(CI_POLICY)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["WAITING_EVIDENCE"]);
    assert!(
        !w.engine.work_once().await.expect("work"),
        "released until the deadline"
    );
    w.publish_ci(555, &head, "success", Some(&head), JUNIT);
    w.ci("c-1", &head).await;
    w.schedule(11).await;
    assert!(w.engine.work_once().await.expect("work"), "woken by CI");
    assert_eq!(w.states().await, ["COMPLETED"]);
    let first = w.result(0).await;
    assert!(
        first
            .unresolved_questions
            .iter()
            .all(|q| !q.contains("CI_")),
        "{:?}",
        first.unresolved_questions
    );

    // A new head whose CI misses the deadline: the run continues with a gap, then the late
    // report makes a successor that keeps its parent.
    let next = w.branch("src/c.rs");
    w.open_pull(&next, &w.base, false);
    w.pr_event("d-2", "synchronize", &next).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    advance(181);
    w.engine.work_once().await.expect("work");
    assert_eq!(w.states().await, ["COMPLETED", "COMPLETED"]);
    let gapped = w.result(1).await;
    assert_eq!(
        gapped.coverage,
        Coverage::Partial,
        "a missing expected report is not a pass"
    );
    assert!(
        gapped
            .unresolved_questions
            .iter()
            .any(|q| q.starts_with("CI_JOB_MISSING for report `tests`"))
    );
    w.publish_ci(556, &next, "success", Some(&next), JUNIT);
    w.ci("c-2", &next).await;
    w.schedule(11).await;
    let latest = w.app.storage.latest(1).await.expect("latest").expect("run");
    let (_, _, generation, parent) = w
        .app
        .storage
        .run_row("local", &latest.run_id)
        .await
        .expect("row");
    assert_eq!(generation, 3);
    assert!(parent.is_some(), "successor keeps its parent");
    assert!(w.engine.work_once().await.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED", "COMPLETED", "COMPLETED"]);
    let successor = w.result(2).await;
    assert_eq!(
        successor.parent_result_id,
        Some(gapped.result_id),
        "the successor's result links its parent's"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_crashed_worker_is_reaped_and_missed_webhooks_are_reconciled() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    // No webhook at all: reconciliation finds the open change.
    w.open_pull(&head, &w.base, false);
    w.engine.reconcile_once().await.expect("reconcile");
    w.engine
        .reconcile_once()
        .await
        .expect("a repeat is a no-op");
    w.schedule(6).await;
    let crashed = w
        .app
        .storage
        .claim("w0", 60, now())
        .await
        .expect("claim")
        .expect("job");
    assert!(
        !w.engine.work_once().await.expect("work"),
        "leased by the crashed worker"
    );
    advance(61);
    assert_eq!(w.app.storage.reap(now()).await.expect("reap"), 1);
    assert!(w.engine.work_once().await.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED"]);
    let jobs = w
        .app
        .storage
        .jobs_of(&crashed.run.id.to_string())
        .await
        .expect("jobs");
    assert_eq!(jobs[0].0, "DONE");
    assert!(jobs[0].1 > crashed.fence.epoch);

    // The change was closed while the server was down.
    lock(&w.fake).pulls.clear();
    w.engine.reconcile_once().await.expect("reconcile");
    lock(&w.fake).pulls.insert(7, {
        let mut p = pull_json(7, &head, &w.base);
        p["state"] = json!("closed");
        p
    });
    w.schedule(1).await;
    let change = w.app.storage.change(1).await.expect("change");
    assert_eq!(change.scm_state.as_deref(), Some("CLOSED"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_slow_preparation_keeps_its_lease() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    lock(&w.fake).token_delay = Some(Duration::from_millis(300));
    // Past the cached token's expiry, so the job asks for a new one; that request outlasts
    // the 60 s lease twice over while heartbeats renew it.
    advance(7_200);
    let clock = async {
        for _ in 0..4 {
            time::sleep(Duration::from_millis(50)).await;
            advance(30);
        }
    };
    let (worked, ()) = tokio::join!(w.engine.work_once(), clock);
    assert!(worked.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_second_worker_cannot_claim_a_job_still_preparing() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    lock(&w.fake).token_delay = Some(Duration::from_millis(300));
    advance(7_200);
    // A second worker reaps and claims at every tick while the first one is still fetching.
    let rival = async {
        let mut taken = Vec::new();
        for _ in 0..4 {
            time::sleep(Duration::from_millis(50)).await;
            advance(30);
            let reaped = w.app.storage.reap(now()).await.expect("reap");
            let claim = w.app.storage.claim("w2", 60, now()).await.expect("claim");
            taken.push((reaped, claim.is_some()));
        }
        taken
    };
    let (worked, taken) = tokio::join!(w.engine.work_once(), rival);
    assert!(worked.expect("work"));
    assert_eq!(taken, [(0, false); 4], "the heartbeat keeps the lease");
    assert_eq!(w.states().await, ["COMPLETED"]);
    assert!(
        !w.engine.work_once().await.expect("work"),
        "nothing left to claim"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_repeated_ci_report_after_its_run_used_it_is_not_significant() {
    let w = world(Some(CI_POLICY)).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    w.engine.work_once().await.expect("work");
    w.publish_ci(555, &head, "success", Some(&head), JUNIT);
    w.ci("c-1", &head).await;
    w.schedule(11).await;
    assert!(w.engine.work_once().await.expect("work"));
    assert_eq!(w.states().await, ["COMPLETED"]);

    // The same artifact again under a new delivery: the run already has it.
    assert!(matches!(w.ci("c-2", &head).await, Admitted::Stored { .. }));
    w.schedule(11).await;
    assert_eq!(
        w.events().await.last().map(String::as_str),
        Some("SKIPPED:CI_NOT_SIGNIFICANT")
    );
    assert!(!w.engine.work_once().await.expect("work"), "no new run");
    assert_eq!(w.states().await, ["COMPLETED"]);
}

#[tokio::test(flavor = "current_thread")]
async fn ci_the_policy_does_not_expect_makes_no_successor() {
    let w = world(None).await;
    let head = w.branch("src/b.rs");
    w.open_pull(&head, &w.base, false);
    w.pr_event("d-1", "opened", &head).await;
    w.schedule(6).await;
    assert!(w.engine.work_once().await.expect("work"));
    let run = w.run_ids().await;

    w.publish_ci(555, &head, "success", Some(&head), JUNIT);
    w.ci("c-1", &head).await;
    w.schedule(11).await;
    assert_eq!(
        w.events().await.last().map(String::as_str),
        Some("SKIPPED:CI_NOT_SIGNIFICANT")
    );
    assert!(!w.engine.work_once().await.expect("work"));
    assert_eq!(w.run_ids().await, run, "no successor generation");
    let (_, _, generation, parent) = w.app.storage.run_row("local", &run[0]).await.expect("row");
    assert_eq!((generation, parent), (1, None));
}
