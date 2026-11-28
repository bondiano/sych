//! Rule and preference matching over review units and semantic facts.

use crate::config::{RepositoryPolicy, Rule, RuleException, RuleKind, RuleSeverity};
use crate::deploy::TransitionMatrix;
use crate::generic;
use crate::{
    Applicability, FileChange, NOT_ASSESSED, Obligation, ObligationId, ObligationStatus, RepoPath,
    ReviewUnit, RiskLevel, RunFacts, Severity, Sha256Digest, SourceSnapshotId, derived_uuid,
    glob_match,
};
use crate::{deploy, kubernetes, migration, terraform};

use super::{PlannedObligation, PlannedPreference, paths};

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

fn contracts_of(rule: &Rule, policy: &RepositoryPolicy) -> Vec<(RepoPath, Sha256Digest, String)> {
    rule.contract_refs
        .iter()
        .filter_map(|path| {
            let text = policy.contract_texts.get(path)?;
            Some((
                path.clone(),
                policy.contracts.get(path)?.clone(),
                text.clone(),
            ))
        })
        .collect()
}

/// Repository preferences matched to changed or semantically linked units.
pub(super) fn preferences(
    policy: &RepositoryPolicy,
    units: &[(ReviewUnit, Vec<&FileChange>)],
    facts: &RunFacts,
) -> Vec<PlannedPreference> {
    policy
        .rules
        .iter()
        .filter(|r| r.kind == RuleKind::Preference)
        .filter_map(|r| {
            let (matched, links) = matched_units(r, units, facts);
            (!matched.is_empty()).then(|| PlannedPreference {
                rule_id: r.id.clone(),
                rule_version: r.version,
                rule_title: r.title.clone(),
                statement: r.statement.trim().to_owned(),
                rationale: r
                    .rationale
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                review_unit_ids: matched.iter().map(|u| u.id).collect(),
                contracts: contracts_of(r, policy),
                exceptions: r.exceptions.clone(),
                limitations: links
                    .into_iter()
                    .map(|l| format!("applies through a semantic link: {l}"))
                    .collect(),
            })
        })
        .collect()
}

/// Obligations of the repository's invariant rules, then of the activated generic rules.
pub(super) fn obligations(
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
            planned.contracts = contracts_of(r, policy);
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
