//! CI reports a case carries (Terraform plans, rendered manifests, test outcomes): written
//! outside the case repository with a manifest that binds them to its head, so the review
//! imports them exactly as `sych review --ci-facts … --ci-manifest …` would.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use sych_adapters::git::LocalSnapshot;
use tempfile::TempDir;

use crate::args::Comparison;
use crate::ci::import as import_reports;

pub(super) struct CaseCi {
    pub reports: Vec<PathBuf>,
    pub manifest: Option<PathBuf>,
    /// Keeps the files alive for the review.
    _dir: Option<TempDir>,
}

fn head_oid(comparison: &Comparison) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(&comparison.repo)
        .args(["rev-parse", "--verify"])
        .arg(format!("{}^{{commit}}", comparison.head))
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("cannot resolve {}", comparison.head));
    }

    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Report names are plain file names; the manifest says the case head is what was tested.
pub(super) fn write(
    reports: Option<&BTreeMap<String, String>>,
    comparison: &Comparison,
) -> Result<CaseCi, String> {
    let Some(reports) = reports.filter(|r| !r.is_empty()) else {
        return Ok(CaseCi {
            reports: Vec::new(),
            manifest: None,
            _dir: None,
        });
    };
    let dir = TempDir::new().map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (name, text) in reports {
        if name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(format!("ci report `{name}` must be a plain file name"));
        }

        let path = dir.path().join(name);
        fs::write(&path, text).map_err(|e| format!("{name}: {e}"))?;
        paths.push(path);
    }
    let manifest = dir.path().join("sych-ci.yaml");
    let text = format!(
        "schema_version: 1\njob: eval:case\ntested_oid: \"{}\"\ntoolchain: eval\ntarget: eval\n",
        head_oid(comparison)?
    );
    fs::write(&manifest, text).map_err(|e| format!("manifest: {e}"))?;
    Ok(CaseCi {
        reports: paths,
        manifest: Some(manifest),
        _dir: Some(dir),
    })
}

/// The case reports as the review would see them; an import error is a labelling problem.
pub(super) fn import(
    reports: Option<&BTreeMap<String, String>>,
    comparison: &Comparison,
    snap: &LocalSnapshot,
) -> Result<(), String> {
    let ci = write(reports, comparison)?;
    import_reports(snap, &ci.reports, ci.manifest.as_deref())
        .map(drop)
        .map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::{QualityCase, check};

    fn case(plan: &str) -> QualityCase {
        let yaml = format!(
            "schema_version: 1\nid: tf\nsplit: development\nlanguage: terraform\nvariant: unknown\n\
             profiles: [infrastructure]\nrationale: r\nbase:\n  infra/db.tf: |\n    a = 1\nhead:\n  infra/db.tf: |\n    a = 2\n\
             ci:\n  plan.json: '{plan}'\nexpected:\n  obligations:\n    - {{ rule_id: TF-CHANGE, expected_status: UNKNOWN }}\n  unknown: u\n"
        );
        serde_yaml_ng::from_str(&yaml).expect("case")
    }

    #[test]
    fn case_reports_are_imported_and_a_broken_one_fails_the_case() {
        let good =
            r#"{"format_version": "1.2", "terraform_version": "1.9.5", "resource_changes": []}"#;
        let (status, problems) = check(&case(good), &BTreeMap::new());
        assert_eq!(status, "PASSED", "{problems:?}");

        let major =
            r#"{"format_version": "2.0", "terraform_version": "2.0.0", "resource_changes": []}"#;
        let (status, problems) = check(&case(major), &BTreeMap::new());
        assert_eq!(status, "FAILED");
        assert!(
            problems.iter().any(|p| p.starts_with("ci:")),
            "{problems:?}"
        );
    }
}
