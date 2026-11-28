//! Planner: review units, exclusions, risk hints and obligations fixed before discovery
//!. Pure and deterministic for the same snapshot and policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{RepositoryPolicy, RuleException};
use crate::{
    BLOB_TOO_LARGE, ContentKind, DiffManifest, FileChange, Obligation, PathGap, RepoPath,
    ReviewUnit, ReviewUnitId, RiskHint, RiskLevel, RunFacts, ScopeManifest, Severity, Sha256Digest,
    SourceSnapshotId, derived_uuid, glob_match,
};

mod rules;
use rules::{obligations, preferences};

/// An obligation as planned, with the rule data the reducer needs later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedObligation {
    /// Before assessment an applicable obligation is `UNKNOWN/NOT_ASSESSED`, so empty
    /// discovery can never close it.
    pub obligation: Obligation,
    pub rule_title: String,
    /// Acceptance criteria one by one; the rule statement when the rule lists none.
    pub criteria: Vec<String>,
    pub severity_on_violation: Option<Severity>,
    /// Exceptions of the rule scoped to a path of this change; the reducer applies the ones
    /// still in force, and they never rewrite the status.
    pub exceptions: Vec<RuleException>,
    /// The rule's contract documents from the trusted policy: path, content hash and text.
    #[serde(default)]
    pub contracts: Vec<(RepoPath, Sha256Digest, String)>,
    /// The shipped `TEST-INTEGRITY` rule, whose subject is the tests, so test files are its
    /// evidence. Never set for a repository rule, whatever its id.
    #[serde(default)]
    pub about_tests: bool,
}

/// A repository preference (`RuleKind::Preference`) matched to review units of this change.
/// Preferences never create obligations, never end in `UNKNOWN`, and never block a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedPreference {
    pub rule_id: String,
    pub rule_version: u32,
    pub rule_title: String,
    pub statement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    pub review_unit_ids: Vec<ReviewUnitId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contracts: Vec<(RepoPath, Sha256Digest, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<RuleException>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPlan {
    pub source_snapshot_id: SourceSnapshotId,
    pub scope: ScopeManifest,
    pub obligations: Vec<PlannedObligation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferences: Vec<PlannedPreference>,
    /// Files in scope whose content is not reviewed; coverage cannot be complete.
    pub unreviewed: Vec<PathGap>,
    /// Parts of the diff the snapshot could not obtain.
    pub omissions: Vec<String>,
    pub limit_reached: bool,
}

pub const NOT_ASSESSED: &str = "NOT_ASSESSED";
pub const LIMIT_MAX_SOURCE_FILES: &str = "LIMIT_MAX_SOURCE_FILES";
/// Hidden from the models only because the change's own content marks it generated.
pub const GENERATED_IN_CHANGE: &str = "GENERATED_IN_CHANGE";

/// The side a file is shown and grouped by: new, or old for a deletion. Not its identity:
/// rule and semantic matching use both sides through [`paths`].
fn reporting_side(file: &FileChange) -> Option<&crate::FileSide> {
    file.new.as_ref().or(file.old.as_ref())
}

fn paths(file: &FileChange) -> impl Iterator<Item = &RepoPath> {
    file.old.iter().chain(file.new.iter()).map(|s| &s.path)
}

/// Declared out of reading: the file stays in scope and its obligations stay.
fn excluded_reason(file: &FileChange) -> Option<&'static str> {
    match reporting_side(file)?.content {
        ContentKind::Binary => Some("BINARY"),
        ContentKind::LfsPointer => Some("LFS_POINTER"),
        ContentKind::Symlink => Some("SYMLINK"),
        ContentKind::Submodule => Some("SUBMODULE"),
        _ if file.generated => Some("GENERATED"),
        _ => None,
    }
}

/// Content that should have been reviewed but cannot be.
fn unreviewed_reason(file: &FileChange) -> Option<String> {
    if let Some(code) = file
        .limitations
        .iter()
        .find(|l| l.as_str() == "SECRET_PATH" || l.as_str() == "PATH_NOT_UTF8")
    {
        return Some(code.clone());
    }

    if file.generated_claim {
        return Some(GENERATED_IN_CHANGE.into());
    }

    match reporting_side(file)?.content {
        ContentKind::NotRead => Some(
            file.limitations
                .iter()
                .find(|l| l.starts_with(BLOB_TOO_LARGE))
                .cloned()
                .unwrap_or_else(|| "NOT_READ".into()),
        ),
        ContentKind::NonUtf8Text => Some("CONTENT_NOT_UTF8".into()),
        _ => None,
    }
}

