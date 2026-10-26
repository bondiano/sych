//! Observations beyond semantic neighbours: imports, risky and contract syntax shapes on
//! changed lines, project hook definitions and CI diagnostics of the tested head.

use super::build::{Candidate, file_for};
use super::{Priority, UNSEEN_GAPS};
use crate::effects::{EffectLedger, EffectMatch, LEDGER_GAPS, Resolution};
use crate::{
    BackendStatus, CiFacts, CodeRef, Completeness, DiffManifest, LintFacts, RepoPath, SourceSide,
    SyntaxFact, SyntaxFacts, SyntaxKind,
};

/// `subject` of a Python import that neither the snapshot nor the standard library has.
pub const UNRESOLVED_IMPORT: &str = "unresolved";

/// Ledger entries of the unit's file: an effect that is not preserved refutes, its other
/// side supports; an unresolved effect or a registry guarantee whose implementation changed
/// is a `LEDGER_GAPS` gap.
pub(super) fn ledger(
    ledger: &EffectLedger,
    path: &RepoPath,
    manifest: &DiffManifest,
    out: &mut Vec<Candidate>,
    gaps: &mut Vec<String>,
) {
    let Some(file) = file_for(manifest, path) else {
        return;
    };
    let names: Vec<&RepoPath> = file.old.iter().chain(&file.new).map(|s| &s.path).collect();
    let evidence = ledger.evidence_id();
    for entry in ledger.entries.iter().filter(|e| names.contains(&&e.path)) {
        let matched = serde_json::to_value(entry.matched).unwrap_or_default();
        let summary = format!(
            "effect ledger {} {} `{}` ({}){}",
            matched.as_str().unwrap_or_default(),
            serde_json::to_value(entry.kind)
                .unwrap_or_default()
                .as_str()
                .unwrap_or_default(),
            entry.target,
            serde_json::to_value(entry.resolution)
                .unwrap_or_default()
                .as_str()
                .unwrap_or_default(),
            if entry.notes.is_empty() {
                String::new()
            } else {
                format!(": {}", entry.notes.join("; "))
            }
        );
        let priority = match entry.matched {
            EffectMatch::Preserved | EffectMatch::Added => Priority::Supporting,
            _ => Priority::Refuting,
        };
        for o in entry.base.iter().chain(&entry.head) {
            out.push(Candidate {
                side: o.code_ref.side,
                path: o.code_ref.path.clone(),
                start: o.code_ref.start_line,
                end: o.code_ref.end_line,
                priority,
                reason: summary.clone(),
                evidence,
                completeness: Completeness::Partial,
                link: None,
            });
        }
        if entry.resolution == Resolution::DynamicUnknown {
            push_gap(
                gaps,
                format!("{}: {} — {}", LEDGER_GAPS[0], entry.effect_id, summary),
            );
        }
    }
    for binding in ledger
        .registry
        .iter()
        .filter(|b| !b.carries_guarantees())
        .filter(|b| b.implementation.iter().any(|i| names.contains(&&i.path)))
    {
        push_gap(
            gaps,
            format!(
                "{}: registry `{}` ({}) implementation changed; its guarantees are not carried \
                 and must be established from the current code",
                LEDGER_GAPS[1], binding.registry_id, binding.symbol
            ),
        );
    }
}

