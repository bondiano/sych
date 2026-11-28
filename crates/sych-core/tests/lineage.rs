//! Lineage corpus: what an open finding of the previous revision becomes. A
//! missing finding is never a fix by itself; a rename continues the problem or keeps it open.

use std::fs;
use std::path::Path;
use std::slice;

use sych_core::architecture::{ArchitectureFacts, Limits as ArchLimits};
use sych_core::db_schema::{DbSchemaFacts, DbSchemaSource, SqlDialect};
use sych_core::lineage::{Decision, Prior, decide};
use sych_core::{
    BackendStatus, Coverage, DiffManifest, Finding, FindingCategory, Language, ObligationStatus,
    PathGap, Producer, ReviewResult, RunFacts, SchemaVersion, decode, sha256,
};

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name),
    )
    .expect("fixture")
}

fn manifest() -> DiffManifest {
    serde_json::from_str(&fixture("publication/diff-manifest.json")).expect("manifest")
}

/// A complete revision that reviewed the modified and the renamed file; EFFECT-01 SUPPORTED.
fn revision() -> ReviewResult {
    let mut r: ReviewResult = decode(&fixture("core/v1/review-result.json")).expect("result");
    r.coverage = Coverage::CompleteForScope;
    r.findings.clear();
    r.unresolved_questions.clear();
    let mut unit = r.scope_manifest.units[0].clone();
    unit.paths = vec![
        "backend/orders/create.go".parse().expect("path"),
        "src/новое_имя.rs".parse().expect("path"),
    ];
    r.scope_manifest.units = vec![unit];
    for o in &mut r.obligations {
        o.status = ObligationStatus::Supported;
        o.applicability = sych_core::Applicability::Applies;
    }
    r
}

fn prior(rule: Option<&str>, path: &str) -> Prior {
    Prior {
        fingerprint: sha256(format!("{rule:?}{path}").as_bytes()).to_string(),
        category: if rule.is_some() {
            FindingCategory::RuleViolation
        } else {
            FindingCategory::Defect
        },
        rule_id: rule.map(str::to_owned),
        path: Some(path.into()),
    }
}

fn finding_at(path: &str, fingerprint: &str) -> Finding {
    let mut f = decode::<ReviewResult>(&fixture("core/v1/review-result.json"))
        .expect("result")
        .findings[0]
        .clone();
    f.category = FindingCategory::Defect;
    f.rule_id = None;
    f.anchors[0].path = path.parse().expect("path");
    f.fingerprint = fingerprint.parse().expect("digest");
    f
}

fn one(p: &Prior, r: &ReviewResult) -> Decision {
    decide(slice::from_ref(p), r, &manifest(), &RunFacts::default())
        .remove(0)
        .1
}

fn one_with_facts(p: &Prior, r: &ReviewResult, facts: &RunFacts) -> Decision {
    decide(slice::from_ref(p), r, &manifest(), facts)
        .remove(0)
        .1
}

fn open_reason(d: &Decision) -> &str {
    match d {
        Decision::Open { reason } => reason,
        other => panic!("expected open, got {other:?}"),
    }
}

#[test]
fn a_rule_finding_resolves_only_on_a_comparable_complete_recheck() {
    let p = prior(Some("EFFECT-01"), "backend/orders/create.go");
    assert!(matches!(one(&p, &revision()), Decision::Resolved { .. }));

    let mut partial = revision();
    partial.coverage = Coverage::Partial;
    assert_eq!(
        open_reason(&one(&p, &partial)),
        "the new revision was reviewed partially"
    );

    let mut removed = revision();
    removed.obligations.retain(|o| o.rule_id != "EFFECT-01");
    assert_eq!(
        open_reason(&one(&p, &removed)),
        "the rule was not evaluated again"
    );

    let mut unknown = revision();
    for o in &mut unknown.obligations {
        o.status = ObligationStatus::Unknown {
            reason_code: "CI_JOB_SKIPPED".into(),
            next_evidence_needed: "tests".into(),
        };
    }
    assert_eq!(
        open_reason(&one(&p, &unknown)),
        "the rule is still not supported"
    );

    let elsewhere = prior(Some("EFFECT-01"), "README.md");
    assert_eq!(
        open_reason(&one(&elsewhere, &revision())),
        "its file was not reviewed again"
    );
}

