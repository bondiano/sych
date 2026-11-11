//! easement, an architecture gate, on a head that has `easement.yml`: it indexes the Rust
//! workspace with the pinned rust-analyzer and reports violations on the changed Rust files.
//! `rust-analyzer scip` always builds build scripts and proc-macros, so cargo is given a
//! `RUSTC_WRAPPER` that cannot start: nothing from the repository is compiled or run.

use std::env;
use std::ffi::OsString;
use std::iter;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use sych_core::{
    BackendStatus, CiDiagnostic, LintFacts, Producer, RepoPath, SchemaVersion, SourceSide,
    canonical_sha256,
};

use super::{changed_config, run};
use crate::git::{LocalSnapshot, Materialized, SnapshotError};
use crate::semantic::{Resolve, toolchain};
use crate::tools::{self, ToolError, host_target};

const TOOL: &str = "easement";
const CONFIG: &str = "easement.yml";
/// Project files that decide what easement reports; a `baseline:` elsewhere is not tracked.
const CONFIGS: [&str; 2] = [CONFIG, "easement.baseline.yml"];
const ARGS: [&str; 4] = ["check", "--format", "json", "--no-cache"];
/// It indexes the whole workspace, not only the changed files.
const TIMEOUT: Duration = Duration::from_secs(600);
/// No file has this path, so cargo fails before any build script or proc-macro compiles.
const NO_RUSTC: &str = "/nonexistent/sych-compiles-no-repository-code";

/// Whether the head opts in with an `easement.yml` at its root.
pub(super) fn configured(snapshot: &LocalSnapshot) -> bool {
    CONFIG
        .parse::<RepoPath>()
        .is_ok_and(|p| snapshot.blob(SourceSide::Head, &p).is_ok())
}

/// One document for the changed Rust `files`: [`LintFacts::evidence_id`] is one per producer.
pub(super) fn collect(
    snapshot: &LocalSnapshot,
    files: &[&str],
    materialized: &Result<(PathBuf, Materialized), SnapshotError>,
    scratch: &Path,
    resolve: &Resolve<'_>,
) -> Vec<LintFacts> {
    let mut facts = new_facts(snapshot, files);
    let (root, m) = match materialized {
        Ok((root, m)) => (root, m),
        Err(err) => return vec![failed(facts, format!("workspace: {err}"))],
    };
    facts.files.retain(|f| root.join(f).is_file());
    if facts.files.len() < files.len() {
        facts.limitations.push(format!(
            "{} changed files not materialized (secret, too large or not text)",
            files.len() - facts.files.len()
        ));
    }

    if m.over_budget > 0 {
        facts.limitations.push(format!(
            "workspace budget spent: {} files not materialized, so imports of them are not judged",
            m.over_budget
        ));
    }

    facts.config = CONFIGS
        .iter()
        .filter(|c| root.join(c).is_file())
        .map(|c| (*c).to_owned())
        .collect();
    facts.limitations.extend(changed_config(snapshot, &CONFIGS));
    facts.limitations.push(
        "build scripts and proc-macros are not run: imports that generated or macro-expanded \
         code writes are not judged; only the Rust indexer is available"
            .into(),
    );
    let binaries = toolchain(root).and_then(|_| {
        let target = host_target();
        let easement = resolve(tools::sole_pinned(TOOL)?, &target)?;
        let analyzer = resolve(tools::sole_pinned("rust-analyzer")?, &target)?;
        Ok((easement, analyzer))
    });
    let (easement, analyzer) = match binaries {
        Ok(found) => found,
        Err(err) => {
            let mut facts = failed(facts, err.to_string());
            if matches!(err, ToolError::Unsupported(_)) {
                facts.status = BackendStatus::Unsupported;
            }

            return vec![facts];
        }
    };
    let args: Vec<String> = ARGS.iter().map(|a| (*a).to_owned()).collect();
    let out = match run::run(
        &easement,
        &args,
        root,
        scratch,
        &environment(&analyzer),
        TIMEOUT,
    ) {
        Ok(out) => out,
        Err(err) => return vec![failed(facts, format!("{TOOL}: {err}"))],
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    // 0 passes, 1 has violations; 2 is an error such as a broken config or a missing indexer.
    let parsed = match out.status.code() {
        Some(0 | 1) => parse(&stdout, &facts.files).map_err(|e| e.to_string()),
        _ => Err(out.status.to_string()),
    };
    let (diagnostics, gaps) = match parsed {
        Ok(parsed) => parsed,
        Err(err) => {
            let stderr: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(300)
                .collect();
            return vec![failed(facts, format!("{TOOL} {err}: {stderr}"))];
        }
    };
    facts.diagnostics = diagnostics
        .into_iter()
        .map(|mut d| {
            d.code_ref = d
                .path
                .parse()
                .ok()
                .and_then(|p| snapshot.code_ref(SourceSide::Head, &p, d.line, d.line).ok());
            d
        })
        .collect();
    facts.limitations.extend(gaps);
    vec![facts]
}

fn new_facts(snapshot: &LocalSnapshot, files: &[&str]) -> LintFacts {
    LintFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        side: SourceSide::Head,
        producer: Producer {
            name: TOOL.into(),
            version: tools::sole_pinned(TOOL)
                .map_or_else(|_| "unpinned".into(), |t| t.version.clone()),
            config_hash: canonical_sha256(&json!(ARGS)).ok(),
        },
        status: BackendStatus::Ready,
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        config: Vec::new(),
        diagnostics: Vec::new(),
        limitations: Vec::new(),
    }
}

