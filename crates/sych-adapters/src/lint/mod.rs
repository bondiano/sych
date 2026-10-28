//! Pinned linters on the head's changed files (spec §10.6): ruff for Python, oxlint for
//! JS/TS (also where no TypeScript backend applies), sqruff for SQL, mago for PHP, jarl for R,
//! clj-kondo for Clojure, actionlint and zizmor for GitHub workflows. Each is a single verified binary that reads source and its own config but never
//! executes repository code: it runs on a materialized head with an empty `PATH` and `HOME`,
//! no network options, and a time bound.

mod run;

use std::path::Path;
use std::{fs, io};

use serde_json::json;
use sych_core::{
    BackendStatus, ChangeStatus, CiDiagnostic, CiFacts, CiFormat, LintFacts, Producer,
    SchemaVersion, SourceSide, SourceSnapshotId, canonical_sha256, sha256,
};

use crate::ci;
use crate::git::{LocalSnapshot, SnapshotError};
use crate::semantic::Resolve;
use crate::tools::{self, ToolError, host_target};

const MAX_WORKSPACE_BLOB_BYTES: u64 = 4 << 20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Sarif,
    /// `actionlint -format '{{json .}}'`.
    ActionlintJson,
}

struct Linter {
    tool: &'static str,
    selects: fn(&str) -> bool,
    /// Project files the linter reads its configuration from.
    configs: &'static [&'static str],
    format: Format,
    args: &'static [&'static str],
}

fn ext(path: &str, wanted: &[&str]) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, e)| wanted.contains(&e))
}

fn workflow(path: &str) -> bool {
    path.starts_with(".github/workflows/") && ext(path, &["yml", "yaml"])
}

fn action_or_workflow(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    workflow(path)
        || matches!(name, "action.yml" | "action.yaml")
        || matches!(path, ".github/dependabot.yml" | ".github/dependabot.yaml")
}

const LINTERS: [Linter; 8] = [
    Linter {
        tool: "ruff",
        selects: |p| ext(p, &["py", "pyi"]),
        configs: &["pyproject.toml", "ruff.toml", ".ruff.toml"],
        format: Format::Sarif,
        args: &[
            "check",
            "--output-format",
            "sarif",
            "--no-cache",
            "--exit-zero",
            "--force-exclude",
        ],
    },
    Linter {
        tool: "oxlint",
        selects: |p| ext(p, &["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"]),
        configs: &[".oxlintrc.json"],
        format: Format::Sarif,
        args: &["--format", "sarif"],
    },
    Linter {
        tool: "sqruff",
        selects: |p| ext(p, &["sql"]),
        configs: &[".sqruff", ".sqlfluff", "pyproject.toml"],
        format: Format::Sarif,
        args: &["lint", "--format", "sarif"],
    },
    Linter {
        tool: "mago",
        selects: |p| ext(p, &["php"]),
        configs: &[
            "mago.toml",
            "mago.yaml",
            "mago.yml",
            "mago.json",
            "mago.dist.toml",
            "mago.dist.yaml",
            "mago.dist.yml",
            "mago.dist.json",
        ],
        format: Format::Sarif,
        // Extension hosts in the project config are unsandboxed processes; `lint` is syntax
        // and style only, `analyze` would need the project's `vendor/`.
        args: &[
            "--no-extensions",
            "--no-version-check",
            "lint",
            "--reporting-format",
            "sarif",
        ],
    },
    Linter {
        tool: "jarl",
        selects: |p| ext(p, &["r", "R"]),
        configs: &["jarl.toml", ".jarl.toml"],
        format: Format::Sarif,
        // Package-specific rules would start `Rscript` from the project root; with an empty
        // `PATH` jarl finds no R and skips them.
        args: &["check", "--output-format", "sarif"],
    },
    Linter {
        tool: "clj-kondo",
        selects: |p| ext(p, &["clj", "cljs", "cljc"]),
        // The project's `.clj-kondo` is never read: its hooks run repository Clojure.
        configs: &[],
        format: Format::Sarif,
        // `--config-dir` (an empty Sych-owned directory) and `--lint` are added at run time.
        args: &[
            "--repro",
            "--cache",
            "false",
            "--config",
            "{:output {:format :sarif}}",
        ],
    },
    Linter {
        tool: "actionlint",
        selects: workflow,
        configs: &[".github/actionlint.yaml", ".github/actionlint.yml"],
        format: Format::ActionlintJson,
        // Without these it would run shellcheck and pyflakes found on `PATH`.
        args: &[
            "-no-color",
            "-shellcheck=",
            "-pyflakes=",
            "-format",
            "{{json .}}",
        ],
    },
    Linter {
        tool: "zizmor",
        selects: action_or_workflow,
        configs: &["zizmor.yml", ".github/zizmor.yml"],
        format: Format::Sarif,
        args: &["--offline", "--format", "sarif"],
    },
];

