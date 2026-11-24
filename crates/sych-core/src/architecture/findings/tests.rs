use super::*;
use crate::architecture::Limits;
use crate::architecture::facts::CheckCoverage;
use crate::config::{AppliesTo, RuleException, RuleKind, RuleSeverity};
use crate::{
    CodeRef, Completeness, GitHashAlgorithm, GitObjectId, Language, Producer, RepoPath,
    SchemaVersion, SourceSide, SourceSnapshotId,
};

fn code_ref(path: &str, line: u32) -> CodeRef {
    CodeRef {
        snapshot_id: SourceSnapshotId(sha256(b"s")),
        side: SourceSide::Head,
        path: path.parse::<RepoPath>().expect("a path"),
        blob_oid: GitObjectId::new(GitHashAlgorithm::Sha1, &"a".repeat(40)).expect("an oid"),
        start_line: line,
        end_line: line,
        content_sha256: sha256(b"x"),
    }
}

fn found(check: Check, key: &str, change: Option<Change>, path: &str) -> ArchitectureFinding {
    ArchitectureFinding {
        check,
        key: key.into(),
        message: format!("`{key}` is reported"),
        notes: vec!["src/a.rs:1:1: a note".into()],
        picture: vec!["    ┌─▶ a".into()],
        size: 3,
        change,
        base_size: (change == Some(Change::Worse)).then_some(2),
        code_ref: Some(code_ref(path, 7)),
    }
}

fn facts(findings: Vec<ArchitectureFinding>, partial: Option<Check>) -> ArchitectureFacts {
    ArchitectureFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: SourceSnapshotId(sha256(b"s")),
        producer: Producer {
            name: "rust-analyzer".into(),
            version: "1".into(),
            config_hash: None,
        },
        language: Language::Rust,
        project: String::new(),
        status: BackendStatus::Ready,
        base_checked: true,
        limits: Limits::default(),
        coverage: partial
            .map(|check| CheckCoverage {
                check,
                completeness: Completeness::Partial,
                reason: Some("macros are not expanded".into()),
            })
            .into_iter()
            .collect(),
        findings,
        fixed: Vec::new(),
        limitations: Vec::new(),
    }
}

fn rule(id: &str, exceptions: Vec<RuleException>) -> Rule {
    Rule {
        id: id.into(),
        version: 1,
        title: "no leaks".into(),
        kind: RuleKind::Invariant,
        required: true,
        severity_on_violation: Some(RuleSeverity::High),
        owner: "team".into(),
        applies_to: AppliesTo::default(),
        statement: "types guard their fields".into(),
        rationale: None,
        contract_refs: Vec::new(),
        acceptance: None,
        exceptions,
    }
}

fn now() -> Timestamp {
    "2026-11-24T00:00:00Z".parse().expect("a time")
}

#[test]
fn what_the_change_introduces_or_worsens_is_a_finding_and_existing_debt_is_not() {
    let project = facts(
        vec![
            found(
                Check::FieldLeak,
                "p::Q::items",
                Some(Change::New),
                "src/app.rs",
            ),
            found(
                Check::ModuleCycle,
                "p::a,p::b",
                Some(Change::Worse),
                "src/a.rs",
            ),
            found(
                Check::LongFile,
                "src/old.rs",
                Some(Change::Same),
                "src/old.rs",
            ),
            found(Check::Tangle, "p::A,p::B", Some(Change::Better), "src/b.rs"),
            found(Check::WideDirectory, "src/", None, "src/lib.rs"),
            found(Check::ShallowModule, "p::m", None, "src/m.rs"),
        ],
        Some(Check::ModuleCycle),
    );
    let changed = |p: &str| p == "src/lib.rs";
    let out = findings(&[project], &[], &changed, ReviewLanguage::En, now());
    let titles: Vec<&str> = out.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "A field is worked on outside its type",
            "Modules use each other in a circle",
            "A directory holds too many files"
        ]
    );
    assert!(
        out.iter()
            .all(|f| f.category == FindingCategory::Improvement && f.severity == Severity::Low)
    );
    assert!(
        out[0].explanation.contains("this change introduces it"),
        "{}",
        out[0].explanation
    );
    assert!(
        out[0]
            .explanation
            .ends_with(". src/a.rs:1:1: a note.\n\n```text\n    ┌─▶ a\n```"),
        "{}",
        out[0].explanation
    );
    assert!(
        out[1].explanation.contains("worse: 2 → 3 modules"),
        "{}",
        out[1].explanation
    );
    assert_eq!(out[1].limitations, ["macros are not expanded"]);
    assert!(out[2].limitations[0].contains("not checked"));
    let moved = facts(
        vec![ArchitectureFinding {
            code_ref: Some(code_ref("src/app.rs", 40)),
            ..found(
                Check::FieldLeak,
                "p::Q::items",
                Some(Change::New),
                "src/app.rs",
            )
        }],
        None,
    );
    let again = findings(&[moved], &[], &changed, ReviewLanguage::En, now());
    assert_eq!(
        again[0].fingerprint, out[0].fingerprint,
        "a line is no part of it"
    );
    assert_eq!(again[0].id, out[0].id);
}

#[test]
fn a_required_rule_makes_it_a_violation_that_an_exception_in_force_waives() {
    let project = || {
        facts(
            vec![found(
                Check::FieldLeak,
                "p::Q::items",
                Some(Change::New),
                "src/app.rs",
            )],
            None,
        )
    };
    let exception = |expires: &str| RuleException {
        paths: vec!["src/**".into()],
        reason: "being split".into(),
        owner: "team".into(),
        author: "a".into(),
        expires_at: Some(expires.parse().expect("a time")),
        until: None,
    };
    let none = |_: &str| false;
    let violation = findings(
        &[project()],
        &[rule("ARCH-FIELD-LEAK", Vec::new())],
        &none,
        ReviewLanguage::En,
        now(),
    );
    assert_eq!(violation[0].category, FindingCategory::RuleViolation);
    assert_eq!(violation[0].severity, Severity::High);
    assert_eq!(violation[0].rule_id.as_deref(), Some("ARCH-FIELD-LEAK"));
    let waived = findings(
        &[project()],
        &[rule(
            "ARCH-FIELD-LEAK",
            vec![exception("2027-01-01T00:00:00Z")],
        )],
        &none,
        ReviewLanguage::En,
        now(),
    );
    assert_eq!(waived[0].category, FindingCategory::Improvement);
    assert!(
        waived[0]
            .limitations
            .iter()
            .any(|l| l.contains("waived by team")),
        "{:?}",
        waived[0].limitations
    );
    let expired = findings(
        &[project()],
        &[rule(
            "ARCH-FIELD-LEAK",
            vec![exception("2026-01-01T00:00:00Z")],
        )],
        &none,
        ReviewLanguage::En,
        now(),
    );
    assert_eq!(expired[0].category, FindingCategory::RuleViolation);
    let other = findings(
        &[project()],
        &[rule("ARCH-TANGLE", Vec::new())],
        &none,
        ReviewLanguage::Ru,
        now(),
    );
    assert_eq!(other[0].category, FindingCategory::Improvement);
    assert_eq!(other[0].title, "Поле обрабатывается вне своего типа");
}