fn failed(mut facts: LintFacts, why: String) -> LintFacts {
    facts.status = BackendStatus::Failed;
    facts.diagnostics.clear();
    facts.limitations.push(why);
    facts
}

/// The pinned rust-analyzer ahead of the host's cargo and rustc, offline, with the wrapper
/// that keeps cargo from compiling anything. The host `PATH` is not passed: another indexer
/// on it could run project code.
fn environment(analyzer: &Path) -> Vec<(&'static str, OsString)> {
    let host = env::var_os("PATH").unwrap_or_default();
    let dir_of = |program: &str| env::split_paths(&host).find(|d| d.join(program).is_file());
    let dirs = [
        analyzer.parent().map(Path::to_path_buf),
        dir_of("cargo"),
        dir_of("rustc"),
    ];
    let path = env::join_paths(dirs.into_iter().flatten()).unwrap_or_default();
    let home = |var: &str, rest: &str| {
        env::var_os(var).or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(rest).into()))
    };
    let mut vars = vec![
        ("PATH", path),
        ("RUSTC_WRAPPER", NO_RUSTC.into()),
        ("CARGO_NET_OFFLINE", "true".into()),
        ("RUSTUP_AUTO_INSTALL", "0".into()),
    ];
    vars.extend(home("CARGO_HOME", ".cargo").map(|v| ("CARGO_HOME", v)));
    vars.extend(home("RUSTUP_HOME", ".rustup").map(|v| ("RUSTUP_HOME", v)));
    vars
}

#[derive(Deserialize)]
struct Report {
    violations: Vec<Violation>,
    /// Violations of rules in `warn`.
    #[serde(default)]
    warnings: Vec<Violation>,
    #[serde(default)]
    evidence_gaps: Vec<Gap>,
}

#[derive(Deserialize)]
struct Violation {
    rule: String,
    file: String,
    line: u32,
    message: String,
    because: Option<String>,
    /// Every import behind the violation, e.g. each edge of a cycle.
    #[serde(default)]
    evidence: Vec<Place>,
}

#[derive(Deserialize)]
struct Place {
    file: String,
    line: u32,
}

#[derive(Deserialize)]
struct Gap {
    #[serde(default)]
    files: Vec<String>,
    /// Set when the gap holds for a whole language.
    #[serde(default)]
    extensions: Vec<String>,
    message: String,
}