/// One document per linter with changed files it reads; tool failures are reported inside.
#[tracing::instrument(level = "info", name = "lint", skip_all)]
pub fn collect(snapshot: &LocalSnapshot, resolve: &Resolve<'_>) -> Vec<LintFacts> {
    let changed: Vec<&str> = snapshot
        .manifest
        .files
        .iter()
        .filter(|f| f.status != ChangeStatus::Deleted)
        .filter_map(|f| f.new.as_ref())
        .map(|s| s.path.as_str())
        .collect();
    let wanted: Vec<(&Linter, Vec<&str>)> = LINTERS
        .iter()
        .map(|l| {
            (
                l,
                changed.iter().copied().filter(|p| (l.selects)(p)).collect(),
            )
        })
        .filter(|(_, files): &(_, Vec<&str>)| !files.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }

    let (workspace, scratch) = match (tempfile::tempdir(), tempfile::tempdir()) {
        (Ok(w), Ok(s)) => (w, s),
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!(%e, "lint skipped: no temporary directory");
            return Vec::new();
        }
    };
    // External tools report canonical paths, including macOS /var -> /private/var.
    let materialized = fs::canonicalize(workspace.path())
        .map_err(|err| SnapshotError::Git(format!("workspace directory: {err}")))
        .and_then(|root| {
            snapshot
                .materialize(SourceSide::Head, &root, MAX_WORKSPACE_BLOB_BYTES)
                .map(|materialized| (root, materialized))
        });
    wanted
        .into_iter()
        .map(|(linter, files)| {
            let mut facts = lint_facts(snapshot, linter, &files);
            match &materialized {
                Ok((root, m)) => {
                    // Skipped secret, oversized or non-UTF-8 files are not on disk.
                    let present: Vec<&str> = files
                        .iter()
                        .copied()
                        .filter(|f| root.join(f).is_file())
                        .collect();
                    if present.len() < files.len() {
                        facts.limitations.push(format!(
                            "{} changed files not materialized (secret, too large or not text)",
                            files.len() - present.len()
                        ));
                    }

                    if m.over_budget > 0 {
                        facts.limitations.push(format!(
                            "workspace budget spent: {} files not materialized",
                            m.over_budget
                        ));
                    }

                    run_linter(
                        snapshot,
                        linter,
                        &present,
                        root,
                        scratch.path(),
                        resolve,
                        &mut facts,
                    );
                }
                Err(err) => {
                    facts.status = BackendStatus::Failed;
                    facts.limitations.push(format!("workspace: {err}"));
                }
            }
            tracing::info!(linter = linter.tool, status = ?facts.status, diagnostics = facts.diagnostics.len(), "lint facts collected");
            facts
        })
        .collect()
}

fn lint_facts(snapshot: &LocalSnapshot, linter: &Linter, files: &[&str]) -> LintFacts {
    let version =
        tools::sole_pinned(linter.tool).map_or_else(|_| "unpinned".into(), |t| t.version.clone());
    LintFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        side: SourceSide::Head,
        producer: Producer {
            name: linter.tool.into(),
            version,
            config_hash: canonical_sha256(&json!(linter.args)).ok(),
        },
        status: BackendStatus::Ready,
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        config: Vec::new(),
        diagnostics: Vec::new(),
        limitations: Vec::new(),
    }
}

