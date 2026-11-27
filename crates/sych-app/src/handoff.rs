//! `sych handoff`: selected findings of a saved run as a `handoff@2` fix brief for a coding
//! agent or separate author workflow. Reads the stored result and pinned Git blobs only: no model
//! call, no source edit, no mutation of stored rows and no publication.

use std::fmt::Write as _;
use std::process::ExitCode;

use sych_core::handoff::{self, Handoff, HandoffFinding, HandoffSelection};
use sych_core::publication::fence;
use sych_core::{FindingId, ObligationStatus, ReportAnchor, SourceSnapshotId};

use crate::args::HandoffArguments;
use crate::config;
use crate::errors::Failure;
use crate::output::emit_as;
use crate::render::{tag, uncommitted_label};
use crate::report_view;
use crate::storage::saved_report;

fn status(s: &ObligationStatus) -> &'static str {
    match s {
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::Unknown { .. } => "UNKNOWN",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}

/// Neutralizes terminal control codes and bidi overrides while keeping Markdown and code verbatim.
fn verbatim(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.replace("\r\n", "\n").chars() {
        if matches!(ch, '\n' | '\t') {
            out.push(ch);
        } else if ch.is_control()
            || matches!(
                ch,
                '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        {
            let _ = write!(out, "U+{:04X}", u32::from(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

fn fenced_block(out: &mut String, info: &str, body: &str) {
    let body = verbatim(body);
    let ticks = fence(&body);
    let _ = writeln!(out, "{ticks}{info}");
    out.push_str(&body);
    if !body.is_empty() && !body.ends_with('\n') {
        out.push('\n');
    }

    let _ = writeln!(out, "{ticks}");
}

fn inline_code(text: &str) -> String {
    let clean = verbatim(text).replace(['\n', '\t'], " ");
    let longest = clean.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let ticks = "`".repeat(longest + 1);
    if clean.starts_with('`')
        || clean.ends_with('`')
        || clean.starts_with(' ')
        || clean.ends_with(' ')
    {
        format!("{ticks} {clean} {ticks}")
    } else {
        format!("{ticks}{clean}{ticks}")
    }
}

fn write_anchor(out: &mut String, index: usize, anchor: &ReportAnchor) {
    let r = &anchor.reference;
    let _ = writeln!(
        out,
        "\n### Anchor {}: {}:{}-{} ({}, blob `{}`)\n",
        index + 1,
        inline_code(r.path.as_str()),
        r.start_line,
        r.end_line,
        tag(&r.side),
        r.blob_oid
    );
    if let Some(excerpt) = &anchor.excerpt {
        let mut numbered = String::new();
        for (offset, line_text) in excerpt.text.lines().enumerate() {
            let line = excerpt
                .first_line
                .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            let marker = if (r.start_line..=r.end_line).contains(&line) {
                '>'
            } else {
                ' '
            };
            let _ = writeln!(numbered, "{marker}{line:>5} | {line_text}");
        }
        fenced_block(out, "text", &numbered);
    } else {
        out.push_str("Source unavailable.\n");
    }

    if let Some(diff) = &anchor.diff {
        out.push_str("\nChange hunk:\n\n");
        fenced_block(out, "diff", diff);
    }
}

fn write_finding(out: &mut String, number: usize, f: &HandoffFinding) {
    let _ = writeln!(
        out,
        "\n## {}. [{} · {} · {}] Finding `{}`\n",
        number,
        tag(&f.severity),
        tag(&f.category),
        tag(&f.support),
        f.id
    );
    let mut details = String::new();
    let _ = writeln!(details, "Title: {}", f.title);
    if let Some(rule) = &f.rule_id {
        let _ = writeln!(details, "Rule: {rule}");
    }

    let _ = writeln!(details, "\nExplanation:\n{}", f.explanation);
    if !f.triggering_conditions.is_empty() {
        details.push_str("\nWhen:\n");
        for cond in &f.triggering_conditions {
            let _ = writeln!(details, "- {cond}");
        }
    }

    if !f.consequence.is_empty() {
        let _ = writeln!(details, "\nConsequence: {}", f.consequence);
    }

    if !f.also.is_empty() {
        details.push_str("\nAlso on these lines:\n");
        for also in &f.also {
            let where_at = also
                .anchors
                .first()
                .map(|r| format!(" ({}:{}-{})", r.path, r.start_line, r.end_line))
                .unwrap_or_default();
            let _ = writeln!(
                details,
                "- [{} · {}]{}: {}",
                tag(&also.severity),
                tag(&also.category),
                where_at,
                also.title
            );
            if !also.explanation.is_empty() {
                let _ = writeln!(details, "  {}", also.explanation);
            }

            for cond in &also.triggering_conditions {
                let _ = writeln!(details, "  when: {cond}");
            }
            if !also.consequence.is_empty() {
                let _ = writeln!(details, "  consequence: {}", also.consequence);
            }
        }
    }

    let _ = writeln!(details, "\nFix criterion: {}", f.fix_criterion);
    let _ = writeln!(details, "Suggested action: {}", f.suggested_action);
    if let Some(remediation) = f
        .remediation
        .as_ref()
        .filter(|rem| rem.as_str() != f.suggested_action)
    {
        let _ = writeln!(details, "Remediation: {remediation}");
    }

    if !f.limitations.is_empty() {
        details.push_str("\nLimitations:\n");
        for lim in &f.limitations {
            let _ = writeln!(details, "- {lim}");
        }
    }

    fenced_block(out, "text", &details);

    if !f.requirements.is_empty() {
        out.push_str("\nRequirements:\n\n");
        let mut reqs = String::new();
        for r in &f.requirements {
            let _ = writeln!(
                reqs,
                "- {} v{} ({}): {}",
                r.rule_id,
                r.rule_version,
                status(&r.status),
                r.acceptance_criterion
            );
        }
        fenced_block(out, "text", &reqs);
    }

    if !f.evidence_ids.is_empty() {
        out.push_str("\nEvidence IDs:");
        for e in &f.evidence_ids {
            let _ = write!(out, " `{e}`");
        }
        out.push('\n');
    }

    for (idx, anchor) in f.anchors.iter().enumerate() {
        write_anchor(out, idx, anchor);
    }

    if let Some(patch) = &f.suggested_patch {
        out.push_str("\n### Suggested change\n\n");
        fenced_block(out, "diff", patch);
    }

    if let Some(note) = &f.fix_note {
        out.push_str("\nFix note:\n\n");
        fenced_block(out, "text", note);
    }
}

/// Renders a `handoff@2` package as Markdown written for coding agents: repository code and model
/// prose sit inside dynamic-length fences without inline backslash escaping.
pub fn markdown(h: &Handoff) -> String {
    let mut text = format!(
        "# Fix brief ({})\n\n- Run: `{}`\n- Result: `{}` (revision {})\n- Snapshot: `{}`\n- Coverage: `{}`\n",
        h.format,
        h.run_id,
        h.result_id,
        h.review_revision,
        h.source_snapshot_id.0,
        tag(&h.coverage),
    );
    if let Some(head) = &h.head {
        let _ = write!(text, "- Head commit: `{head}`");
        if let Some(label) = uncommitted_label(h.head_kind) {
            let _ = write!(text, " ({label})");
        }

        text.push('\n');
    }

    if let Some(branch) = &h.head_branch {
        let _ = writeln!(text, "- Head branch: {}", inline_code(branch));
    }

    text.push_str("\n## Procedure\n\n");
    for (i, step) in h.procedure.iter().enumerate() {
        let _ = writeln!(text, "{}. {step}", i + 1);
    }
    if h.findings.is_empty() {
        text.push_str("\nNo open findings in this run.\n");
    } else {
        for (i, f) in h.findings.iter().enumerate() {
            write_finding(&mut text, i + 1, f);
        }
    }

    if !h.omitted.is_empty() {
        text.push_str("\n## Truncated material\n\n");
        for item in &h.omitted {
            let prefix = item
                .finding_id
                .map_or_else(|| "brief".to_owned(), |id| format!("finding `{id}`"));
            let _ = writeln!(
                text,
                "- {prefix}: {} -> inspect with {}",
                inline_code(&item.what),
                inline_code(&item.command)
            );
        }
    }

    if !h.gaps.is_empty() {
        text.push_str("\n## Gaps a fix does not settle\n\n");
        let mut gap_lines = String::new();
        for g in &h.gaps {
            let _ = writeln!(gap_lines, "- {g}");
        }
        fenced_block(&mut text, "text", &gap_lines);
    }

    text
}

pub fn run(args: &HandoffArguments) -> anyhow::Result<ExitCode> {
    let finding_ids = args
        .findings
        .iter()
        .map(|f| {
            f.parse()
                .map(FindingId)
                .map_err(|_| Failure::invalid(format!("`{f}` is not a finding id")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = args
        .snapshot
        .as_deref()
        .map(|s| {
            s.parse()
                .map(SourceSnapshotId)
                .map_err(|_| Failure::invalid(format!("`{s}` is not a snapshot id")))
        })
        .transpose()?;
    let selection = HandoffSelection {
        finding_ids,
        all_open: args.all,
        min_severity: args.min_severity.map(Into::into),
        categories: args.categories.iter().copied().map(Into::into).collect(),
    };
    let redactor = config::server(args.config.as_deref())?
        .redaction
        .redactor()
        .map_err(Failure::invalid)?;
    let report = saved_report(args.config.as_deref(), &args.run)?;
    let (view, _) = report_view::build(&report, &args.repo, &redactor);
    let package = handoff::package(&report.review_result, &view, &selection, expected.as_ref())
        .map_err(Failure::invalid)?;
    emit_as(&package, || markdown(&package), &args.out)?;
    Ok(ExitCode::SUCCESS)
}
