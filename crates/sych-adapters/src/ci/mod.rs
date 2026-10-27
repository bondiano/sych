//! Import of CI reports into normalized `CiFacts` (spec §15, roadmap M1.14). Reports are
//! data from outside the snapshot: they are parsed, never executed, and their spans are
//! mapped onto the head only when the manifest says the head is what was tested.

pub mod archive;
mod cargo;
mod coverage;
mod findings;
pub(crate) use findings::sarif;
mod golang;
mod junit;
mod outcomes;
mod terraform;

use std::fmt::Display;
use std::path::Path;

use sych_core::{CiFacts, CiFormat, CiManifest, CiOrigin, RepoPath, SourceSide, TestStatus};
use thiserror::Error;

use crate::git::LocalSnapshot;

#[derive(Debug, Error)]
pub enum CiError {
    #[error("{0}: neither cargo --message-format=json nor JUnit XML")]
    UnknownFormat(String),
    #[error("{report}: invalid JUnit XML: {source}")]
    Junit {
        report: String,
        #[source]
        source: quick_xml::Error,
    },
    #[error("{report}: malformed report: {detail}")]
    Malformed { report: String, detail: String },
}

/// Diagnostics and tests kept per report; the rest is named in limitations.
const MAX_RECORDS: usize = 2000;
/// XML nesting a report may use; deeper input is refused, not walked.
const MAX_DEPTH: usize = 64;

fn malformed(facts: &CiFacts, detail: &dyn Display) -> CiError {
    CiError::Malformed {
        report: facts.report.clone(),
        detail: detail.to_string(),
    }
}

fn detect(text: &str) -> Option<CiFormat> {
    let start = text.trim_start();
    let first = start.lines().next().unwrap_or_default();
    if first.starts_with("mode: ") {
        return Some(CiFormat::GoCover);
    }

    if first.starts_with("{\"Time\":") || first.starts_with("{\"Action\":") {
        return Some(CiFormat::GoTestJson);
    }

    if (first.starts_with("# ") || start.starts_with('{')) && start.contains("\"posn\"") {
        return Some(CiFormat::GoVetJson);
    }

    if start.starts_with('<') {
        Some(if start.contains("<coverage") {
            CiFormat::Cobertura
        } else {
            CiFormat::Junit
        })
    } else if start.starts_with("TN:") || start.starts_with("SF:") {
        Some(CiFormat::Lcov)
    } else if start.starts_with('[') {
        Some(CiFormat::GitlabCodeQuality)
    } else if start.starts_with('{')
        && start.contains("\"format_version\"")
        && start.contains("\"terraform_version\"")
    {
        Some(CiFormat::TerraformPlan)
    } else if start.starts_with('{') && start.contains("\"runs\"") {
        Some(CiFormat::Sarif)
    } else if start.starts_with('{') && start.contains("\"commands\"") {
        Some(CiFormat::SychManifest)
    } else if start
        .lines()
        .next()
        .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .is_some_and(|v| v.get("reason").is_some())
    {
        Some(CiFormat::CargoJson)
    } else {
        None
    }
}

/// Normalizes one locally given report, its format detected from content.
pub fn import(
    snapshot: &LocalSnapshot,
    name: &str,
    text: &str,
    manifest: Option<&CiManifest>,
) -> Result<CiFacts, CiError> {
    import_as(snapshot, name, text, manifest, None, None)
}

