//! Labelled quality suite (spec §27.2–27.3). `sych eval validate` is deterministic: it proves
//! the labels are well-formed and anchored — every evidence location resolves to an exact
//! `CodeRef` and the expected syntax shapes are present — not that any model finds them.
//! `sych eval measure` ([`measure`]) reviews the cases with a real model profile.

mod ci;
mod measure;
mod score;

pub use measure::{MeasureArguments, measure};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use sych_adapters::git::LocalSnapshot;
use sych_adapters::syntax;
use sych_core::config::{
    CheckRequest, Execution, ExecutionProfile, RepositoryPolicy, ServerConfig,
};
use sych_core::{RepoPath, SchemaVersion, SourceSide, SyntaxFacts, SyntaxKind};
use tempfile::TempDir;

use crate::args::Comparison;
use crate::errors::{EXIT_EVAL_FAILED, Failure};
use crate::eval::build_inline;
use crate::output::emit_json;
use crate::snapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Split {
    Development,
    Holdout,
}

/// `unsafe` has a known defect, `safe` is its correct twin, `unknown` cannot be decided from
/// the available evidence and must stay `UNKNOWN`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CaseDisposition {
    Safe,
    Unsafe,
    Unknown,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    #[serde(default = "head")]
    side: SourceSide,
    path: RepoPath,
    start_line: u32,
    end_line: u32,
}

