//! Size-cap accounting, obligation links and gap extraction for [`super::Handoff`].

use crate::{
    Coverage, Finding, FindingId, ObligationStatus, ReportAnchor, ReportExcerpt, ReportFinding,
    ReportView, ReviewResult, RunId, redact,
};

use super::{
    HandoffFinding, HandoffOmission, MAX_BRIEF_CODE_BYTES, MAX_FINDING_DIFF_BYTES,
    MAX_FINDING_EXCERPT_BYTES, MAX_FINDING_PATCH_BYTES, Requirement,
};

fn clean(text: &str) -> String {
    redact(text).into_owned()
}

pub(super) fn gaps(result: &ReviewResult, view: &ReportView) -> Vec<String> {
    let mut out = Vec::new();
    if result.coverage != Coverage::CompleteForScope {
        out.push(format!(
            "coverage is {:?}, not complete for the scope",
            result.coverage
        ));
    }

    for unknown in &view.gaps.unknown_obligations {
        if unknown.required {
            out.push(format!(
                "{} UNKNOWN ({}); needs: {}",
                unknown.rule_id, unknown.reason_code, unknown.next_evidence_needed
            ));
        }
    }
    out.extend(
        view.gaps
            .paths
            .iter()
            .map(|g| format!("{} not reviewed: {}", g.path, g.reason)),
    );
    out.extend(
        view.gaps
            .open_questions
            .iter()
            .map(|q| format!("open question: {q}")),
    );
    out
}

fn cap_lines(text: &str, limit: usize) -> (Option<String>, usize, bool) {
    if text.len() <= limit {
        return (Some(text.to_owned()), text.len(), false);
    }

    if limit == 0 {
        return (None, 0, !text.is_empty());
    }

    let mut kept = String::new();
    for line in text.split_inclusive('\n') {
        if kept.len() + line.len() <= limit {
            kept.push_str(line);
        } else {
            if kept.is_empty() {
                let end = line.floor_char_boundary(limit);
                if end > 0 {
                    kept.push_str(&line[..end]);
                }
            }

            break;
        }
    }
    let used = kept.len();
    ((!kept.is_empty()).then_some(kept), used, true)
}

pub(super) fn requirements_of(result: &ReviewResult, raw: &Finding) -> Vec<Requirement> {
    result
        .obligations
        .iter()
        .filter(|o| raw.obligation_ids.contains(&o.id) || raw.rule_id.as_ref() == Some(&o.rule_id))
        .map(|o| Requirement {
            rule_id: clean(&o.rule_id),
            rule_version: o.rule_version,
            question: clean(&o.question),
            acceptance_criterion: clean(&o.acceptance_criterion),
            status: match &o.status {
                ObligationStatus::Unknown {
                    reason_code,
                    next_evidence_needed,
                } => ObligationStatus::Unknown {
                    reason_code: clean(reason_code),
                    next_evidence_needed: clean(next_evidence_needed),
                },
                ObligationStatus::NotApplicable { basis } => ObligationStatus::NotApplicable {
                    basis: clean(basis),
                },
                other => other.clone(),
            },
        })
        .collect()
}

pub(super) struct CapState {
    brief_remaining: usize,
    pub(super) omitted: Vec<HandoffOmission>,
}

impl CapState {
    pub(super) fn new() -> Self {
        Self {
            brief_remaining: MAX_BRIEF_CODE_BYTES,
            omitted: Vec::new(),
        }
    }