/// Normalizes one report. `name` is the file name shown in the result; `format` is the declared
/// format of an expected report (detected when `None`); `origin` binds an authenticated one.
pub fn import_as(
    snapshot: &LocalSnapshot,
    name: &str,
    text: &str,
    manifest: Option<&CiManifest>,
    format: Option<CiFormat>,
    origin: Option<CiOrigin>,
) -> Result<CiFacts, CiError> {
    let format = format
        .or_else(|| detect(text))
        .ok_or_else(|| CiError::UnknownFormat(name.to_owned()))?;
    let report = Path::new(name)
        .file_name()
        .map_or_else(|| name.to_owned(), |n| n.to_string_lossy().into_owned());
    let mut facts = CiFacts::new(snapshot.snapshot.id.clone(), format, report);
    facts.manifest = manifest.cloned();
    match format {
        CiFormat::CargoJson => cargo::parse(text, &mut facts),
        CiFormat::Junit => junit::parse(text, &mut facts)?,
        CiFormat::Sarif => findings::sarif(text, &mut facts)?,
        CiFormat::GitlabCodeQuality => findings::code_quality(text, &mut facts)?,
        CiFormat::Lcov => coverage::lcov(text, &mut facts)?,
        CiFormat::Cobertura => coverage::cobertura(text, &mut facts)?,
        CiFormat::SychManifest => outcomes::parse(text, &mut facts)?,
        CiFormat::GoTestJson => golang::test_json(text, &mut facts)?,
        CiFormat::GoCover => golang::cover(text, root_module(snapshot).as_deref(), &mut facts)?,
        CiFormat::GoVetJson => golang::vet_json(text, &mut facts)?,
        CiFormat::TerraformPlan => terraform::parse(text, &mut facts)?,
    }
    facts.origin = origin;
    if facts.manifest.is_none() && facts.origin.is_none() {
        facts.limitations.push(
            "UNBOUND: no CI manifest, so toolchain, features, target and tested commit are unknown"
                .into(),
        );
    }

    tracing::info!(report = %facts.report, format = ?format, bound = facts.bound_to(&snapshot.snapshot), diagnostics = facts.diagnostics.len(), tests = facts.tests.len(), "CI report imported");
    if facts.bound_to(&snapshot.snapshot) {
        for d in &mut facts.diagnostics {
            d.code_ref = repo_suffixes(&d.path)
                .find_map(|p| snapshot.code_ref(SourceSide::Head, &p, d.line, d.line).ok());
        }
    } else if facts.manifest.is_some() || facts.origin.is_some() {
        facts.limitations.push(
            "STALE: the report tested another commit or an unproven checkout; its locations are \
             not mapped onto the head"
                .into(),
        );
    }

    // A test changed in the reviewed diff is not an independent oracle for that diff.
    let changed: Vec<&str> = snapshot
        .manifest
        .files
        .iter()
        .filter_map(|f| f.new.as_ref())
        .map(|side| side.path.as_str())
        .collect();
    let mut changed_tests: Vec<&str> = facts
        .tests
        .iter()
        .filter_map(|t| t.file.as_deref())
        .filter(|f| changed.contains(f))
        .collect();
    changed_tests.sort_unstable();
    changed_tests.dedup();
    if !changed_tests.is_empty() {
        let list = changed_tests.join(", ");
        facts.limitations.push(format!(
            "TEST_CHANGED: {list} changed in the reviewed diff; its outcome is not an independent oracle"
        ));
    }

    let skipped = facts
        .tests
        .iter()
        .filter(|t| t.status == TestStatus::Skipped)
        .count();
    if skipped > 0 {
        facts
            .limitations
            .push(format!("{skipped} tests skipped: not run, so not evidence"));
    }

    Ok(facts)
}

/// `path` and, for an absolute CI checkout path, its suffixes from the longest down: Go race
/// and vet reports name `/ci/src/repo/pkg/file.go`.
fn repo_suffixes(path: &str) -> impl Iterator<Item = RepoPath> + '_ {
    let parts: Vec<&str> = path.split('/').collect();
    (0..parts.len().min(16))
        .filter(move |&i| i == 0 || path.starts_with('/'))
        .filter_map(move |i| parts[i..].join("/").parse().ok())
}

/// The `module` of the head's root `go.mod`, which Go coverage paths start with.
fn root_module(snapshot: &LocalSnapshot) -> Option<String> {
    let (_, text) = snapshot
        .blob(SourceSide::Head, &"go.mod".parse().ok()?)
        .ok()?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().trim_matches('"').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_detected_by_content() {
        assert_eq!(
            detect("<?xml version=\"1.0\"?><testsuites/>"),
            Some(CiFormat::Junit)
        );
        assert_eq!(
            detect("{\"reason\":\"build-finished\",\"success\":true}\n"),
            Some(CiFormat::CargoJson)
        );
        assert_eq!(detect("PASS src/a.test.ts"), None);
        assert_eq!(detect("TN:\nSF:src/a.rs\n"), Some(CiFormat::Lcov));
        assert_eq!(
            detect("<?xml version=\"1.0\"?><coverage/>"),
            Some(CiFormat::Cobertura)
        );
        assert_eq!(
            detect("{\"version\":\"2.1.0\",\"runs\":[]}"),
            Some(CiFormat::Sarif)
        );
        assert_eq!(
            detect("[{\"check_name\":\"x\"}]"),
            Some(CiFormat::GitlabCodeQuality)
        );
        assert_eq!(detect("mode: atomic\n"), Some(CiFormat::GoCover));
        assert_eq!(
            detect("{\"format_version\":\"1.2\",\"terraform_version\":\"1.9.5\"}"),
            Some(CiFormat::TerraformPlan)
        );
        assert_eq!(
            detect("{\"Time\":\"2026-01-01T00:00:00Z\",\"Action\":\"start\"}\n"),
            Some(CiFormat::GoTestJson)
        );
        assert_eq!(
            detect("# ex/orders\n{\"ex/orders\": {\"printf\": [{\"posn\": \"/a.go:1:1\"}]}}"),
            Some(CiFormat::GoVetJson)
        );
    }

    #[test]
    fn absolute_ci_paths_are_tried_by_suffix() {
        let found: Vec<String> = repo_suffixes("/ci/src/orders/create.go")
            .map(|p| p.to_string())
            .collect();
        assert!(found.contains(&"orders/create.go".to_owned()), "{found:?}");
        assert_eq!(repo_suffixes("orders/create.go").count(), 1);
    }
}
