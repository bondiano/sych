//! Inline comments across revisions: a later revision that no longer reports a finding, after a
//! complete re-check, rewrites that finding's comment to say so, its text folded below. A
//! partial revision leaves every comment as it is: a missing finding is not a fix.

use sych_core::architecture::{ArchitectureFacts, Limits as ArchLimits};
use sych_core::db_schema::{DbSchemaFacts, DbSchemaSource, SqlDialect};
use sych_core::{
    Applicability, BackendStatus, Coverage, FindingCategory, Language, ObligationStatus, Producer,
    RunFacts, SchemaVersion, SourceSnapshotId, sha256,
};

mod fake_github;
mod world;

use fake_github::lock;
use world::{commit, git, world};

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

fn arch_facts(snap_id: SourceSnapshotId, status: BackendStatus) -> ArchitectureFacts {
    ArchitectureFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snap_id,
        producer: Producer {
            name: "rust-analyzer".into(),
            version: "1".into(),
            config_hash: None,
        },
        language: Language::Rust,
        project: String::new(),
        status,
        base_checked: status == BackendStatus::Ready,
        limits: ArchLimits::default(),
        coverage: Vec::new(),
        findings: Vec::new(),
        fixed: Vec::new(),
        limitations: Vec::new(),
    }
}

fn db_facts(snap_id: SourceSnapshotId, status: BackendStatus) -> DbSchemaFacts {
    DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snap_id,
        producer: Producer {
            name: "sych-db-schema".into(),
            version: "0.2.0".into(),
            config_hash: None,
        },
        target: "migrations".into(),
        dialect: SqlDialect::Postgres,
        source: DbSchemaSource::StaticSql,
        ci_authenticated: false,
        model_extractor: None,
        status,
        base_checked: true,
        findings: Vec::new(),
        fixed: Vec::new(),
        unmatched_exclusions: Vec::new(),
        warnings: Vec::new(),
        skipped_rules: Vec::new(),
        limitations: Vec::new(),
    }
}

fn count_resolved_inline(w: &world::World) -> usize {
    lock(&w.fake)
        .review_comments
        .values()
        .filter(|c| {
            c["body"]
                .as_str()
                .is_some_and(|b| b.contains("<!-- sych:resolved -->"))
        })
        .count()
}

#[tokio::test(flavor = "current_thread")]
async fn fixing_sec_and_db_findings_resolves_them_and_updates_inline_comments_unless_disabled_or_unavailable()
 {
    let w = world(None).await;
    let head1 = w.branch("src/b.rs");
    w.open_pull(&head1, &w.base, false);
    w.pr_event("d-1", "opened", &head1).await;
    w.schedule(6).await;

    world::commit_claimed_with(&w, &head1, 3, b"policy", |r| {
        r.coverage = Coverage::CompleteForScope;
        r.obligations.clear();
        r.scope_manifest.units[0].paths = vec!["src/b.rs".parse().expect("path")];
        r.scope_manifest.requested_profiles = vec!["security".into()];
        for (i, (cat, rule, fp)) in [
            (
                FindingCategory::Defect,
                "SEC-SECRET",
                "SEC-SECRET\0src/b.rs",
            ),
            (
                FindingCategory::RuleViolation,
                "DB-FK-WITHOUT-INDEX",
                "db-schema\0fk-without-index\0orders.user_id",
            ),
            (
                FindingCategory::RuleViolation,
                "ARCH-FIELD-LEAK",
                "architecture\0field_leak\0Order.total",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            r.findings[i].category = cat;
            r.findings[i].rule_id = Some(rule.into());
            r.findings[i].obligation_ids.clear();
            r.findings[i].fingerprint = sha256(fp.as_bytes());
        }
    })
    .await;
    while w.engine.publish_once().await.expect("publish rev1") {}
    w.engine.lineage_once().await.expect("lineage rev1");
    assert_eq!(lock(&w.fake).review_comments.len(), 3);

    // Revision 2: schema_lint disabled and architecture unavailable — SEC-SECRET resolves, DB-* and ARCH-* stay OPEN.
    git(&w.origin, &["checkout", "-q", "pr"]);
    let head2 = commit(&w.origin, "src/b.rs", "pub fn b_clean() {}\n");
    git(&w.origin, &["checkout", "-q", "main"]);
    w.open_pull(&head2, &w.base, false);
    w.pr_event("d-2", "synchronize", &head2).await;
    w.schedule(6).await;
    let snap_id = w.result(0).await.source_snapshot_id;
    let unavailable = RunFacts {
        architecture: vec![arch_facts(snap_id.clone(), BackendStatus::Failed)],
        ..RunFacts::default()
    };
    world::commit_claimed_with_facts(&w, &head2, 0, b"policy", Some(&unavailable), |r| {
        r.coverage = Coverage::CompleteForScope;
        r.obligations.clear();
        r.scope_manifest.units[0].paths = vec!["src/b.rs".parse().expect("path")];
        r.scope_manifest.requested_profiles = vec!["security".into()];
    })
    .await;
    while w.engine.publish_once().await.expect("publish rev2") {}
    w.engine.lineage_once().await.expect("lineage rev2");
    let rows = w.app.storage.findings_of(1).await.expect("findings");
    assert_eq!(
        rows.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(),
        ["RESOLVED", "OPEN", "OPEN"]
    );
    assert_eq!(count_resolved_inline(&w), 1);

    // Revision 3: schema_lint and architecture both Ready — DB-* and ARCH-* resolve and update inline comments.
    git(&w.origin, &["checkout", "-q", "pr"]);
    let head3 = commit(&w.origin, "src/b.rs", "pub fn b_ready() {}\n");
    git(&w.origin, &["checkout", "-q", "main"]);
    w.open_pull(&head3, &w.base, false);
    w.pr_event("d-3", "synchronize", &head3).await;
    w.schedule(6).await;
    let ready = RunFacts {
        db_schema: vec![db_facts(snap_id.clone(), BackendStatus::Ready)],
        architecture: vec![arch_facts(snap_id, BackendStatus::Ready)],
        ..RunFacts::default()
    };
    world::commit_claimed_with_facts(&w, &head3, 0, b"policy", Some(&ready), |r| {
        r.coverage = Coverage::CompleteForScope;
        r.obligations.clear();
        r.scope_manifest.units[0].paths = vec!["src/b.rs".parse().expect("path")];
        r.scope_manifest.requested_profiles = vec!["security".into()];
    })
    .await;
    while w.engine.publish_once().await.expect("publish rev3") {}
    w.engine.lineage_once().await.expect("lineage rev3");
    let rows = w.app.storage.findings_of(1).await.expect("findings");
    assert_eq!(
        rows.iter().map(|r| r.state.as_str()).collect::<Vec<_>>(),
        ["RESOLVED", "RESOLVED", "RESOLVED"]
    );
    assert_eq!(count_resolved_inline(&w), 3);
}