    fn cap_anchors(
        &mut self,
        run_id: RunId,
        finding_id: FindingId,
        source_anchors: &[ReportAnchor],
    ) -> (Vec<ReportAnchor>, bool) {
        let mut excerpt_remaining = MAX_FINDING_EXCERPT_BYTES;
        let mut diff_remaining = MAX_FINDING_DIFF_BYTES;
        let mut cut_any = false;
        let mut anchors = Vec::with_capacity(source_anchors.len());
        for anchor in source_anchors {
            let excerpt = anchor.excerpt.as_ref().and_then(|ex| {
                let limit = excerpt_remaining.min(self.brief_remaining);
                let (kept, used, cut) = cap_lines(&ex.text, limit);
                excerpt_remaining = excerpt_remaining.saturating_sub(used);
                self.brief_remaining = self.brief_remaining.saturating_sub(used);
                if cut {
                    cut_any = true;
                    self.omitted.push(HandoffOmission {
                        finding_id: Some(finding_id),
                        what: format!(
                            "source excerpt at {}:{}-{}",
                            anchor.reference.path,
                            anchor.reference.start_line,
                            anchor.reference.end_line
                        ),
                        command: format!("sych show --run {run_id} --format markdown"),
                    });
                }

                kept.map(|text| ReportExcerpt {
                    first_line: ex.first_line,
                    text,
                })
            });
            let diff = anchor.diff.as_ref().and_then(|d| {
                let limit = diff_remaining.min(self.brief_remaining);
                let (kept, used, cut) = cap_lines(d, limit);
                diff_remaining = diff_remaining.saturating_sub(used);
                self.brief_remaining = self.brief_remaining.saturating_sub(used);
                if cut {
                    cut_any = true;
                    self.omitted.push(HandoffOmission {
                        finding_id: Some(finding_id),
                        what: format!(
                            "change hunk at {}:{}-{}",
                            anchor.reference.path,
                            anchor.reference.start_line,
                            anchor.reference.end_line
                        ),
                        command: format!("sych show --run {run_id} --format markdown"),
                    });
                }

                kept
            });
            anchors.push(ReportAnchor {
                reference: anchor.reference.clone(),
                excerpt,
                diff,
            });
        }
        (anchors, cut_any)
    }

    pub(super) fn cap_finding(
        &mut self,
        run_id: RunId,
        raw: &Finding,
        view_finding: &ReportFinding,
        requirements: Vec<Requirement>,
    ) -> HandoffFinding {
        let (anchors, mut cut_any) =
            self.cap_anchors(run_id, view_finding.id, &view_finding.anchors);
        let mut fix_note = view_finding.fix_note.clone();
        let suggested_patch = view_finding.suggested_patch.as_ref().and_then(|patch| {
            let limit = MAX_FINDING_PATCH_BYTES.min(self.brief_remaining);
            if patch.len() <= limit {
                self.brief_remaining = self.brief_remaining.saturating_sub(patch.len());
                Some(patch.clone())
            } else {
                cut_any = true;
                if fix_note.is_none() {
                    fix_note = Some("Suggested patch omitted to fit the brief size limit.".into());
                }

                self.omitted.push(HandoffOmission {
                    finding_id: Some(view_finding.id),
                    what: format!("suggested patch ({} bytes)", patch.len()),
                    command: format!("sych show --run {run_id} --fixes-patch fixes.patch"),
                });
                None
            }
        });

        if cut_any
            && let Some(rule_or_ob) = raw
                .rule_id
                .as_deref()
                .map(str::to_owned)
                .or_else(|| raw.obligation_ids.first().map(ToString::to_string))
                .filter(|key| result_mentions_obligation(&requirements, raw, key))
        {
            self.omitted.push(HandoffOmission {
                finding_id: Some(view_finding.id),
                what: format!("obligation evidence and context packet for {rule_or_ob}"),
                command: format!(
                    "sych explain --run {run_id} --obligation {rule_or_ob} --format markdown"
                ),
            });
        }

        HandoffFinding {
            id: view_finding.id,
            title: view_finding.title.clone(),
            category: view_finding.category,
            severity: view_finding.severity,
            lifecycle: view_finding.lifecycle,
            support: view_finding.support,
            rule_id: view_finding.rule_id.clone(),
            explanation: view_finding.explanation.clone(),
            triggering_conditions: view_finding.triggering_conditions.clone(),
            consequence: view_finding.consequence.clone(),
            anchors,
            also: view_finding.also.clone(),
            evidence_ids: raw.evidence_ids.clone(),
            requirements,
            suggested_action: view_finding.suggested_action.clone(),
            remediation: view_finding.remediation.clone(),
            suggested_patch,
            fix_note,
            fix_criterion: view_finding.validation_expectation.clone(),
            limitations: view_finding.limitations.clone(),
        }
    }
}

fn result_mentions_obligation(requirements: &[Requirement], raw: &Finding, key: &str) -> bool {
    !requirements.is_empty() || raw.obligation_ids.iter().any(|id| id.to_string() == key)
}