fn head() -> SourceSide {
    SourceSide::Head
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFinding {
    /// `defect`, `nitpick`, `rule_violation`, …
    category: String,
    semantic_target: String,
    required_condition: String,
    evidence_locations: Vec<Location>,
}

/// A true remark the case is not about: it does not count as a false finding on a safe case.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tolerated {
    semantic_target: String,
    evidence_locations: Vec<Location>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFact {
    kind: SyntaxKind,
    #[serde(default = "head")]
    side: SourceSide,
    path: RepoPath,
    line: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedObligation {
    rule_id: String,
    expected_status: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Expected {
    obligations: Vec<ExpectedObligation>,
    findings: Vec<ExpectedFinding>,
    /// Shapes the deterministic syntax pass must see.
    facts: Vec<ExpectedFact>,
    /// Shapes it must not see (the safe twin of an unsafe case).
    forbidden_facts: Vec<ExpectedFact>,
    /// Claims a review must not make, matched by meaning, not text.
    forbidden_claims: Vec<String>,
    tolerated: Vec<Tolerated>,
    /// What cannot be decided, for `unknown` cases.
    unknown: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryFixture {
    /// Name given with `--repo NAME=PATH`.
    repo: String,
    base_ref: String,
    head_ref: String,
}

/// Targeted checks of one case: its own profiles, all allowed, instead of the suite config's.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseExecution {
    profiles: BTreeMap<String, ExecutionProfile>,
    /// Requested runs with their parameters; empty runs every profile without parameters.
    #[serde(default)]
    checks: Vec<CheckRequest>,
}

impl CaseExecution {
    fn config(&self) -> Execution {
        Execution {
            enabled: true,
            allowed_profiles: self.profiles.keys().cloned().collect(),
            profiles: self.profiles.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QualityCase {
    #[expect(dead_code, reason = "validated on decode")]
    schema_version: SchemaVersion,
    id: String,
    split: Split,
    language: String,
    /// Cases of one scenario; a pair needs both an `unsafe` and a `safe` case.
    pair: Option<String>,
    variant: CaseDisposition,
    profiles: Vec<String>,
    rationale: String,
    base: Option<BTreeMap<String, String>>,
    head: Option<BTreeMap<String, Option<String>>>,
    repository_fixture: Option<RepositoryFixture>,
    /// CI reports by file name, bound to the case head.
    ci: Option<BTreeMap<String, String>>,
    /// Execution profiles run on the case head (docker), bound like CI reports.
    execution: Option<CaseExecution>,
    expected: Expected,
}

#[derive(Serialize)]
struct CaseReport {
    id: String,
    language: String,
    variant: CaseDisposition,
    /// `PASSED`, `FAILED` or `SKIPPED` (fixture repository not given).
    status: &'static str,
    problems: Vec<String>,
}

#[derive(Serialize)]
struct SuiteReport {
    suite: String,
    split: Option<Split>,
    passed: usize,
    failed: usize,
    skipped: usize,
    /// Case counts per `language/variant`.
    coverage: BTreeMap<String, usize>,
    cases: Vec<CaseReport>,
}

fn label_problems(case: &QualityCase) -> Vec<String> {
    let mut out = Vec::new();
    let e = &case.expected;
    if case.rationale.trim().is_empty() {
        out.push("rationale is empty".into());
    }

    if case.profiles.is_empty() {
        out.push("no review profiles".into());
    }

    if let Some(x) = &case.execution {
        let config = x.config();
        out.extend(
            config
                .problems()
                .into_iter()
                .map(|(field, message)| format!("{field}: {message}")),
        );
        out.extend(
            x.checks
                .iter()
                .filter_map(|c| config.command(&c.profile, &c.params).err())
                .map(|e| format!("execution.checks: {e}")),
        );
    }

    let defects = e.findings.iter().filter(|f| f.category == "defect").count();
    match case.variant {
        CaseDisposition::Unsafe if defects == 0 => {
            out.push("an unsafe case needs a defect finding".into());
        }
        CaseDisposition::Safe if defects > 0 => {
            out.push("a safe case cannot expect a defect".into());
        }
        CaseDisposition::Unknown if e.unknown.as_deref().is_none_or(|u| u.trim().is_empty()) => {
            out.push("an unknown case must say what cannot be decided".into());
        }
        _ => {}
    }
    for o in &e.obligations {
        if !matches!(
            o.expected_status.as_str(),
            "SUPPORTED" | "VIOLATED" | "UNKNOWN" | "NOT_APPLICABLE"
        ) {
            out.push(format!(
                "obligation {}: unknown status {}",
                o.rule_id, o.expected_status
            ));
        }
    }
    for f in &e.findings {
        if f.evidence_locations.is_empty()
            || f.semantic_target.is_empty()
            || f.required_condition.is_empty()
        {
            out.push(format!(
                "finding `{}` needs a target, a condition and evidence locations",
                f.semantic_target
            ));
        }
    }
    for t in &e.tolerated {
        if t.evidence_locations.is_empty() || t.semantic_target.is_empty() {
            out.push(format!(
                "tolerated `{}` needs a target and evidence locations",
                t.semantic_target
            ));
        }
    }
    let inline = case.base.is_some() || case.head.is_some();
    if inline == case.repository_fixture.is_some() {
        out.push("give either inline base/head files or a repository_fixture".into());
    }

    let generic = sych_core::generic_rule_ids();
    let repository: Vec<&ExpectedObligation> = e
        .obligations
        .iter()
        .filter(|o| !generic.contains(&o.rule_id))
        .collect();
    if let Some(rules) = case.base.as_ref().and_then(|b| b.get(".sych/rules.yaml")) {
        for o in &repository {
            if !rules.contains(&format!("id: {}", o.rule_id)) {
                out.push(format!(
                    "obligation {} is not a rule of the base policy",
                    o.rule_id
                ));
            }
        }
    } else if !repository.is_empty() && inline {
        out.push("expected obligations need .sych/rules.yaml in base".into());
    }

    out
}

fn has_fact(facts: &[SyntaxFacts], want: &ExpectedFact) -> bool {
    facts
        .iter()
        .filter(|d| d.side == want.side)
        .flat_map(|d| &d.facts)
        .any(|f| {
            f.kind == want.kind
                && f.code_ref.path == want.path
                && (f.code_ref.start_line..=f.code_ref.end_line).contains(&want.line)
        })
}

fn anchored_problems(case: &QualityCase, snap: &LocalSnapshot) -> Vec<String> {
    let mut out = Vec::new();
    let e = &case.expected;
    let locations = e.findings.iter().map(|f| &f.evidence_locations);
    for ls in locations.chain(e.tolerated.iter().map(|t| &t.evidence_locations)) {
        for l in ls {
            if let Err(err) = snap.code_ref(l.side, &l.path, l.start_line, l.end_line) {
                out.push(format!(
                    "{}:{}-{} does not resolve: {err}",
                    l.path, l.start_line, l.end_line
                ));
            }
        }
    }
    // Registered wrapper calls are facts only against the case's own effect registry.
    let effects = snap
        .policy_files(SourceSide::DiffBase)
        .ok()
        .and_then(|files| RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).ok())
        .map(|policy| policy.effects)
        .unwrap_or_default();
    let facts = syntax::collect(snap, &effects);
    for want in &case.expected.facts {
        if !has_fact(&facts, want) {
            out.push(format!(
                "missing {:?} at {}:{} ({:?})",
                want.kind, want.path, want.line, want.side
            ));
        }
    }
    for banned in &case.expected.forbidden_facts {
        if has_fact(&facts, banned) {
            out.push(format!(
                "unexpected {:?} at {}:{} ({:?})",
                banned.kind, banned.path, banned.line, banned.side
            ));
        }
    }
    out
}

/// The comparison a case reviews: a fixture repository or inline files built into `dir`.
/// `Ok(None)` when its fixture repository was not given.
fn materialize(
    case: &QualityCase,
    repos: &BTreeMap<String, PathBuf>,
    dir: &Path,
) -> Result<Option<Comparison>, String> {
    let (repo, base, head) = if let Some(f) = &case.repository_fixture {
        let Some(path) = repos.get(&f.repo) else {
            return Ok(None);
        };
        (path.clone(), f.base_ref.clone(), f.head_ref.clone())
    } else {
        let base = case.base.clone().unwrap_or_default();
        let head = case.head.clone().unwrap_or_default();
        build_inline(dir, &base, &head)?;
        (dir.to_path_buf(), "target".into(), "HEAD".into())
    };
    Ok(Some(Comparison {
        repo,
        base,
        head,
        diff_base: None,
    }))
}

fn not_given(case: &QualityCase) -> String {
    let repo = case
        .repository_fixture
        .as_ref()
        .map_or("", |f| f.repo.as_str());
    format!("repository `{repo}` not given (--repo {repo}=PATH)")
}

fn check(case: &QualityCase, repos: &BTreeMap<String, PathBuf>) -> (&'static str, Vec<String>) {
    let mut problems = label_problems(case);
    let dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(err) => return ("FAILED", vec![err.to_string()]),
    };
    let comparison = match materialize(case, repos, dir.path()) {
        Ok(Some(comparison)) => comparison,
        Ok(None) => return ("SKIPPED", vec![not_given(case)]),
        Err(err) => {
            problems.push(err);
            return ("FAILED", problems);
        }
    };
    match snapshot::capture(&comparison) {
        Ok(snap) => {
            problems.extend(anchored_problems(case, &snap));
            problems.extend(
                ci::import(case.ci.as_ref(), &comparison, &snap)
                    .err()
                    .map(|e| format!("ci: {e}")),
            );
        }
        Err(err) => problems.push(format!("snapshot: {err}")),
    }
    (
        if problems.is_empty() {
            "PASSED"
        } else {
            "FAILED"
        },
        problems,
    )
}

type Parsed = (PathBuf, Result<QualityCase, String>);

fn read_cases(suite: &Path) -> Result<Vec<Parsed>, String> {
    let mut paths: Vec<PathBuf> = fs::read_dir(suite)
        .map_err(|e| format!("{}: {e}", suite.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|p| {
            let parsed = fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_yaml_ng::from_str(&t).map_err(|e| e.to_string()));
            (p, parsed)
        })
        .collect())
}

/// Pairs and ids across the suite: unique ids, file name = id, one split, complete pairs.
fn suite_problems(cases: &[(PathBuf, QualityCase)]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut pairs: BTreeMap<&str, BTreeSet<&'static str>> = BTreeMap::new();
    for (path, case) in cases {
        let mut push = |p: String| out.entry(case.id.clone()).or_default().push(p);
        if !seen.insert(case.id.as_str()) {
            push("duplicate id".into());
        }

        if path.file_stem().is_none_or(|s| s != case.id.as_str()) {
            push("file name must be <id>.yaml".into());
        }

        if let Some(pair) = &case.pair {
            pairs.entry(pair).or_default().insert(match case.variant {
                CaseDisposition::Safe => "safe",
                CaseDisposition::Unsafe => "unsafe",
                CaseDisposition::Unknown => "unknown",
            });
        }
    }
    for (path, case) in cases {
        let Some(variants) = case.pair.as_deref().and_then(|p| pairs.get(p)) else {
            continue;
        };
        if case.variant != CaseDisposition::Unknown
            && !(variants.contains("safe") && variants.contains("unsafe"))
        {
            out.entry(case.id.clone()).or_default().push(format!(
                "pair `{}` needs both safe and unsafe cases ({})",
                case.pair.as_deref().unwrap_or_default(),
                path.display()
            ));
        }
    }
    out
}

fn repo_paths(repos: &[String]) -> Result<BTreeMap<String, PathBuf>, Failure> {
    let mut paths = BTreeMap::new();
    for spec in repos {
        let (name, path) = spec
            .split_once('=')
            .ok_or_else(|| Failure::invalid(format!("--repo takes NAME=PATH, got `{spec}`")))?;
        paths.insert(name.to_owned(), PathBuf::from(path));
    }
    Ok(paths)
}

pub fn validate(suite: &Path, repos: &[String], output: Option<&Path>) -> anyhow::Result<ExitCode> {
    let repo_paths = repo_paths(repos)?;
    let entries = read_cases(suite).map_err(Failure::invalid)?;
    let mut reports = Vec::new();
    let mut parsed = Vec::new();
    for (path, case) in entries {
        match case {
            Ok(case) => parsed.push((path, case)),
            Err(err) => reports.push(CaseReport {
                id: path.display().to_string(),
                language: String::new(),
                variant: CaseDisposition::Unknown,
                status: "FAILED",
                problems: vec![format!("does not parse: {err}")],
            }),
        }
    }
    let mut cross = suite_problems(&parsed);
    let splits: BTreeSet<_> = parsed.iter().map(|(_, c)| c.split).collect();
    let mut coverage = BTreeMap::new();
    for (_, case) in &parsed {
        let (status, mut problems) = check(case, &repo_paths);
        problems.extend(cross.remove(&case.id).unwrap_or_default());
        if splits.len() > 1 {
            problems.push("a suite directory holds one split; keep holdout separate".into());
        }

        let status = if status == "PASSED" && !problems.is_empty() {
            "FAILED"
        } else {
            status
        };
        *coverage
            .entry(format!("{}/{:?}", case.language, case.variant).to_lowercase())
            .or_insert(0) += 1;
        reports.push(CaseReport {
            id: case.id.clone(),
            language: case.language.clone(),
            variant: case.variant,
            status,
            problems,
        });
    }
    let count = |s: &str| reports.iter().filter(|r| r.status == s).count();
    let report = SuiteReport {
        suite: suite.display().to_string(),
        split: (splits.len() == 1).then(|| *splits.iter().next().expect("one split")),
        passed: count("PASSED"),
        failed: count("FAILED"),
        skipped: count("SKIPPED"),
        coverage,
        cases: reports,
    };
    tracing::info!(
        passed = report.passed,
        failed = report.failed,
        skipped = report.skipped,
        "quality suite validated"
    );
    emit_json(&report, output)?;
    Ok(if report.failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_EVAL_FAILED)
    })
}
