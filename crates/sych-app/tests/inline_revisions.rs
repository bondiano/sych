//! Inline comments across revisions: a later revision that no longer reports a finding, after a
//! complete re-check, rewrites that finding's comment to say so, its text folded below. A
//! partial revision leaves every comment as it is: a missing finding is not a fix.

use sych_core::{Applicability, Coverage, ObligationStatus};

mod fake_github;
mod world;

use fake_github::lock;
use world::world;

#[tokio::test(flavor = "current_thread")]
async fn an_inline_comment_says_when_a_later_revision_no_longer_reports_it() {
    for (coverage, rewritten) in [
        (Coverage::CompleteForScope, true),
        (Coverage::Partial, false),
    ] {
        let w = world(None).await;
        let head = world::commit_result(&w, 2).await;
        while w.engine.publish_once().await.expect("publish") {}
        w.engine.lineage_once().await.expect("lineage");
        let before = lock(&w.fake).review_comments.clone();
        assert_eq!(before.len(), 2);

        // A rerun of the head that finds only `Defect 0`, with EFFECT-01 assessed again.
        lock(&w.fake)
            .permissions
            .insert("dev".into(), "write".into());
        w.comment("d-9", "dev", "/sych rerun").await;
        w.schedule(1).await;
        world::commit_claimed_with(&w, &head, 1, b"policy", |r| {
            r.coverage = coverage;
            r.scope_manifest.units[0].paths = vec!["src/b.rs".parse().expect("path")];
            for o in &mut r.obligations {
                o.status = ObligationStatus::Supported;
                o.applicability = Applicability::Applies;
            }
        })
        .await;
        while w.engine.publish_once().await.expect("publish") {}

        let after = lock(&w.fake).review_comments.clone();
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>()
        );
        let body = |id| after[id]["body"].as_str().expect("body").to_owned();
        let changed: Vec<String> = before
            .iter()
            .filter(|(id, c)| c["body"] != after[*id]["body"])
            .map(|(id, _)| body(id))
            .collect();
        assert_eq!(changed.len(), usize::from(rewritten), "{after:?}");
        if let Some(gone) = changed.first() {
            assert!(gone.contains("<!-- sych:resolved -->"), "{gone}");
            assert!(
                gone.contains(&format!(
                    "Sych no longer reports this finding at `{}`",
                    &head[..12]
                )),
                "{gone}"
            );
            assert!(
                gone.contains("Defect 1") && !gone.contains("Defect 0"),
                "{gone}"
            );
        }
    }
}