fn run_linter(
    snapshot: &LocalSnapshot,
    linter: &Linter,
    files: &[&str],
    root: &Path,
    scratch: &Path,
    resolve: &Resolve<'_>,
    facts: &mut LintFacts,
) {
    if files.is_empty() {
        return;
    }

    let binary = match tools::sole_pinned(linter.tool).and_then(|t| resolve(t, &host_target())) {
        Ok(binary) => binary,
        Err(err) => {
            facts.status = match err {
                ToolError::Unsupported(_) => BackendStatus::Unsupported,
                _ => BackendStatus::Failed,
            };
            facts.limitations.push(err.to_string());
            return;
        }
    };
    facts.config = linter
        .configs
        .iter()
        .filter(|c| root.join(c).is_file())
        .map(|c| (*c).to_owned())
        .collect();
    let changed_config: Vec<&String> = facts
        .config
        .iter()
        .filter(|c| {
            snapshot.manifest.files.iter().any(|f| {
                f.new
                    .iter()
                    .chain(&f.old)
                    .any(|s| s.path.as_str() == c.as_str())
            })
        })
        .collect();
    if !changed_config.is_empty() {
        facts.limitations.push(format!(
            "linter configuration changed in this diff ({}): what it reports is the author's selection",
            changed_config.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }

    let mut args: Vec<String> = linter.args.iter().map(|a| (*a).to_owned()).collect();
    if let Err(err) = isolate(
        linter.tool,
        root,
        scratch,
        &mut args,
        &mut facts.limitations,
    ) {
        facts.status = BackendStatus::Failed;
        facts.limitations.push(format!("{}: {err}", linter.tool));
        return;
    }

    args.extend(files.iter().map(|f| (*f).to_owned()));
    let out = match run::run(&binary, &args, root, scratch) {
        Ok(out) => out,
        Err(err) => {
            facts.status = BackendStatus::Failed;
            facts.limitations.push(format!("{}: {err}", linter.tool));
            return;
        }
    };
    // Absolute locations become repository paths before parsing.
    let root_text = root.display().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout)
        .replace(&format!("file://{root_text}/"), "")
        .replace(&format!("{root_text}/"), "");
    let parsed = match linter.format {
        Format::Sarif => sarif(&stdout, linter.tool),
        Format::ActionlintJson => run::actionlint(&stdout),
    };
    match parsed {
        Ok((diagnostics, limitations)) => {
            facts.limitations.extend(limitations);
            facts.diagnostics = diagnostics
                .into_iter()
                .map(|mut d| {
                    d.code_ref =
                        d.path.parse().ok().and_then(|p| {
                            snapshot.code_ref(SourceSide::Head, &p, d.line, d.line).ok()
                        });
                    d
                })
                .collect();
        }
        Err(err) => {
            facts.status = BackendStatus::Failed;
            let stderr: String = String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(300)
                .collect();
            facts.limitations.push(format!(
                "{} output not readable ({err}): {stderr}",
                linter.tool
            ));
        }
    }
}

/// Arguments that keep a linter off project code its configuration would run, each with the
/// limitation it costs.
fn isolate(
    tool: &str,
    root: &Path,
    scratch: &Path,
    args: &mut Vec<String>,
    limitations: &mut Vec<String>,
) -> io::Result<()> {
    if tool == "oxlint" && run::oxlint_runs_code(root) {
        let isolated = scratch.join("oxlintrc.json");
        if fs::write(&isolated, "{}").is_ok() {
            args.extend([
                "--disable-nested-config".into(),
                "-c".into(),
                isolated.display().to_string(),
            ]);
            limitations.push(
                "the project's oxlint configuration loads JS plugins or a JS config, which would \
                 run repository code: default rules were used instead"
                    .into(),
            );
        }
    }

    if tool == "clj-kondo" {
        let empty = scratch.join("clj-kondo-config");
        fs::create_dir_all(&empty)?;
        args.extend([
            "--config-dir".into(),
            empty.display().to_string(),
            "--lint".into(),
        ]);
        if root.join(".clj-kondo").is_dir() {
            limitations.push(
                "the project's .clj-kondo configuration was not read: its hooks and imported \
                 configs run repository code; default rules were used instead"
                    .into(),
            );
        }
    }

    Ok(())
}

/// SARIF through the CI reader, so locations and limits follow one implementation.
fn sarif(text: &str, tool: &str) -> Result<(Vec<CiDiagnostic>, Vec<String>), String> {
    let id = SourceSnapshotId(sha256(b"lint"));
    let mut facts = CiFacts::new(id, CiFormat::Sarif, tool.into());
    ci::sarif(text, &mut facts).map_err(|e| e.to_string())?;
    Ok((facts.diagnostics, facts.limitations))
}

#[cfg(test)]
mod tests;