#[test]
fn renames_continue_or_stay_open_and_deletions_are_obsolete() {
    let p = prior(None, "src/old_name.rs");
    let mut moved = revision();
    moved.findings = vec![finding_at(
        "src/новое_имя.rs",
        &sha256(b"moved").to_string(),
    )];
    assert_eq!(
        one(&p, &moved),
        Decision::Continued {
            successor: sha256(b"moved").to_string()
        }
    );
    let mut ambiguous = moved.clone();
    ambiguous
        .findings
        .push(finding_at("src/новое_имя.rs", &sha256(b"twin").to_string()));
    assert_eq!(
        open_reason(&one(&p, &ambiguous)),
        "renamed or moved; not matched again with certainty"
    );
    assert_eq!(
        open_reason(&one(&p, &revision())),
        "renamed or moved; not matched again with certainty",
        "no successor: nothing closes"
    );

    let gone = prior(None, "src/gone.rs");
    assert!(matches!(one(&gone, &revision()), Decision::Obsolete { .. }));
}

#[test]
fn a_model_finding_needs_changed_code_and_no_finding_of_its_kind_there() {
    let changed = prior(None, "backend/orders/create.go");
    assert!(matches!(
        one(&changed, &revision()),
        Decision::Resolved { .. }
    ));
    let mut again = revision();
    again.findings = vec![finding_at(
        "backend/orders/create.go",
        &sha256(b"other").to_string(),
    )];
    assert_eq!(
        open_reason(&one(&changed, &again)),
        "a finding of the same kind is still there"
    );

    let same = Prior {
        fingerprint: sha256(b"same").to_string(),
        ..changed
    };
    let mut matched = revision();
    matched.findings = vec![finding_at("backend/orders/create.go", &same.fingerprint)];
    assert_eq!(one(&same, &matched), Decision::Matched);
}

/// An earlier revision's inline comment says its finding is gone only when lineage closes it,
/// and gets its text back when the finding returns; a partial review touches nothing.
#[test]
fn an_inline_comment_is_rewritten_only_when_lineage_decides() {
    use sych_core::ReviewLanguage;
    use sych_core::publication::{GONE_MARKER, Posted, gone_body, revisit};

    let posted = |object_id, noted| Posted {
        object_id,
        prior: prior(Some("EFFECT-01"), "backend/orders/create.go"),
        dismissed: false,
        noted,
    };
    let resolved = revision();
    let facts = RunFacts::default();
    assert_eq!(
        revisit(&[posted(1, false)], &resolved, &manifest(), &facts),
        [1]
    );
    let none: [i64; 0] = [];
    assert_eq!(
        revisit(&[posted(1, true)], &resolved, &manifest(), &facts),
        none
    );

    let mut partial = revision();
    partial.coverage = Coverage::Partial;
    assert_eq!(
        revisit(
            &[posted(1, false), posted(2, true)],
            &partial,
            &manifest(),
            &facts
        ),
        none
    );

    let mut back = revision();
    let mut again = finding_at(
        "backend/orders/create.go",
        &posted(1, true).prior.fingerprint,
    );
    again.rule_id = Some("EFFECT-01".into());
    back.findings = vec![again];
    assert_eq!(
        revisit(
            &[posted(1, true), posted(2, false)],
            &back,
            &manifest(),
            &facts
        ),
        [1]
    );

    // A finding the change dismissed is gone too, though the result still has it.
    let mut dismissed = back.clone();
    dismissed.findings[0].lifecycle = sych_core::Lifecycle::Dismissed;
    assert_eq!(
        revisit(&[posted(1, false)], &dismissed, &manifest(), &facts),
        [1]
    );
    assert_eq!(
        revisit(&[posted(1, true)], &dismissed, &manifest(), &facts),
        none
    );
    // A dismissed finding is also gone on a partial review that no longer reports it.
    let dismissed_posted = Posted {
        dismissed: true,
        ..posted(1, false)
    };
    assert_eq!(
        revisit(&[dismissed_posted], &partial, &manifest(), &facts),
        [1]
    );

    let body = gone_body(
        "sha256:ab",
        "0123456789abcdef",
        "<!-- sych:inline:sha256:ab -->\n**[P1] Lost event**\n\nDetails.",
        ReviewLanguage::En,
    );
    assert!(
        body.starts_with("<!-- sych:inline:sha256:ab -->\n"),
        "{body}"
    );
    assert!(
        body.contains(GONE_MARKER) && body.contains("`0123456789ab`"),
        "{body}"
    );
    assert_eq!(body.matches("<!-- sych:inline:").count(), 1, "{body}");
    assert!(
        body.contains("<summary>Original comment</summary>\n\n**[P1] Lost event**"),
        "{body}"
    );
    let bare = gone_body("sha256:ab", "0123456789abcdef", "", ReviewLanguage::En);
    assert!(!bare.contains("<details>"), "nothing to fold: {bare}");
}

