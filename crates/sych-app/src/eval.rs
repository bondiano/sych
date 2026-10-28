//! Harness-correctness suite for `sych eval run` (spec §24.4, §27.1). Each case builds a small Git
//! repository from the suite, reviews it with the fake adapter into a throwaway store and
//! compares significant states, not runtime ids or timestamps.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use sych_core::config::RunMode;

use serde::{Deserialize, Serialize};
use sych_core::{Coverage, ObligationStatus, Recommendation, RepoPath, SchemaVersion};
use sych_storage::{Storage, StoreConfig};
use tempfile::TempDir;

use crate::args::{Comparison, ExitPolicy, ModelSelection, Output, ReviewArguments, Switch};
use crate::errors::{EXIT_EVAL_FAILED, Failure};
use crate::output::emit_json;
use crate::render::{self, Format};
use crate::{review, storage};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Suite {
    #[expect(dead_code, reason = "validated on decode")]
    schema_version: SchemaVersion,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    /// Files of the target commit; `.sych/` here is the trusted policy.
    base: BTreeMap<String, String>,
    /// Changes of the reviewed head on top of base; `null` deletes a file.
    #[serde(default)]
    head: BTreeMap<String, Option<String>>,
    /// Relative to the suite file.
    fake_responses: Option<PathBuf>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    recommendation: Option<Recommendation>,
    coverage: Option<Coverage>,
    /// Rule id to status, e.g. `SUPPORTED`.
    #[serde(default)]
    obligations: BTreeMap<String, String>,
    findings: Option<usize>,
    units: Option<usize>,
}

/// The significant state of one result.
#[derive(Serialize)]
struct Actual {
    recommendation: Recommendation,
    coverage: Coverage,
    obligations: BTreeMap<String, String>,
    findings: usize,
    units: usize,
    model_adapters: Vec<String>,
    model_attempts: u32,
}

#[derive(Serialize)]
struct CaseReport {
    name: String,
    passed: bool,
    mismatches: Vec<String>,
    actual: Option<Actual>,
}

#[derive(Serialize)]
struct SuiteReport {
    suite: String,
    passed: usize,
    failed: usize,
    /// Model calls across the suite; zero with the fake adapter.
    model_attempts: u32,
    cases: Vec<CaseReport>,
}

pub(crate) fn status_tag(status: &ObligationStatus) -> &'static str {
    match status {
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::Unknown { .. } => "UNKNOWN",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".git-eval-config"))
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .args([
            "-c",
            "user.name=sych-eval",
            "-c",
            "user.email=eval@sych.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=.git/no-hooks",
        ])
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Suite paths are repository paths: never absolute and never leaving the work tree.
fn write_tree<'a>(
    dir: &Path,
    files: impl IntoIterator<Item = (&'a String, Option<&'a String>)>,
) -> Result<(), String> {
    for (path, content) in files {
        let path: RepoPath = path.parse().map_err(|e| format!("{e}"))?;
        let host = dir.join(path.as_str());
        let written = match content {
            Some(text) => host
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(&host, text)),
            None => fs::remove_file(&host),
        };
        written.map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

/// A repository whose `target` branch holds `base` and whose `HEAD` adds `head` on top.
pub fn build_inline(
    dir: &Path,
    base: &BTreeMap<String, String>,
    head: &BTreeMap<String, Option<String>>,
) -> Result<(), String> {
    git(dir, &["init", "-q", "-b", "main"])?;
    write_tree(dir, base.iter().map(|(p, c)| (p, Some(c))))?;
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-q", "--allow-empty", "-m", "base"])?;
    git(dir, &["branch", "target"])?;
    write_tree(dir, head.iter().map(|(p, c)| (p, c.as_ref())))?;
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-q", "--allow-empty", "-m", "head"])
}

fn build_repo(case: &Case, dir: &Path) -> Result<(), String> {
    build_inline(dir, &case.base, &case.head)
}

fn compare(expect: &Expect, actual: &Actual) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |what: &str, want: Option<String>, got: String| {
        if let Some(want) = want.filter(|w| *w != got) {
            out.push(format!("{what}: expected {want}, got {got}"));
        }
    };
    check(
        "recommendation",
        expect.recommendation.map(|r| render::tag(&r)),
        render::tag(&actual.recommendation),
    );
    check(
        "coverage",
        expect.coverage.map(|c| render::tag(&c)),
        render::tag(&actual.coverage),
    );
    check(
        "findings",
        expect.findings.map(|n| n.to_string()),
        actual.findings.to_string(),
    );
    check(
        "units",
        expect.units.map(|n| n.to_string()),
        actual.units.to_string(),
    );
    for (rule, want) in &expect.obligations {
        let got = actual
            .obligations
            .get(rule)
            .map_or("absent", String::as_str);
        check(
            &format!("obligation {rule}"),
            Some(want.clone()),
            got.into(),
        );
    }
    out
}

