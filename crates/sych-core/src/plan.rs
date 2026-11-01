//! Planner: review units, exclusions, risk hints and obligations fixed before discovery
//!. Pure and deterministic for the same snapshot and policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::config::{RepositoryPolicy, Rule, RuleException, RuleKind, RuleSeverity};
use crate::deploy::TransitionMatrix;
use crate::generic;
use crate::{
    Applicability, ContentKind, DiffManifest, FileChange, Obligation, ObligationId,
    ObligationStatus, PathGap, RepoPath, ReviewUnit, ReviewUnitId, RiskHint, RiskLevel, RunFacts,
    ScopeManifest, Severity, Sha256Digest, SourceSnapshotId, derived_uuid, glob_match,
};
use crate::{deploy, kubernetes, migration, terraform};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPlan {
    pub source_snapshot_id: SourceSnapshotId,
    pub scope: ScopeManifest,
    pub obligations: Vec<PlannedObligation>,
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
    if file.generated_claim {
        return Some(GENERATED_IN_CHANGE.into());
    }

    if let Some(code) = file
        .limitations
        .iter()
        .find(|l| l.as_str() == "SECRET_PATH" || l.as_str() == "PATH_NOT_UTF8")
    {
        return Some(code.clone());
    }

    match reporting_side(file)?.content {
        ContentKind::NotRead => Some(
            file.limitations
                .iter()
                .find(|l| l.starts_with("BLOB_TOO_LARGE"))
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

fn unit_title(path: &RepoPath) -> String {
    path.as_str()
        .rsplit_once('/')
        .map_or_else(|| "(root)".to_owned(), |(dir, _)| dir.to_owned())
}

fn risk_of(severity: Option<RuleSeverity>) -> RiskLevel {
    match severity {
        Some(RuleSeverity::Critical) => RiskLevel::Critical,
        Some(RuleSeverity::High) => RiskLevel::High,
        Some(RuleSeverity::Medium) => RiskLevel::Medium,
        _ => RiskLevel::Low,
    }
}

fn kinds_text(rule: &Rule) -> String {
    rule.acceptance.as_ref().map_or_else(
        || "any valid evidence".to_owned(),
        |a| {
            let kinds: Vec<String> = a
                .evidence_kinds
                .iter()
                .filter_map(|k| serde_json::to_value(k).ok())
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            kinds.join(", ")
        },
    )
}

/// Units whose old or new paths match the rule, then units linked to it semantically: a
/// changed construct called or referenced from the rule's paths keeps the rule, so moving
/// logic out of a path does not drop its invariant.
fn matched_units<'u>(
    rule: &Rule,
    units: &'u [(ReviewUnit, Vec<&FileChange>)],
    facts: &RunFacts,
) -> (Vec<&'u ReviewUnit>, Vec<String>) {
    let in_rule = |p: &RepoPath| {
        rule.applies_to
            .paths
            .iter()
            .any(|g| glob_match(g, p.as_str()))
    };
    let mut links = Vec::new();
    let matched = units
        .iter()
        .filter(|(_, files)| {
            if rule.applies_to.paths.is_empty() || files.iter().flat_map(|f| paths(f)).any(in_rule)
            {
                return true;
            }

            // Both sides: a renamed helper keeps its base-side callers.
            let linked = facts
                .semantic
                .iter()
                .flat_map(|f| &f.symbols)
                .filter(|s| {
                    files
                        .iter()
                        .flat_map(|f| paths(f))
                        .any(|p| *p == s.definition.path)
                })
                .find_map(|s| {
                    s.callers
                        .refs
                        .iter()
                        .chain(&s.references.refs)
                        .find(|r| in_rule(&r.path))
                        .map(|r| format!("`{}` is used from {}:{}", s.name, r.path, r.start_line))
                });
            linked.map(|l| links.push(l)).is_some()
        })
        .map(|(unit, _)| unit)
        .collect();
    (matched, links)
}

fn exceptions(rule: &Rule, matched: &[&ReviewUnit]) -> Vec<RuleException> {
    rule.exceptions
        .iter()
        .filter(|e| {
            matched
                .iter()
                .flat_map(|u| &u.paths)
                .any(|p| e.paths.iter().any(|g| glob_match(g, p.as_str())))
        })
        .cloned()
        .collect()
}

/// One obligation per invariant rule; preferences only feed nitpicks.
fn obligation(
    snapshot: &SourceSnapshotId,
    rule: &Rule,
    matched: &[&ReviewUnit],
    diff_incomplete: bool,
) -> PlannedObligation {
    let risk = matched
        .iter()
        .flat_map(|u| u.risk_hints.iter().map(|h| h.level))
        .chain([risk_of(rule.severity_on_violation)])
        .max()
        .unwrap_or(RiskLevel::Low);
    let (applicability, status) = if matched.is_empty() && diff_incomplete {
        (
            Applicability::Unknown,
            ObligationStatus::Unknown {
                reason_code: "DIFF_INCOMPLETE".into(),
                next_evidence_needed: "the omitted part of the diff".into(),
            },
        )
    } else if matched.is_empty() {
        (
            Applicability::DoesNotApply,
            ObligationStatus::NotApplicable {
                basis: format!(
                    "no changed path, old or new, matches applies_to.paths {:?}",
                    rule.applies_to.paths
                ),
            },
        )
    } else if !rule.applies_to.change_kinds.is_empty() {
        // shortcut: change kinds need a semantic provider; until then they stay UNKNOWN.
        (
            Applicability::Unknown,
            ObligationStatus::Unknown {
                reason_code: "CHANGE_KIND_UNRESOLVED".into(),
                next_evidence_needed: format!(
                    "semantic classification of change kinds {:?}",
                    rule.applies_to.change_kinds
                ),
            },
        )
    } else {
        (
            Applicability::Applies,
            ObligationStatus::Unknown {
                reason_code: NOT_ASSESSED.into(),
                next_evidence_needed: format!("assessment backed by {}", kinds_text(rule)),
            },
        )
    };
    let version = rule.version.to_string();
    let criteria: Vec<String> = rule
        .acceptance
        .as_ref()
        .map(|a| a.criteria.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| vec![rule.statement.trim().to_owned()]);
    PlannedObligation {
        obligation: Obligation {
            id: ObligationId(derived_uuid(&[
                "obligation",
                &snapshot.0.to_string(),
                &rule.id,
                &version,
            ])),
            rule_id: rule.id.clone(),
            rule_version: rule.version,
            question: rule.statement.trim().to_owned(),
            required: rule.required,
            risk_level: risk,
            review_unit_ids: matched.iter().map(|u| u.id).collect(),
            code_refs: Vec::new(),
            applicability,
            acceptance_criterion: criteria.join("; "),
            acceptable_evidence_kinds: rule
                .acceptance
                .as_ref()
                .map(|a| a.evidence_kinds.clone())
                .unwrap_or_default(),
            status,
            evidence_ids: Vec::new(),
            limitations: Vec::new(),
            waivers: Vec::new(),
            evaluation_mode: None,
        },
        rule_title: rule.title.clone(),
        criteria,
        severity_on_violation: rule.severity_on_violation.map(Severity::from),
        exceptions: exceptions(rule, matched),
        contracts: Vec::new(),
        about_tests: false,
    }
}

/// Obligations of the repository's invariant rules, then of the activated generic rules.
fn obligations(
    snapshot: &SourceSnapshotId,
    policy: &RepositoryPolicy,
    units: &[(ReviewUnit, Vec<&FileChange>)],
    facts: &RunFacts,
    incomplete: bool,
) -> Vec<PlannedObligation> {
    let repository = policy
        .rules
        .iter()
        .filter(|r| r.kind == RuleKind::Invariant)
        .map(|r| {
            let (matched, links) = matched_units(r, units, facts);
            let mut planned = obligation(snapshot, r, &matched, incomplete);
            planned.contracts = r
                .contract_refs
                .iter()
                .filter_map(|path| {
                    let text = policy.contract_texts.get(path)?;
                    Some((
                        path.clone(),
                        policy.contracts.get(path)?.clone(),
                        text.clone(),
                    ))
                })
                .collect();
            planned.obligation.limitations.extend(
                links
                    .into_iter()
                    .map(|l| format!("applies through a semantic link: {l}")),
            );
            planned
        });
    let matrix = deploy::matrix(policy.deployment.as_ref());
    let matrix_evidence = TransitionMatrix::evidence_id(snapshot);
    let plans: Vec<_> = facts
        .ci
        .iter()
        .filter_map(|r| Some((r.terraform.as_ref()?, r.head_bound())))
        .collect();
    let rendered: Vec<_> = facts
        .ci
        .iter()
        .filter_map(|r| Some((r.kubernetes.as_ref()?, r.head_bound())))
        .collect();
    let generic = generic::pack().into_iter().filter(|g| {
        policy.config.review.profiles.contains(&g.profile)
            && !policy.rules.iter().any(|r| r.id == g.rule.id)
    });
    let activated = generic.filter_map(|g| {
        let matched: Vec<&ReviewUnit> = units
            .iter()
            .filter(|(_, files)| generic::activated(g.trigger, files, &policy.effects, facts))
            .map(|(unit, _)| unit)
            .collect();
        (!matched.is_empty()).then(|| {
            let mut planned = obligation(snapshot, &g.rule, &matched, false);
            planned.about_tests = g.rule.id == generic::TEST_INTEGRITY;
            migration::bind(&mut planned, &matrix, &facts.schema_refs, matrix_evidence);
            terraform::bind(&mut planned, &plans);
            kubernetes::bind(&mut planned, &rendered);
            planned
        })
    });
    repository.chain(activated).collect()
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
    let mut by_dir: BTreeMap<String, Vec<&FileChange>> = BTreeMap::new();
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

        by_dir.entry(unit_title(&side.path)).or_default().push(file);
    }
    let limit_reached = unreviewed
        .iter()
        .any(|g| g.reason == LIMIT_MAX_SOURCE_FILES);
    let units: Vec<(ReviewUnit, Vec<&FileChange>)> = by_dir
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
    let obligations: Vec<PlannedObligation> = if units.is_empty() {
        Vec::new()
    } else {
        obligations(
            snapshot,
            policy,
            &units,
            facts,
            !manifest.omissions.is_empty(),
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
        unreviewed,
        omissions: manifest.omissions.clone(),
        limit_reached,
    }
}
