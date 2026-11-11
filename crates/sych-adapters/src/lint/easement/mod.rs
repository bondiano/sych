//! easement, an architecture gate, on each project an `easement.yml` roots on head: it indexes
//! Rust, Go, TypeScript/JavaScript and Python with pinned indexers and reports the violations
//! at the changed files, and, against the diff base, the violations new on head that no changed
//! line shows (a changed policy, a moved file).

mod env;
mod report;

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use sych_core::{
    BackendStatus, ChangeStatus, CiDiagnostic, Language, LintFacts, Producer, RepoPath,
    SchemaVersion, SourceSide, canonical_sha256,
};
use tempfile::TempDir;

use super::{MAX_WORKSPACE_BLOB_BYTES, changed_config, run};
use crate::git::{LocalSnapshot, Materialized, SnapshotError};
use crate::semantic::{Options, deps, toolchain};
use crate::tools::{self, ToolError, host_target};
use report::{Key, Report};

const TOOL: &str = "easement";
const CONFIG: &str = "easement.yml";
const ARGS: [&str; 4] = ["check", "--format", "json", "--no-cache"];
/// Each run indexes a whole project, not only the changed files.
const TIMEOUT: Duration = Duration::from_secs(600);
/// Violations a limitation lists one by one when no packet can carry them.
const MAX_LISTED: usize = 20;
/// New violations kept as facts; a policy added over old code can have thousands.
const MAX_INTRODUCED: usize = 200;

pub(super) type Workspace = Result<(PathBuf, Materialized), SnapshotError>;

/// Why a run gave no report, and the status that makes it.
type Failure = (BackendStatus, String);

/// Directories whose `easement.yml` on head roots a project this diff touches: a changed
/// source or manifest under it, or a changed file of its policy. Root first.
pub(super) fn projects(snapshot: &LocalSnapshot) -> Vec<String> {
    let changed = changed_paths(snapshot);
    let mut dirs = BTreeSet::from([String::new()]);
    for path in &changed {
        let mut rest = path.as_str();
        while let Some((parent, _)) = rest.rsplit_once('/') {
            dirs.insert(parent.to_owned());
            rest = parent;
        }
    }

    dirs.into_iter()
        .filter(|dir| {
            policy(snapshot, dir).is_some_and(|configs| {
                changed
                    .iter()
                    .any(|p| configs.contains(p) || (under(p, dir) && env::language(p).is_some()))
            })
        })
        .collect()
}

/// One document per project, each its own producer: [`LintFacts::evidence_id`] is one per
/// producer.
pub(super) fn collect(
    snapshot: &LocalSnapshot,
    projects: &[String],
    head: &Workspace,
    scratch: &Path,
    options: &Options<'_>,
) -> Vec<LintFacts> {
    let base = OnceCell::new();
    projects
        .iter()
        .enumerate()
        .map(|(i, dir)| {
            let scratch = scratch.join(format!("easement-{i}"));
            let facts = project(snapshot, dir, head, &base, &scratch, options);
            tracing::info!(project = %dir, status = ?facts.status, diagnostics = facts.diagnostics.len(), introduced = facts.introduced.len(), "easement facts collected");
            facts
        })
        .collect()
}

fn project(
    snapshot: &LocalSnapshot,
    dir: &str,
    head: &Workspace,
    base: &OnceCell<(Option<TempDir>, Workspace)>,
    scratch: &Path,
    options: &Options<'_>,
) -> LintFacts {
    let configs = policy(snapshot, dir).unwrap_or_default();
    let files: Vec<String> = changed_paths_on(snapshot, SourceSide::Head)
        .filter(|p| under(p, dir) && (env::language(p).is_some() || configs.contains(p)))
        .collect();
    let mut facts = new_facts(snapshot, dir, files);
    let declared: Vec<&str> = configs.iter().map(String::as_str).collect();
    facts
        .limitations
        .extend(changed_config(snapshot, &declared));
    let (root, m) = match head {
        Ok((root, m)) => (root, m),
        Err(err) => return failed(facts, (BackendStatus::Failed, format!("workspace: {err}"))),
    };
    facts.config = configs
        .iter()
        .filter(|c| root.join(c).is_file())
        .cloned()
        .collect();
    let listed = facts.files.len();
    facts.files.retain(|f| root.join(f).is_file());
    if facts.files.len() < listed {
        facts.limitations.push(format!(
            "{} changed files not materialized (secret, too large or not text)",
            listed - facts.files.len()
        ));
    }

    if m.over_budget > 0 {
        facts.limitations.push(format!(
            "workspace budget spent: {} files not materialized, so imports of them are not judged",
            m.over_budget
        ));
    }

    let checked = check(
        root,
        m,
        dir,
        &scratch.join("head"),
        options,
        &mut facts.limitations,
    );
    let report = match checked {
        Ok(report) => report,
        Err(failure) => return failed(facts, failure),
    };
    facts.diagnostics = report
        .diagnostics(&facts.files)
        .into_iter()
        .map(|d| anchored(snapshot, d))
        .collect();
    facts.limitations.extend(report.gaps(&facts.files));
    introduced(snapshot, dir, &report, base, scratch, options, &mut facts);
    facts
}

