//! A pinned SCIP indexer on one project of a materialized side, in the environment easement's
//! indexers get: a `PATH` of the pinned indexers alone, an empty `HOME`, no network where bwrap
//! can make a namespace, cargo unable to compile (so no build script or proc-macro runs),
//! scip-python given an empty environment (so no `python` or `pip` runs) and no package script.
//! A run past its time bound is killed, and an index past its size bound is not read.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use sych_core::{BackendStatus, Language};

use crate::git::Materialized;
use crate::lint::easement::{self, Failure};
use crate::lint::run;
use crate::semantic::Options;

/// Each run indexes a whole project, not only the changed files.
pub(super) const TIMEOUT: Duration = Duration::from_secs(600);
/// A larger index is not read: the project is past what one review analyses.
pub(super) const MAX_INDEX_BYTES: u64 = 512 << 20;

/// The SCIP index of the `language` project in `dir` of the workspace at `root`; what the run
/// costs goes to `limitations`.
pub(super) fn index(
    language: Language,
    root: &Path,
    m: &Materialized,
    dir: &str,
    scratch: &Path,
    options: &Options<'_>,
    limitations: &mut Vec<String>,
) -> Result<Vec<u8>, Failure> {
    fs::create_dir_all(scratch).map_err(|e| failed(format!("scratch directory: {e}")))?;
    let languages = BTreeSet::from([language]);
    let vars = easement::environment(root, m, dir, &languages, scratch, options, limitations)?;
    let bin = scratch.join("bin");
    let output = scratch.join("index.scip");
    let project = root.join(dir);
    let out = output.display().to_string();
    let (program, args): (&str, Vec<String>) = match language {
        Language::Rust => (
            "rust-analyzer",
            vec!["scip".into(), ".".into(), "--output".into(), out],
        ),
        Language::Go => ("scip-go", vec![format!("--output={out}")]),
        Language::Typescript => {
            let mut args = vec!["index".into(), "--output".into(), out];
            if !project.join("tsconfig.json").is_file() {
                args.push("--infer-tsconfig".into());
            }

            ("scip-typescript", args)
        }
        Language::Python => (
            "scip-python",
            vec![
                "index".into(),
                ".".into(),
                "--project-name".into(),
                python_project(&project, dir),
                "--project-version".into(),
                "0".into(),
                "--output".into(),
                out,
            ],
        ),
    };
    let ran = run::run(&bin.join(program), &args, &project, scratch, &vars, TIMEOUT)
        .map_err(|e| failed(format!("{program}: {e}")))?;
    if !ran.status.success() {
        let stderr: String = String::from_utf8_lossy(&ran.stderr)
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(failed(format!("{program} {}: {stderr}", ran.status)));
    }

    let size = fs::metadata(&output)
        .map_err(|e| failed(format!("{program} wrote no index: {e}")))?
        .len();
    if size > MAX_INDEX_BYTES {
        return Err(failed(format!(
            "{program} wrote an index of {size} bytes, past the {MAX_INDEX_BYTES} it may have"
        )));
    }

    fs::read(&output).map_err(|e| failed(format!("{program} index: {e}")))
}

/// The name scip-python gives the project's package: `[project] name` of its
/// `pyproject.toml`, else its directory's, else `root`.
fn python_project(project: &Path, dir: &str) -> String {
    fs::read_to_string(project.join("pyproject.toml"))
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .and_then(|t| t.get("project")?.get("name")?.as_str().map(str::to_owned))
        .or_else(|| {
            dir.rsplit('/')
                .next()
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "root".to_owned())
}

fn failed(why: String) -> Failure {
    (BackendStatus::Failed, why)
}
