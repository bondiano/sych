//! Source, patches and HTML rendering for a report, reconstructed from its pinned Git objects.

use std::collections::BTreeMap;
use std::path::Path;

use sych_core::architecture::{self, ArchitectureFacts};
use sych_core::scan::{HIDDEN_TEXT, SECRET, SUPPLY_CHAIN};
use sych_core::{
    Evidence, Redactor, ReportPolicy, ReportPolicyProvenance, ReportView, Support,
    neutralize_prose, ordered_findings,
};

use crate::render::{Report, tag};
use crate::snapshot;

pub mod html;
pub(crate) mod markdown;
mod patch;
mod source;

fn clean(redactor: &Redactor, text: &str) -> String {
    neutralize_prose(&redactor.redact(text))
}

fn clean_policy(report: &Report, redactor: &Redactor) -> ReportPolicy {
    let diff = &report.policy_review.diff;
    ReportPolicy {
        snapshot: report.policy.clone(),
        provenance: report
            .policy_review
            .provenance
            .iter()
            .map(|p| ReportPolicyProvenance {
                layer: tag(&p.layer),
                origin: clean(redactor, &p.origin),
                sha256: p.sha256.clone(),
            })
            .collect(),
        proposed_files: diff
            .files
            .iter()
            .map(|f| format!("{} {}", tag(&f.change), clean(redactor, f.path.as_str())))
            .collect(),
        proposed_rules: diff
            .rules
            .iter()
            .map(|r| format!("{} {}", clean(redactor, &r.id), tag(&r.change)))
            .collect(),
        head_errors: diff
            .head_errors
            .iter()
            .map(|e| clean(redactor, e))
            .collect(),
    }
}

fn clean_evidence(evidence: &[Evidence], redactor: &Redactor) -> Vec<Evidence> {
    evidence
        .iter()
        .map(|ev| {
            let mut copy = ev.clone();
            copy.producer.name = clean(redactor, &ev.producer.name);
            copy.producer.version = clean(redactor, &ev.producer.version);
            copy.limitations = ev.limitations.iter().map(|l| clean(redactor, l)).collect();
            copy
        })
        .collect()
}

fn clean_architecture(facts: &[ArchitectureFacts], redactor: &Redactor) -> Vec<ArchitectureFacts> {
    facts
        .iter()
        .map(|project| {
            let mut copy = project.clone();
            copy.project = clean(redactor, &project.project);
            copy.producer.name = clean(redactor, &project.producer.name);
            copy.producer.version = clean(redactor, &project.producer.version);
            copy.limitations = project
                .limitations
                .iter()
                .map(|l| clean(redactor, l))
                .collect();
            for cov in &mut copy.coverage {
                cov.reason = cov.reason.as_deref().map(|r| clean(redactor, r));
            }
            for f in &mut copy.findings {
                f.key = clean(redactor, &f.key);
                f.message = clean(redactor, &f.message);
                f.notes = f.notes.iter().map(|n| clean(redactor, n)).collect();
                f.picture = f.picture.iter().map(|p| clean(redactor, p)).collect();
            }
            for f in &mut copy.fixed {
                f.key = clean(redactor, &f.key);
                f.message = clean(redactor, &f.message);
            }
            copy
        })
        .collect()
}

/// Builds the renderable view and the raw combined patch. A missing snapshot leaves the
/// original result and every finding available, with source fields unset.
pub fn build(report: &Report, repo: &Path, redactor: &Redactor) -> (ReportView, String) {
    let result = &report.review_result;
    let saved = report.snapshot.as_ref();
    let snap = saved.and_then(|saved| snapshot::reopen(saved, repo).ok());
    let ordered = ordered_findings(&result.findings);
    let (combined, mut previews) = patch::fixes(snap.as_ref(), &ordered, redactor);
    let remediations: BTreeMap<_, _> = architecture::findings(
        &report.architecture_facts,
        &[],
        &|_| true,
        result.language,
        result.created_at,
    )
    .into_iter()
    .map(|f| (f.id, clean(redactor, &f.suggested_action)))
    .collect();
    let mut view = ReportView::from_result(result, saved);
    view.fixes_patch = (!combined.is_empty()).then(|| combined.clone());
    view.policy = Some(clean_policy(report, redactor));
    view.evidence = clean_evidence(&report.evidence, redactor);
    view.architecture = clean_architecture(&report.architecture_facts, redactor);
    for finding in &mut view.findings {
        if let Some(preview) = previews.remove(&finding.id) {
            finding.suggested_patch = preview.patch;
            finding.fix_note = preview.note;
        }

        finding.remediation = remediations.get(&finding.id).cloned().or_else(|| {
            let rule = finding.rule_id.as_deref()?;
            (finding.support == Support::ExplicitRule
                || (finding.support == Support::StaticTrace
                    && matches!(rule, SECRET | HIDDEN_TEXT | SUPPLY_CHAIN)))
            .then(|| finding.suggested_action.clone())
        });
        if finding.rule_id.as_deref() != Some(SECRET)
            && let Some(snap) = snap.as_ref()
        {
            for anchor in &mut finding.anchors {
                source::enrich(snap, anchor, redactor);
            }
        }
    }
    (view, combined)
}