/// Diagnostics at the places in `files` behind each violation, and each evidence gap's
/// message naming the `files` it leaves partly unjudged.
fn parse(
    text: &str,
    files: &[String],
) -> Result<(Vec<CiDiagnostic>, Vec<String>), serde_json::Error> {
    let report: Report = serde_json::from_str(text)?;
    let levels = report
        .violations
        .iter()
        .map(|v| ("error", v))
        .chain(report.warnings.iter().map(|v| ("warning", v)));
    let mut diagnostics = Vec::new();
    for (level, v) in levels {
        let message = match &v.because {
            Some(because) => format!("{}; because: {because}", v.message),
            None => v.message.clone(),
        };
        let places = iter::once((v.file.as_str(), v.line))
            .chain(v.evidence.iter().map(|p| (p.file.as_str(), p.line)));
        for (path, line) in places {
            let seen = diagnostics.iter().any(|d: &CiDiagnostic| {
                d.path == path
                    && d.line == line
                    && d.message == message
                    && d.code.as_deref() == Some(v.rule.as_str())
            });
            if files.iter().any(|f| f == path) && !seen {
                diagnostics.push(CiDiagnostic {
                    code_ref: None,
                    path: path.to_owned(),
                    line,
                    level: level.into(),
                    code: Some(v.rule.clone()),
                    message: message.clone(),
                });
            }
        }
    }

    let gaps = report
        .evidence_gaps
        .into_iter()
        .map(|g| {
            let hit: Vec<&str> = files
                .iter()
                .filter(|f| {
                    g.files.contains(f)
                        || g.extensions.iter().any(|e| f.ends_with(&format!(".{e}")))
                })
                .map(String::as_str)
                .collect();
            if hit.is_empty() {
                g.message
            } else {
                format!("{} (changed files: {})", g.message, hit.join(", "))
            }
        })
        .collect();
    Ok((diagnostics, gaps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violations_and_gaps_on_changed_files_are_kept() {
        let report = r#"{
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
        let files: Vec<String> = ["src/handlers/books.rs", "src/b.rs", "src/macros.rs"]
            .map(String::from)
            .into();
        let (diagnostics, gaps) = parse(report, &files).expect("parses");
        let seen: Vec<(&str, u32, &str, Option<&str>)> = diagnostics
            .iter()
            .map(|d| (d.path.as_str(), d.line, d.level.as_str(), d.code.as_deref()))
            .collect();
        assert_eq!(
            seen,
            [
                // One per rule at the same place and message.
                ("src/handlers/books.rs", 1, "error", Some("dependency")),
                ("src/handlers/books.rs", 1, "error", Some("privacy")),
                // The changed edge of a cycle reported at an unchanged file.
                ("src/b.rs", 7, "error", Some("cycle")),
            ]
        );
        assert_eq!(
            diagnostics[0].message,
            "handlers -> repository is not allowed; because: go through services"
        );
        assert_eq!(
            gaps,
            [
                "1 import not resolved (changed files: src/macros.rs)",
                "ts not covered"
            ]
        );
        assert!(parse("easement: error", &files).is_err());
    }

    #[test]
    fn cargo_gets_a_wrapper_that_cannot_start_and_no_host_path() {
        let vars = environment(Path::new(
            "/tools/rust-analyzer/1/x86_64-linux/rust-analyzer",
        ));
        let get = |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert!(!Path::new(NO_RUSTC).exists());
        assert_eq!(get("RUSTC_WRAPPER"), Some(NO_RUSTC.into()));
        assert_eq!(get("CARGO_NET_OFFLINE"), Some("true".into()));
        let path = get("PATH").expect("PATH");
        let first = env::split_paths(&path).next();
        assert_eq!(
            first.as_deref(),
            Some(Path::new("/tools/rust-analyzer/1/x86_64-linux"))
        );
    }
}
