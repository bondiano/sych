//! easement through a scripted stand-in on a real Git snapshot: where it runs, the environment
//! it gets, projects in subdirectories, and violations new on head outside the changed lines.

use sych_core::{BackendStatus, LintFacts};

mod common;

/// Answers like easement with one violation in `src/lib.rs` at the line a `violation-line`
/// file names (2 without one); the message reports the environment and arguments it saw.
const EASEMENT: &str = r#"
line=2
[ -f violation-line ] && read -r line < violation-line
printf '{"passed":false,"violations":[{"rule":"dependency","file":"src/lib.rs","line":%s,"message":"PATH=[%s] WRAPPER=%s OFFLINE=%s ARGS=[%s]","because":"stay inside","from":"a","to":"b","target":"t","specifier":"s"}],"warnings":[],"evidence_gaps":[]}' "$line" "$PATH" "$RUSTC_WRAPPER" "$CARGO_NET_OFFLINE" "$*"
exit 1
"#;

fn collect(base: &[(&str, &str)], head: &[(&str, &str)]) -> Vec<LintFacts> {
    // Every other pinned tool only has to exist: the stand-in never starts it.
    common::repo::collect_with(base, head, |tool| {
        if tool == "easement" {
            EASEMENT
        } else {
            "exit 0\n"
        }
    })
}

const CRATE: [(&str, &str); 2] = [
    ("Cargo.toml", "[package]\nname = \"a\"\n"),
    ("src/lib.rs", "mod a;\nmod b;\n"),
];
const CHANGE: [(&str, &str); 1] = [("src/lib.rs", "mod a;\nmod c;\n")];

fn with(files: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
    CRATE.into_iter().chain(files.iter().copied()).collect()
}

#[test]
fn easement_judges_changed_files_only_where_configured_with_pinned_indexers_alone() {
    assert!(collect(&CRATE, &CHANGE).is_empty());

    let facts = collect(&with(&[("easement.yml", "version: 1\n")]), &CHANGE);
    let [easement] = facts.as_slice() else {
        panic!("one document: {facts:?}");
    };
    assert_eq!(easement.producer.name, "easement");
    assert_eq!(easement.status, BackendStatus::Ready, "{easement:?}");
    assert_eq!(easement.files, ["src/lib.rs"]);
    assert_eq!(easement.config, ["easement.yml"]);
    assert!(
        easement.introduced.is_empty(),
        "the violation is on a changed line"
    );
    let [d] = easement.diagnostics.as_slice() else {
        panic!("one diagnostic: {:?}", easement.diagnostics);
    };
    assert_eq!(
        (d.path.as_str(), d.line, d.level.as_str()),
        ("src/lib.rs", 2, "error")
    );
    assert!(d.code_ref.is_some(), "line 2 exists on head");
    assert!(d.message.contains("WRAPPER=/nonexistent/"), "{}", d.message);
    assert!(
        d.message
            .contains("OFFLINE=true ARGS=[check --format json --no-cache]")
    );
    assert!(d.message.ends_with("; because: stay inside"));
    let path = d
        .message
        .split("PATH=[")
        .nth(1)
        .and_then(|r| r.split(']').next());
    assert!(
        path.is_some_and(|p| !p.contains(':') && p.ends_with("/head/bin")),
        "only the directory of pinned indexers: {path:?}"
    );
}

#[test]
fn a_project_in_a_subdirectory_is_its_own_producer_with_repository_paths() {
    let base = [
        ("svc/easement.yml", "version: 1\n"),
        ("svc/Cargo.toml", "[package]\nname = \"a\"\n"),
        ("svc/src/lib.rs", "mod a;\nmod b;\n"),
        ("docs/readme.md", "a\n"),
    ];
    let head = [
        ("svc/src/lib.rs", "mod a;\nmod c;\n"),
        ("docs/readme.md", "b\n"),
    ];
    let facts = collect(&base, &head);
    let [svc] = facts.as_slice() else {
        panic!("one project: {facts:?}");
    };
    assert_eq!(svc.producer.name, "easement:svc");
    assert_eq!(svc.files, ["svc/src/lib.rs"]);
    assert_eq!(svc.config, ["svc/easement.yml"]);
    let lines: Vec<(&str, u32)> = svc
        .diagnostics
        .iter()
        .map(|d| (d.path.as_str(), d.line))
        .collect();
    assert_eq!(lines, [("svc/src/lib.rs", 2)]);
}

#[test]
fn a_violation_at_an_unchanged_line_is_new_only_when_the_diff_base_lacks_it() {
    let unchanged = ("violation-line", "1\n");
    // The policy arrives with the change: the base has nothing to compare with.
    let facts = collect(
        &with(&[unchanged]),
        &[CHANGE[0], ("easement.yml", "version: 1\n")],
    );
    let [added] = facts.as_slice() else {
        panic!("one document: {facts:?}");
    };
    let new: Vec<(&str, u32)> = added
        .introduced
        .iter()
        .map(|d| (d.path.as_str(), d.line))
        .collect();
    assert_eq!(new, [("src/lib.rs", 1)]);
    assert!(added.introduced[0].code_ref.is_some());
    assert!(
        added.files.contains(&"easement.yml".to_owned()),
        "a changed policy file is judged too: {:?}",
        added.files
    );
    assert!(
        !added
            .limitations
            .iter()
            .any(|l| l.contains("no packet shows")),
        "the changed policy carries it: {:?}",
        added.limitations
    );

    // The base had the same violation: nothing is new.
    let facts = collect(
        &with(&[unchanged, ("easement.yml", "version: 1\n")]),
        &CHANGE,
    );
    let [kept] = facts.as_slice() else {
        panic!("one document: {facts:?}");
    };
    assert_eq!(kept.status, BackendStatus::Ready, "{kept:?}");
    assert!(kept.introduced.is_empty(), "{:?}", kept.introduced);
}
