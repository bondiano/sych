//! easement's JSON report, and the files an `easement.yml` makes part of its policy.

use std::collections::BTreeSet;
use std::iter;

use serde::Deserialize;
use sych_core::CiDiagnostic;

#[derive(Deserialize)]
pub(super) struct Report {
    violations: Vec<Violation>,
    /// Violations of rules in `warn`.
    #[serde(default)]
    warnings: Vec<Violation>,
    #[serde(default)]
    evidence_gaps: Vec<Gap>,
}

#[derive(Deserialize)]
pub(super) struct Violation {
    rule: String,
    pub(super) file: String,
    pub(super) line: u32,
    message: String,
    because: Option<String>,
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
    #[serde(default)]
    specifier: String,
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

/// What a violation is apart from the lines it sits at: base and head compare by it.
pub(super) type Key = (String, String, String, String, String);

impl Report {
    /// The report of the project in `dir`, its paths made repository paths.
    pub(super) fn read(text: &str, dir: &str) -> Result<Self, serde_json::Error> {
        let mut report: Self = serde_json::from_str(text)?;
        let rebase = |path: &mut String| {
            if !dir.is_empty() {
                *path = format!("{dir}/{path}");
            }
        };
        for v in report.violations.iter_mut().chain(&mut report.warnings) {
            rebase(&mut v.file);
            v.evidence.iter_mut().for_each(|p| rebase(&mut p.file));
        }

        for gap in &mut report.evidence_gaps {
            gap.files.iter_mut().for_each(rebase);
        }

        Ok(report)
    }

    /// Every violation with its level.
    pub(super) fn all(&self) -> impl Iterator<Item = (&'static str, &Violation)> {
        self.violations
            .iter()
            .map(|v| ("error", v))
            .chain(self.warnings.iter().map(|v| ("warning", v)))
    }

    /// Diagnostics at the places in `files` behind each violation, one per rule and place.
    pub(super) fn diagnostics(&self, files: &[String]) -> Vec<CiDiagnostic> {
        let mut out: Vec<CiDiagnostic> = Vec::new();
        for (level, v) in self.all() {
            for (path, line) in v.places() {
                let diagnostic = v.diagnostic(level, path, line);
                if files.iter().any(|f| f == path) && !out.contains(&diagnostic) {
                    out.push(diagnostic);
                }
            }
        }
        out
    }

    /// Each evidence gap's message, naming the `files` it leaves partly unjudged.
    pub(super) fn gaps(&self, files: &[String]) -> Vec<String> {
        self.evidence_gaps
            .iter()
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
                    g.message.clone()
                } else {
                    format!("{} (changed files: {})", g.message, hit.join(", "))
                }
            })
            .collect()
    }
}

impl Violation {
    pub(super) fn key(&self, file: &str) -> Key {
        (
            self.rule.clone(),
            self.from.clone(),
            self.to.clone(),
            self.specifier.clone(),
            file.to_owned(),
        )
    }

    /// Where the violation is reported, then each import behind it.
    pub(super) fn places(&self) -> impl Iterator<Item = (&str, u32)> {
        iter::once((self.file.as_str(), self.line))
            .chain(self.evidence.iter().map(|p| (p.file.as_str(), p.line)))
    }

    pub(super) fn diagnostic(&self, level: &str, path: &str, line: u32) -> CiDiagnostic {
        CiDiagnostic {
            code_ref: None,
            path: path.to_owned(),
            line,
            level: level.into(),
            code: Some(self.rule.clone()),
            message: match &self.because {
                Some(because) => format!("{}; because: {because}", self.message),
                None => self.message.clone(),
            },
        }
    }
}

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    extends: Extends,
    baseline: Option<String>,
    diagram: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Extends {
    #[default]
    None,
    One(String),
    Many(Vec<String>),
}

/// Repository paths of the files the `easement.yml` in `dir` (with `text`) reads: itself, the
/// configs it extends, its baseline and its diagram. A path leaving the repository is dropped.
// shortcut: only the first level of `extends` is followed; follow nested ones when a project uses them.
pub(super) fn config_paths(dir: &str, config: &str, text: &str) -> BTreeSet<String> {
    let parsed: Config = serde_yaml_ng::from_str(text).unwrap_or_default();
    let extends = match parsed.extends {
        Extends::None => Vec::new(),
        Extends::One(path) => vec![path],
        Extends::Many(paths) => paths,
    };
    let baseline = parsed
        .baseline
        .unwrap_or_else(|| "easement.baseline.yml".into());
    iter::once(baseline)
        .chain(extends)
        .chain(parsed.diagram)
        .filter_map(|p| join(dir, &p))
        .chain([config.to_owned()])
        .collect()
}

/// `rel` under `dir`, with `.` and `..` resolved; `None` outside the repository.
fn join(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests;