/// Violations on head that no changed line shows and the diff base does not have. The base
/// is checked only when there are such candidates, and not at all when it has no policy.
fn introduced(
    snapshot: &LocalSnapshot,
    dir: &str,
    report: &Report,
    base: &OnceCell<(Option<TempDir>, Workspace)>,
    scratch: &Path,
    options: &Options<'_>,
    facts: &mut LintFacts,
) {
    let outside: Vec<_> = report
        .all()
        .filter(|(_, v)| {
            !v.places()
                .any(|(path, line)| on_changed_line(snapshot, path, line))
        })
        .collect();
    if outside.is_empty() {
        return;
    }

    let before: BTreeSet<Key> = if policy_on(snapshot, SourceSide::DiffBase, dir).is_none() {
        BTreeSet::new()
    } else {
        let (_, workspace) = base.get_or_init(|| materialize(snapshot, SourceSide::DiffBase));
        let checked = match workspace {
            // What the base run costs is already said for head.
            Ok((root, m)) => check(
                root,
                m,
                dir,
                &scratch.join("base"),
                options,
                &mut Vec::new(),
            ),
            Err(err) => Err((BackendStatus::Failed, format!("workspace: {err}"))),
        };
        match checked {
            Ok(base) => base
                .all()
                .map(|(_, v)| v.key(&head_path(snapshot, &v.file)))
                .collect(),
            Err((_, why)) => {
                facts.limitations.push(format!(
                    "the diff base was not checked ({why}): {} violations outside the changed \
                     lines are not told apart from those it already had",
                    outside.len()
                ));
                return;
            }
        }
    };
    let changed = changed_paths(snapshot);
    let policy_changed = facts.config.iter().any(|c| changed.contains(c));
    let new: Vec<_> = outside
        .into_iter()
        .filter(|(_, v)| !before.contains(&v.key(&v.file)))
        .collect();
    if new.len() > MAX_INTRODUCED {
        facts.limitations.push(format!(
            "{} violations are new on head outside the changed lines; the first {MAX_INTRODUCED} are kept",
            new.len()
        ));
    }

    let mut unplaced = Vec::new();
    for (level, v) in new.into_iter().take(MAX_INTRODUCED) {
        let d = anchored(snapshot, v.diagnostic(level, &v.file, v.line));
        if !policy_changed && !changed.contains(&v.file) {
            unplaced.push(format!("{}:{} {}", d.path, d.line, d.message));
        }

        facts.introduced.push(d);
    }
    if !unplaced.is_empty() {
        let shown: Vec<&str> = unplaced
            .iter()
            .take(MAX_LISTED)
            .map(String::as_str)
            .collect();
        facts.limitations.push(format!(
            "{} violations new on head sit in unchanged files and no policy file changed, so no \
             packet shows them: {}",
            unplaced.len(),
            shown.join("; ")
        ));
    }
}