fn run_case(case: &Case, suite_dir: &Path) -> Result<Actual, String> {
    let repo = TempDir::new().map_err(|e| e.to_string())?;
    let data = TempDir::new().map_err(|e| e.to_string())?;
    build_repo(case, repo.path())?;
    let store = || StoreConfig {
        database: data.path().join("sych.db"),
        artifacts: data.path().join("artifacts"),
    };
    storage::block_on(Storage::migrate(&store()))
        .map_err(|e| format!("{e:#}"))?
        .map_err(|e| e.to_string())?;
    let args = ReviewArguments {
        change: None,
        comparison: Comparison {
            repo: repo.path().to_path_buf(),
            base: "target".into(),
            head: "HEAD".into(),
            diff_base: None,
        },
        config: None,
        models: ModelSelection::Fake,
        policy_ref: None,
        fake_responses: case.fake_responses.as_ref().map(|p| suite_dir.join(p)),
        // Harness evaluation stays deterministic and offline.
        semantic: Switch::Off,
        ci_facts: Vec::new(),
        ci_manifest: None,
        exit_policy: ExitPolicy::Operational,
        out: Output {
            format: Format::Json,
            output: None,
        },
        verify: true,
        ci_imported: Vec::new(),
        ci_gaps: Vec::new(),
        checks: Vec::new(),
        execution: None,
        waivers: Vec::new(),
        run_mode: RunMode::PersonalLocal,
        deep: false,
        parent: None,
        risk_audit: true,
    };
    let report =
        review::run(&args, Some(store())).map_err(|e| format!("review saved no result: {e:#}"))?;
    let r = report.review_result;
    Ok(Actual {
        recommendation: r.recommendation,
        coverage: r.coverage,
        obligations: r
            .obligations
            .iter()
            .map(|o| (o.rule_id.clone(), status_tag(&o.status).to_owned()))
            .collect(),
        findings: r.findings.len(),
        units: r.scope_manifest.units.len(),
        model_adapters: r.provenance.model_adapters,
        model_attempts: r.usage_summary.model_attempts,
    })
}

pub fn run(
    suite_path: &Path,
    models: &ModelSelection,
    output: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    if models != &ModelSelection::Fake {
        return Err(
            Failure::invalid("eval run is the offline harness suite; use --models fake").into(),
        );
    }

    let suite: Suite = fs::read_to_string(suite_path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_yaml_ng::from_str(&t).map_err(|e| e.to_string()))
        .map_err(|err| {
            Failure::invalid(format!("invalid suite {}: {err}", suite_path.display()))
        })?;
    let suite_dir = suite_path.parent().unwrap_or(Path::new("."));
    let cases: Vec<CaseReport> = suite
        .cases
        .iter()
        .map(|case| {
            let (mismatches, actual) = match run_case(case, suite_dir) {
                Ok(actual) => (compare(&case.expect, &actual), Some(actual)),
                Err(err) => (vec![err], None),
            };
            CaseReport {
                name: case.name.clone(),
                passed: mismatches.is_empty(),
                mismatches,
                actual,
            }
        })
        .collect();
    let failed = cases.iter().filter(|c| !c.passed).count();
    let report = SuiteReport {
        suite: suite_path.display().to_string(),
        passed: cases.len() - failed,
        failed,
        model_attempts: cases
            .iter()
            .filter_map(|c| c.actual.as_ref())
            .map(|a| a.model_attempts)
            .sum(),
        cases,
    };
    emit_json(&report, output)?;
    tracing::info!(passed = report.passed, failed, "eval suite finished");
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_EVAL_FAILED)
    })
}