/// Linter findings on the head's changed lines refute; a linter that could not run on the
/// file is a gap, so its silence is not read as a clean result.
pub(super) fn lint(
    lint: &[LintFacts],
    path: &RepoPath,
    manifest: &DiffManifest,
    out: &mut Vec<Candidate>,
    gaps: &mut Vec<String>,
) {
    let Some(file) = file_for(manifest, path) else {
        return;
    };
    let Some(side) = file.side(SourceSide::Head) else {
        return;
    };
    let ranges = file.changed_lines(SourceSide::Head);
    for facts in lint
        .iter()
        .filter(|f| f.files.iter().any(|p| p == side.path.as_str()))
    {
        if facts.status != BackendStatus::Ready {
            push_gap(
                gaps,
                format!(
                    "LINT_UNAVAILABLE: {} for {}: {}",
                    facts.producer.name,
                    side.path,
                    facts.limitations.last().map_or("", String::as_str)
                ),
            );
            continue;
        }

        for d in &facts.diagnostics {
            let Some(r) = d.code_ref.as_ref().filter(|r| r.path == side.path) else {
                continue;
            };
            if !ranges.iter().any(|&(s, e)| (s..=e).contains(&r.start_line)) {
                continue;
            }

            let message: String = d.message.chars().take(120).collect();
            out.push(Candidate {
                side: r.side,
                path: r.path.clone(),
                start: r.start_line,
                end: r.end_line,
                priority: Priority::Refuting,
                reason: format!(
                    "lint {} {} {}: {message}",
                    facts.producer.name,
                    d.level,
                    d.code.as_deref().unwrap_or("-")
                ),
                evidence: Some(facts.evidence_id()),
                completeness: Completeness::Partial,
                link: None,
            });
        }
    }
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if !gaps.contains(&gap) {
        gaps.push(gap);
    }
}

/// Risky shapes on changed lines refute; contract shapes and definitions of the project hooks
/// they call support.
pub(super) fn syntax(
    syntax: &[SyntaxFacts],
    path: &RepoPath,
    manifest: &DiffManifest,
    out: &mut Vec<Candidate>,
) {
    let Some(file) = file_for(manifest, path) else {
        return;
    };
    for doc in syntax {
        let Some(side_path) = file.side(doc.side).map(|s| &s.path) else {
            continue;
        };
        imports(doc, side_path, out);
        for fact in doc
            .facts
            .iter()
            .filter(|f| &f.code_ref.path == side_path && f.in_change)
        {
            let kind = serde_json::to_value(fact.kind).unwrap_or_default();
            let kind = kind.as_str().unwrap_or_default();
            let mut push = |r: &CodeRef, priority, reason: String| {
                out.push(Candidate {
                    side: r.side,
                    path: r.path.clone(),
                    start: r.start_line,
                    end: r.end_line,
                    priority,
                    reason,
                    evidence: Some(doc.evidence_id()),
                    completeness: Completeness::Partial,
                    link: None,
                });
            };
            if fact.kind.is_risk() {
                push(
                    &fact.code_ref,
                    Priority::Refuting,
                    format!("syntax {kind}: {}", fact.detail),
                );
            } else if fact.kind.is_contract() {
                push(
                    &fact.code_ref,
                    Priority::Supporting,
                    format!("syntax {kind}: {}", fact.detail),
                );
            }

            for r in &fact.related {
                let reason = if fact.kind == SyntaxKind::DebugFormatted {
                    format!("definition of the type behind {}", fact.detail)
                } else {
                    format!("definition of project hook `{}`", fact.detail)
                };
                push(r, Priority::Supporting, reason);
            }
        }
    }
}

/// Changed code the snapshot does not show the behaviour of, one gap per fact (`UNSEEN_GAPS`).
pub(super) fn unseen(
    syntax: &[SyntaxFacts],
    path: &RepoPath,
    manifest: &DiffManifest,
    gaps: &mut Vec<String>,
) {
    let Some(file) = file_for(manifest, path) else {
        return;
    };
    for doc in syntax {
        let Some(side_path) = file.side(doc.side).map(|s| &s.path) else {
            continue;
        };
        for (fact, code) in doc
            .facts
            .iter()
            .filter(|f| &f.code_ref.path == side_path)
            .filter_map(|f| unseen_code(f).map(|code| (f, code)))
        {
            let r = &fact.code_ref;
            let gap = format!(
                "{code}: {}:{}-{} ({:?}): {}",
                r.path, r.start_line, r.end_line, r.side, fact.detail
            );
            if !gaps.contains(&gap) {
                gaps.push(gap);
            }
        }
    }
}

