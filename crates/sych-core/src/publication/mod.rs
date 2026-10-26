//! What an advisory publication shows (spec §18.1–18.2, §19.4): provider anchors of verified
//! findings, the inline plan of one revision and the bodies. Pure: the publisher renders from
//! the immutable result and the diff manifest, never from model prose.

mod summary;

use std::fmt::Write;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DiffManifest, Finding, FindingCategory, ReviewResult, SourceSide};

pub use summary::{SummaryContext, render_summary};

/// New inline comments one revision may add (spec §18.2).
pub const MAX_INLINE: usize = 8;
/// Unchanged lines around a hunk GitHub shows and accepts comments on.
pub const DIFF_CONTEXT: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiffSide {
    /// The diff base (old) side.
    Left,
    /// The head (new) side.
    Right,
}

/// A GitHub review-comment position by explicit commit, path, line and side; never the
/// deprecated `position`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InlineAnchor {
    pub commit_id: String,
    pub path: String,
    pub line: u32,
    pub side: DiffSide,
    pub start_line: Option<u32>,
    pub start_side: Option<DiffSide>,
}

/// Where a finding is shown, with the reason when it is not inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "placement", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Placement {
    Inline { anchor: InlineAnchor },
    Summary { reason: String },
}

fn inside(line: u32, ranges: impl Iterator<Item = (u32, u32)>, count: Option<u32>) -> bool {
    ranges.into_iter().any(|(start, lines)| {
        let first = start.saturating_sub(DIFF_CONTEXT).max(1);
        let last = (start + lines.max(1) - 1 + DIFF_CONTEXT).min(count.unwrap_or(u32::MAX));
        (first..=last).contains(&line)
    })
}

/// The provider anchor of the finding's first anchor, or why it stays in the summary: another
/// blob, a target-tip line or a line outside the shown diff is never moved to a neighbour.
pub fn anchor(finding: &Finding, manifest: &DiffManifest) -> Placement {
    let summary = |reason: &str| Placement::Summary {
        reason: reason.into(),
    };
    let Some(r) = finding.anchors.first() else {
        return summary("no source anchor");
    };
    let side = match r.side {
        SourceSide::Head => DiffSide::Right,
        SourceSide::DiffBase => DiffSide::Left,
        SourceSide::TargetTip => return summary("anchored on the target tip, not in the diff"),
    };
    let file = manifest.files.iter().find(|f| match side {
        DiffSide::Right => f.new.as_ref().is_some_and(|n| n.path == r.path),
        DiffSide::Left => f.old.as_ref().is_some_and(|o| o.path == r.path),
    });
    let Some(file) = file else {
        return summary("file is not in the diff");
    };
    let shown = match side {
        DiffSide::Right => file.new.as_ref(),
        DiffSide::Left => file.old.as_ref(),
    };
    let Some(shown) = shown.filter(|s| s.oid == r.blob_oid) else {
        return summary("anchor names another blob than the diff");
    };
    let hunks = file.hunks.iter().map(|h| match side {
        DiffSide::Right => (h.new_start, h.new_lines),
        DiffSide::Left => (h.old_start, h.old_lines),
    });
    let in_diff = |line| inside(line, hunks.clone(), shown.line_count);
    if !in_diff(r.start_line) || !in_diff(r.end_line) {
        return summary("line is outside the diff GitHub shows");
    }

    // GitHub names the file by its head path; a deleted file only has the old one.
    let path = file.new.as_ref().unwrap_or(shown).path.as_str().to_owned();
    let range = r.start_line < r.end_line;
    Placement::Inline {
        anchor: InlineAnchor {
            commit_id: manifest.head.hex().to_owned(),
            path,
            line: r.end_line,
            side,
            start_line: range.then_some(r.start_line),
            start_side: range.then_some(side),
        },
    }
}

/// One finding of the revision with where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Planned {
    pub fingerprint: String,
    pub placement: Placement,
    /// Inline in an earlier revision; does not count against the limit.
    pub already_inline: bool,
}

/// Defects and rule violations go inline by severity, at most [`MAX_INLINE`] new ones; nits and
/// improvements stay in the summary; an incomplete provider diff keeps everything there, and a
/// position the provider `rejected` is shown in the summary with that reason.
pub fn plan(
    result: &ReviewResult,
    manifest: &DiffManifest,
    already_inline: &[String],
    rejected: &[String],
    provider_diff_complete: bool,
) -> Vec<Planned> {
    let mut findings: Vec<&Finding> = result.findings.iter().collect();
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    let mut new_inline = 0;
    findings
        .into_iter()
        .map(|f| {
            let fingerprint = f.fingerprint.to_string();
            let already = already_inline.contains(&fingerprint);
            let placement = match (f.category, anchor(f, manifest)) {
                (FindingCategory::Nit | FindingCategory::Improvement, _) => Placement::Summary {
                    reason: "optional; summary only".into(),
                },
                _ if !provider_diff_complete => Placement::Summary {
                    reason: "provider diff is incomplete".into(),
                },
                _ if rejected.contains(&fingerprint) => Placement::Summary {
                    reason: "the provider rejected the inline position".into(),
                },
                (_, Placement::Inline { anchor }) if already || new_inline < MAX_INLINE => {
                    new_inline += usize::from(!already);
                    Placement::Inline { anchor }
                }
                (_, Placement::Inline { .. }) => Placement::Summary {
                    reason: format!("over the {MAX_INLINE} new inline comments of a revision"),
                },
                (_, summary) => summary,
            };
            Planned {
                fingerprint,
                placement,
                already_inline: already,
            }
        })
        .collect()
}

/// The body of an inline comment; the marker finds it again.
pub fn inline_body(finding: &Finding) -> String {
    let mut out = format!(
        "<!-- sych:inline:{} -->\n**{:?} · {:?}** {}\n\n{}\n",
        finding.fingerprint, finding.severity, finding.category, finding.title, finding.explanation
    );
    if !finding.triggering_conditions.is_empty() {
        let _ = writeln!(out, "\nWhen: {}", finding.triggering_conditions.join("; "));
    }

    if !finding.consequence.is_empty() {
        let _ = writeln!(out, "\nConsequence: {}", finding.consequence);
    }

    if !finding.suggested_action.is_empty() {
        let _ = writeln!(out, "\nSuggested: {}", finding.suggested_action);
    }

    out.push_str("\n_Advisory finding by Sych; not a merge gate._\n");
    out
}