/// easement's report on the project in `dir` of the workspace at `root`; what it costs goes
/// to `limitations` whether or not it ran.
fn check(
    root: &Path,
    m: &Materialized,
    dir: &str,
    scratch: &Path,
    options: &Options<'_>,
    limitations: &mut Vec<String>,
) -> Result<Report, Failure> {
    let project = root.join(dir);
    let tool = |e: ToolError| {
        let status = match e {
            ToolError::Unsupported(_) => BackendStatus::Unsupported,
            _ => BackendStatus::Failed,
        };
        (status, e.to_string())
    };
    let paths = || m.blobs.keys().map(RepoPath::as_str);
    let languages: BTreeSet<Language> = paths()
        .filter(|p| under(p, dir))
        .filter_map(env::language)
        .collect();
    if languages.contains(&Language::Rust) {
        // rustup reads a `rust-toolchain` in any directory above the one cargo runs in.
        for file in paths().filter(|p| matches!(name(p), "rust-toolchain" | "rust-toolchain.toml"))
        {
            toolchain(&root.join(parent(file))).map_err(tool)?;
        }

        limitations.push(
            "build scripts and proc-macros are not run: imports that generated or \
             macro-expanded code writes are not judged"
                .into(),
        );
    }

    if languages.contains(&Language::Python) {
        limitations.push(
            "packages of a Python environment are not resolved: scip-python gets an empty \
             environment and never runs pip or python"
                .into(),
        );
    }

    let installed = install(root, m, dir, &languages, options, limitations)?;
    let easement =
        (options.resolve)(tools::sole_pinned(TOOL).map_err(tool)?, &host_target()).map_err(tool)?;
    let mut vars = env::indexers(
        &scratch.join("bin"),
        &languages,
        options.resolve,
        options.state_dir,
    )
    .map_err(tool)?;
    // Later entries win: an installed `CARGO_HOME` replaces the host's.
    vars.extend(installed);
    let args: Vec<String> = ARGS.map(String::from).into();
    let out = run::run(&easement, &args, &project, scratch, &vars, TIMEOUT)
        .map_err(|e| (BackendStatus::Failed, format!("{TOOL}: {e}")))?;
    let stderr = || -> String {
        String::from_utf8_lossy(&out.stderr)
            .chars()
            .take(300)
            .collect()
    };
    // 0 passes, 1 has violations; 2 is an error such as a broken config or a missing indexer.
    match out.status.code() {
        Some(0 | 1) => Report::read(&String::from_utf8_lossy(&out.stdout), dir)
            .map_err(|e| (BackendStatus::Failed, format!("{TOOL} {e}: {}", stderr()))),
        _ => Err((
            BackendStatus::Failed,
            format!("{TOOL} {}: {}", out.status, stderr()),
        )),
    }
}

/// Dependencies of the project in `dir`, and the environment the installs leave. easement
/// indexes every Cargo and pnpm workspace in it, so each lockfile's directory gets its own;
/// `go mod download` finds the Go modules itself.
fn install(
    root: &Path,
    m: &Materialized,
    dir: &str,
    languages: &BTreeSet<Language>,
    options: &Options<'_>,
    limitations: &mut Vec<String>,
) -> Result<Vec<(String, OsString)>, Failure> {
    let lock_dirs = |lock: &str| -> BTreeSet<&str> {
        m.blobs
            .keys()
            .map(RepoPath::as_str)
            .filter(|p| under(p, dir) && name(p) == lock)
            .map(parent)
            .collect()
    };
    let mut installs: Vec<(Language, &str)> = Vec::new();
    for (language, lock) in [
        (Language::Rust, "Cargo.lock"),
        (Language::Typescript, "pnpm-lock.yaml"),
    ] {
        if languages.contains(&language) {
            installs.extend(lock_dirs(lock).into_iter().map(|d| (language, d)));
        }
    }

    if languages.contains(&Language::Go) {
        installs.push((Language::Go, dir));
    }

    let mut installed: Vec<(String, OsString)> = Vec::new();
    for (language, at) in installs {
        let prepared = deps::prepare(
            language,
            &root.join(at),
            options.tooling,
            options.resolve,
            options.state_dir,
        )
        .map_err(|e| {
            (
                BackendStatus::Failed,
                format!("dependencies in {at:?}: {e}"),
            )
        })?;
        limitations.extend(prepared.limitation);
        installed.extend(prepared.env.into_iter().map(|(k, v)| (k, v.into())));
    }

    if !options.tooling.dependency_install {
        limitations.push(
            "dependencies are not installed (tooling.dependency_install is off): imports of \
             third-party packages are not resolved, and a package.json that declares \
             dependencies stops easement"
                .into(),
        );
    }

    Ok(installed)
}