const RISK_TABLE: &[(&[&str], RiskLevel, &str)] = &[
    (
        &["**/migrations/**", "**/migrate/**", "**/*.sql"],
        RiskLevel::High,
        "MIGRATION",
    ),
    (
        &[
            ".github/workflows/**",
            ".gitlab-ci.yml",
            "**/.gitlab-ci.yml",
        ],
        RiskLevel::High,
        "CI_CONFIG",
    ),
    (
        &[
            "**/*.tf",
            "**/*.tfvars",
            "**/k8s/**",
            "**/helm/**",
            "**/charts/**",
        ],
        RiskLevel::High,
        "INFRASTRUCTURE",
    ),
    (
        &["**/auth/**", "**/*auth*", "**/*tenant*", "**/*permission*"],
        RiskLevel::High,
        "AUTH",
    ),
    (
        &[
            "**/Cargo.toml",
            "**/Cargo.lock",
            "**/package.json",
            "**/*.lock",
            "**/go.mod",
            "**/go.sum",
            "**/Dockerfile",
            "**/build.rs",
        ],
        RiskLevel::Medium,
        "DEPENDENCY_OR_BUILD",
    ),
    (
        &[
            "**/tests/**",
            "**/test/**",
            "**/*_test.*",
            "**/*.test.*",
            "**/*.spec.*",
        ],
        RiskLevel::Medium,
        "TEST_CHANGE",
    ),
    (&[".sych/**"], RiskLevel::Medium, "POLICY_CHANGE"),
];

fn risk_hints(file: &FileChange) -> Vec<RiskHint> {
    let Some(side) = reporting_side(file) else {
        return Vec::new();
    };
    let mut hints: Vec<RiskHint> = RISK_TABLE
        .iter()
        .filter(|(globs, _, _)| {
            paths(file).any(|p| globs.iter().any(|g| glob_match(g, p.as_str())))
        })
        .map(|(_, level, code)| RiskHint {
            level: *level,
            code: (*code).into(),
            path: side.path.clone(),
        })
        .collect();
    if file.status == crate::ChangeStatus::Renamed {
        hints.push(RiskHint {
            level: RiskLevel::Medium,
            code: "MOVE".into(),
            path: side.path.clone(),
        });
    }

    hints
}

/// Builds the plan from the trusted policy, the generic rule pack and the facts of the
/// snapshot. Files beyond `max_source_files` stay in scope and are listed unreviewed.
pub fn plan(
    snapshot: &SourceSnapshotId,
    manifest: &DiffManifest,
    policy: &RepositoryPolicy,
    facts: &RunFacts,
) -> ReviewPlan {
    let mut excluded = Vec::new();
    let mut unreviewed = Vec::new();
    let mut reviewable: u32 = 0;
    let max = policy.config.context.max_source_files;
    // One unit per file: the changed files of a directory shared one unit's code budget, so
    // each showed only a slice. Discovery batching joins small units into one call again.
    let mut by_file: BTreeMap<String, Vec<&FileChange>> = BTreeMap::new();
    for file in &manifest.files {
        let Some(side) = reporting_side(file) else {
            continue;
        };
        let gap = |reason: &str| PathGap {
            path: side.path.clone(),
            reason: reason.to_owned(),
        };
        if let Some(reason) = excluded_reason(file) {
            excluded.push(gap(reason));
        } else if let Some(reason) = unreviewed_reason(file) {
            unreviewed.push(gap(&reason));
        } else if reviewable >= max {
            unreviewed.push(gap(LIMIT_MAX_SOURCE_FILES));
        } else {
            reviewable += 1;
        }

        by_file
            .entry(side.path.as_str().to_owned())
            .or_default()
            .push(file);
    }
    let limit_reached = unreviewed
        .iter()
        .any(|g| g.reason == LIMIT_MAX_SOURCE_FILES);
    let units: Vec<(ReviewUnit, Vec<&FileChange>)> = by_file
        .into_iter()
        .map(|(title, files)| {
            let unit = ReviewUnit {
                id: ReviewUnitId(derived_uuid(&["unit", &snapshot.0.to_string(), &title])),
                paths: files
                    .iter()
                    .filter_map(|f| reporting_side(f))
                    .map(|s| s.path.clone())
                    .collect(),
                risk_hints: files.iter().flat_map(|f| risk_hints(f)).collect(),
                title,
            };
            (unit, files)
        })
        .collect();
    let (obligations, preferences) = if units.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            obligations(
                snapshot,
                policy,
                &units,
                facts,
                !manifest.omissions.is_empty(),
            ),
            preferences(policy, &units, facts),
        )
    };
    let requested_profiles = policy
        .config
        .review
        .profiles
        .iter()
        .filter_map(|p| serde_json::to_value(p).ok())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    ReviewPlan {
        source_snapshot_id: snapshot.clone(),
        scope: ScopeManifest {
            revision: 1,
            units: units.into_iter().map(|(u, _)| u).collect(),
            required_obligation_ids: obligations
                .iter()
                .filter(|o| o.obligation.required)
                .map(|o| o.obligation.id)
                .collect(),
            requested_profiles,
            excluded,
        },
        obligations,
        preferences,
        unreviewed,
        omissions: manifest.omissions.clone(),
        limit_reached,
    }
}
