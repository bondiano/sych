use super::*;

const REPORT: &str = r#"{
  "passed": false,
  "violations": [
    {"rule": "dependency", "file": "src/handlers/books.rs", "line": 1,
     "message": "handlers -> repository is not allowed", "because": "go through services",
     "from": "handlers", "to": "repository", "target": "src/repository/store.rs",
     "specifier": "src/repository/store.rs",
     "evidence": [{"file": "src/handlers/books.rs", "line": 1}]},
    {"rule": "privacy", "file": "src/handlers/books.rs", "line": 1,
     "message": "handlers -> repository is not allowed", "because": "go through services",
     "from": "handlers", "to": "repository", "target": "src/repository/store.rs",
     "specifier": "src/repository/store.rs"},
    {"rule": "cycle", "file": "src/a.rs", "line": 3, "message": "a -> b -> a",
     "from": "a", "to": "b", "target": "src/b.rs", "specifier": "b",
     "evidence": [{"file": "src/a.rs", "line": 3}, {"file": "src/b.rs", "line": 7}]}
  ],
  "warnings": [
    {"rule": "dependency", "file": "src/old.rs", "line": 2, "message": "unchanged file",
     "from": "x", "to": "y", "target": "t", "specifier": "s"}
  ],
  "stale": [], "legalized": 0, "narrowed": [],
  "evidence_gaps": [
    {"kind": "unresolved_imports", "files": ["src/macros.rs"], "count": 1,
     "message": "1 import not resolved", "close": "index it"},
    {"kind": "uncovered", "extensions": ["ts"], "message": "ts not covered", "close": "x"}
  ],
  "accepted_gaps": [], "stats": {}
}"#;

#[test]
fn violations_and_gaps_land_on_the_changed_files_of_a_rebased_project() {
    let files: Vec<String> = [
        "svc/src/handlers/books.rs",
        "svc/src/b.rs",
        "svc/src/macros.rs",
    ]
    .map(String::from)
    .into();
    let report = Report::read(REPORT, "svc").expect("parses");
    let diagnostics = report.diagnostics(&files);
    let seen: Vec<(&str, u32, &str, Option<&str>)> = diagnostics
        .iter()
        .map(|d| (d.path.as_str(), d.line, d.level.as_str(), d.code.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            // One per rule at the same place and message.
            ("svc/src/handlers/books.rs", 1, "error", Some("dependency")),
            ("svc/src/handlers/books.rs", 1, "error", Some("privacy")),
            // The changed edge of a cycle reported at an unchanged file.
            ("svc/src/b.rs", 7, "error", Some("cycle")),
        ]
    );
    assert_eq!(
        diagnostics[0].message,
        "handlers -> repository is not allowed; because: go through services"
    );
    assert_eq!(
        report.gaps(&files),
        [
            "1 import not resolved (changed files: svc/src/macros.rs)",
            "ts not covered"
        ]
    );
    let keys: Vec<Key> = report.all().map(|(_, v)| v.key(&v.file)).collect();
    assert_eq!(keys[0].4, "svc/src/handlers/books.rs");
    assert_ne!(keys[0], keys[1], "the rule is part of the key");
    assert!(Report::read("easement: error", "").is_err());
}

#[test]
fn a_config_names_the_files_it_extends_its_baseline_and_its_diagram() {
    let text = "version: 1\nextends: [../shared/base.yml, ./more.yml]\ndiagram: docs/arch.md\n";
    let paths: Vec<String> = config_paths("svc", "svc/easement.yml", text)
        .into_iter()
        .collect();
    assert_eq!(
        paths,
        [
            "shared/base.yml",
            "svc/docs/arch.md",
            "svc/easement.baseline.yml",
            "svc/easement.yml",
            "svc/more.yml"
        ]
    );
    let own = config_paths(
        "",
        "easement.yml",
        "baseline: debt.yml\nextends: ../../x.yml\n",
    );
    assert_eq!(
        own.into_iter().collect::<Vec<_>>(),
        ["debt.yml", "easement.yml"]
    );
    let broken = config_paths("", "easement.yml", ": not yaml [");
    assert!(broken.contains("easement.baseline.yml") && broken.contains("easement.yml"));
}