/// The `UNSEEN_GAPS` code of a changed fact whose behaviour the snapshot does not show.
fn unseen_code(fact: &SyntaxFact) -> Option<&'static str> {
    if !fact.in_change {
        return None;
    }

    match fact.kind {
        SyntaxKind::InactiveCfg => Some(UNSEEN_GAPS[0]),
        SyntaxKind::UnexpandedMacro => Some(UNSEEN_GAPS[1]),
        SyntaxKind::DebugFormatted if fact.related.is_empty() => Some(UNSEEN_GAPS[2]),
        SyntaxKind::DynamicDispatch | SyntaxKind::MonkeyPatch | SyntaxKind::RuntimeRegistration => {
            Some(UNSEEN_GAPS[3])
        }
        SyntaxKind::Import if fact.subject.as_deref() == Some(UNRESOLVED_IMPORT) => {
            Some(UNSEEN_GAPS[4])
        }
        _ => None,
    }
}

/// Changed spans whose behaviour the snapshot does not show, as the `UNSEEN_GAPS` name them.
pub fn unseen_spans(syntax: &[SyntaxFacts]) -> Vec<&CodeRef> {
    syntax
        .iter()
        .flat_map(|d| &d.facts)
        .filter(|f| unseen_code(f).is_some())
        .map(|f| &f.code_ref)
        .collect()
}

/// The file's imports as supporting context, one span per block of adjacent imports.
fn imports(doc: &SyntaxFacts, path: &RepoPath, out: &mut Vec<Candidate>) {
    let mut blocks: Vec<(&CodeRef, u32, usize)> = Vec::new();
    let found = doc
        .facts
        .iter()
        .filter(|f| f.kind == SyntaxKind::Import && &f.code_ref.path == path);
    for fact in found {
        let r = &fact.code_ref;
        match blocks.last_mut() {
            // A blank line or comment between imports still reads as one block.
            Some((_, end, count)) if r.start_line <= *end + 2 => {
                *end = (*end).max(r.end_line);
                *count += 1;
            }
            _ => blocks.push((r, r.end_line, 1)),
        }
    }
    for (first, end, count) in blocks {
        out.push(Candidate {
            side: first.side,
            path: first.path.clone(),
            start: first.start_line,
            end,
            priority: Priority::Supporting,
            reason: format!("imports ({count} statements)"),
            evidence: Some(doc.evidence_id()),
            completeness: Completeness::Partial,
            link: None,
        });
    }
}

/// CI errors and warnings mapped onto the head refute: they come from the real build of the
/// tested commit, though locally imported reports stay author claims.
pub(super) fn ci(
    ci: &[CiFacts],
    path: &RepoPath,
    manifest: &DiffManifest,
    out: &mut Vec<Candidate>,
) {
    let Some(file) = file_for(manifest, path) else {
        return;
    };
    let Some(head) = file.side(SourceSide::Head) else {
        return;
    };
    let ranges = file.changed_lines(SourceSide::Head);
    let changed = |line: u32| ranges.iter().any(|&(s, e)| (s..=e).contains(&line));
    for report in ci {
        for d in &report.diagnostics {
            let Some(r) = d.code_ref.as_ref().filter(|r| r.path == head.path) else {
                continue;
            };
            let message: String = d.message.chars().take(120).collect();
            out.push(Candidate {
                side: r.side,
                path: r.path.clone(),
                start: r.start_line,
                end: r.end_line,
                // Only findings on changed lines can refute the change itself.
                priority: if changed(r.start_line) {
                    Priority::Refuting
                } else {
                    Priority::Supporting
                },
                reason: format!(
                    "CI {} {}: {message}",
                    d.level,
                    d.code.as_deref().unwrap_or("-")
                ),
                evidence: Some(report.evidence_id()),
                completeness: Completeness::Partial,
                link: None,
            });
        }
    }
}