#[test]
fn sec_and_preference_rules_resolve_without_obligations() {
    let mut sec_result = revision();
    sec_result
        .scope_manifest
        .requested_profiles
        .push("security".into());
    let sec_prior = Prior {
        fingerprint: sha256(b"SEC-SECRET\0backend/orders/create.go").to_string(),
        category: FindingCategory::Defect,
        rule_id: Some("SEC-SECRET".into()),
        path: Some("backend/orders/create.go".into()),
    };
    assert!(matches!(
        one(&sec_prior, &sec_result),
        Decision::Resolved { .. }
    ));

    let reverted_sec = Prior {
        path: Some("src/reverted.rs".into()),
        ..sec_prior.clone()
    };
    assert!(matches!(
        one(&reverted_sec, &sec_result),
        Decision::Resolved { .. }
    ));
    assert_eq!(
        open_reason(&one(&sec_prior, &revision())),
        "the security scan did not run"
    );
    let mut excluded_sec = sec_result;
    excluded_sec.excluded_or_unreviewed.push(PathGap {
        path: "backend/orders/create.go".parse().expect("path"),
        reason: "GENERATED".into(),
    });
    assert_eq!(
        open_reason(&one(&sec_prior, &excluded_sec)),
        "its file was not scanned"
    );

    let pref_prior = Prior {
        fingerprint: sha256(b"pref-1").to_string(),
        category: FindingCategory::Nit,
        rule_id: Some("PREF-01".into()),
        path: Some("backend/orders/create.go".into()),
    };
    let mut with_other_nit = revision();
    let mut other_nit = finding_at("backend/orders/create.go", &sha256(b"pref-2").to_string());
    other_nit.category = FindingCategory::Nit;
    other_nit.rule_id = Some("PREF-02".into());
    with_other_nit.findings = vec![other_nit];
    assert!(
        matches!(one(&pref_prior, &with_other_nit), Decision::Resolved { .. }),
        "an unrelated preference nit on the same file must not keep PREF-01 open"
    );

    let mut with_same_nit = revision();
    let mut same_nit = finding_at(
        "backend/orders/create.go",
        &sha256(b"pref-1-new").to_string(),
    );
    same_nit.category = FindingCategory::Nit;
    same_nit.rule_id = Some("PREF-01".into());
    with_same_nit.findings = vec![same_nit];
    assert_eq!(
        open_reason(&one(&pref_prior, &with_same_nit)),
        "a finding of the same kind is still there"
    );
}

#[test]
fn db_and_arch_rules_resolve_when_ready_and_stay_open_when_disabled_or_unavailable() {
    let db_prior = prior(Some("DB-FK-WITHOUT-INDEX"), "backend/orders/create.go");
    assert_eq!(
        open_reason(&one_with_facts(
            &db_prior,
            &revision(),
            &RunFacts::default()
        )),
        "schema_lint did not run"
    );
    let db_fact = |status: BackendStatus| DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: revision().source_snapshot_id,
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
    };
    let db_ready = RunFacts {
        db_schema: vec![db_fact(BackendStatus::Ready)],
        ..RunFacts::default()
    };
    assert!(matches!(
        one_with_facts(&db_prior, &revision(), &db_ready),
        Decision::Resolved { .. }
    ));
    let db_failed = RunFacts {
        db_schema: vec![db_fact(BackendStatus::Failed)],
        ..RunFacts::default()
    };
    assert_eq!(
        open_reason(&one_with_facts(&db_prior, &revision(), &db_failed)),
        "schema_lint was unavailable"
    );

    let arch_prior = prior(Some("ARCH-FIELD-LEAK"), "backend/orders/create.go");
    assert_eq!(
        open_reason(&one_with_facts(
            &arch_prior,
            &revision(),
            &RunFacts::default()
        )),
        "the architecture check did not run"
    );
    let arch_fact = |status: BackendStatus| ArchitectureFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: revision().source_snapshot_id,
        producer: Producer {
            name: "scip-go".into(),
            version: "0.1".into(),
            config_hash: None,
        },
        language: Language::Go,
        project: "backend".into(),
        status,
        base_checked: true,
        limits: ArchLimits::default(),
        coverage: Vec::new(),
        findings: Vec::new(),
        fixed: Vec::new(),
        limitations: Vec::new(),
    };
    let arch_ready = RunFacts {
        architecture: vec![arch_fact(BackendStatus::Ready)],
        ..RunFacts::default()
    };
    assert!(matches!(
        one_with_facts(&arch_prior, &revision(), &arch_ready),
        Decision::Resolved { .. }
    ));
    let arch_failed = RunFacts {
        architecture: vec![arch_fact(BackendStatus::Failed)],
        ..RunFacts::default()
    };
    assert_eq!(
        open_reason(&one_with_facts(&arch_prior, &revision(), &arch_failed)),
        "the architecture check was unavailable"
    );
}
