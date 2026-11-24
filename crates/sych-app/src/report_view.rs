//! Source and patches for a report, reconstructed from its pinned Git objects.

use std::collections::BTreeMap;
use std::path::Path;

use sych_core::architecture;
use sych_core::scan::{HIDDEN_TEXT, SECRET, SUPPLY_CHAIN};
use sych_core::{Redactor, ReportView, Support, ordered_findings};

use crate::render::Report;
use crate::snapshot;

pub(crate) mod markdown;
mod patch;
mod source;

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
    .map(|f| (f.id, f.suggested_action))
    .collect();
    let mut view = ReportView::from_result(result, saved);
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