/// The repository paths of the `easement.yml` in `dir` on head and the files it reads.
fn policy(snapshot: &LocalSnapshot, dir: &str) -> Option<BTreeSet<String>> {
    policy_on(snapshot, SourceSide::Head, dir)
}

fn policy_on(snapshot: &LocalSnapshot, side: SourceSide, dir: &str) -> Option<BTreeSet<String>> {
    let config = if dir.is_empty() {
        CONFIG.to_owned()
    } else {
        format!("{dir}/{CONFIG}")
    };
    let path: RepoPath = config.parse().ok()?;
    let (_, text) = snapshot.blob(side, &path).ok()?;
    Some(report::config_paths(dir, &config, &text))
}

fn materialize(snapshot: &LocalSnapshot, side: SourceSide) -> (Option<TempDir>, Workspace) {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(e) => {
            return (
                None,
                Err(SnapshotError::Git(format!("workspace directory: {e}"))),
            );
        }
    };
    let workspace = fs::canonicalize(dir.path())
        .map_err(|e| SnapshotError::Git(format!("workspace directory: {e}")))
        .and_then(|root| {
            snapshot
                .materialize(side, &root, MAX_WORKSPACE_BLOB_BYTES)
                .map(|m| (root, m))
        });
    (Some(dir), workspace)
}

/// Every path a change names, on either side.
fn changed_paths(snapshot: &LocalSnapshot) -> BTreeSet<String> {
    snapshot
        .manifest
        .files
        .iter()
        .flat_map(|f| f.new.iter().chain(&f.old))
        .map(|s| s.path.as_str().to_owned())
        .collect()
}

fn changed_paths_on(snapshot: &LocalSnapshot, side: SourceSide) -> impl Iterator<Item = String> {
    snapshot
        .manifest
        .files
        .iter()
        .filter(|f| f.status != ChangeStatus::Deleted)
        .filter_map(move |f| f.side(side))
        .map(|s| s.path.as_str().to_owned())
}

fn on_changed_line(snapshot: &LocalSnapshot, path: &str, line: u32) -> bool {
    snapshot
        .manifest
        .files
        .iter()
        .filter(|f| {
            f.side(SourceSide::Head)
                .is_some_and(|s| s.path.as_str() == path)
        })
        .any(|f| {
            f.changed_lines(SourceSide::Head)
                .iter()
                .any(|&(s, e)| (s..=e).contains(&line))
        })
}

/// Where a diff-base path is on head: a renamed file keeps its violations' identity.
fn head_path(snapshot: &LocalSnapshot, base: &str) -> String {
    snapshot
        .manifest
        .files
        .iter()
        .find(|f| f.old.as_ref().is_some_and(|s| s.path.as_str() == base))
        .and_then(|f| f.new.as_ref())
        .map_or_else(|| base.to_owned(), |s| s.path.as_str().to_owned())
}

fn anchored(snapshot: &LocalSnapshot, mut d: CiDiagnostic) -> CiDiagnostic {
    d.code_ref = d
        .path
        .parse()
        .ok()
        .and_then(|p| snapshot.code_ref(SourceSide::Head, &p, d.line, d.line).ok());
    d
}

fn under(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn new_facts(snapshot: &LocalSnapshot, dir: &str, files: Vec<String>) -> LintFacts {
    LintFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        side: SourceSide::Head,
        producer: Producer {
            name: if dir.is_empty() {
                TOOL.into()
            } else {
                format!("{TOOL}:{dir}")
            },
            version: tools::sole_pinned(TOOL)
                .map_or_else(|_| "unpinned".into(), |t| t.version.clone()),
            config_hash: canonical_sha256(&json!(ARGS)).ok(),
        },
        status: BackendStatus::Ready,
        files,
        config: Vec::new(),
        diagnostics: Vec::new(),
        introduced: Vec::new(),
        limitations: Vec::new(),
    }
}

fn failed(mut facts: LintFacts, (status, why): Failure) -> LintFacts {
    facts.status = status;
    facts.diagnostics.clear();
    facts.introduced.clear();
    facts.limitations.push(why);
    facts
}
