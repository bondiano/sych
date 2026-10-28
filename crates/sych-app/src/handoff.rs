//! `sych handoff`: selected findings of a saved run as a package for a separate author workflow
//! (spec §21.7, roadmap M5.4). It reads the stored result only: no model, no source edit, no
//! change to the result, and a fixed head is reviewed again rather than marked resolved.

use std::fmt::Write as _;
use std::process::ExitCode;

use sych_core::handoff::{self, Handoff};
use sych_core::{FindingId, ObligationStatus, SourceSnapshotId};

use crate::args::HandoffArguments;
use crate::errors::Failure;
use crate::output::emit_as;
use crate::render::{md, tag};
use crate::storage::saved_report;

fn status(s: &ObligationStatus) -> &'static str {
    match s {
        ObligationStatus::Supported => "SUPPORTED",
        ObligationStatus::Violated => "VIOLATED",
        ObligationStatus::Unknown { .. } => "UNKNOWN",
        ObligationStatus::NotApplicable { .. } => "NOT_APPLICABLE",
    }
}

/// The package as Markdown; review and repository text is escaped, never live Markdown.
pub fn markdown(h: &Handoff) -> String {
    let mut text = format!(
        "# Fix handoff ({})\n\nRun `{}`, result `{}` (revision {}), snapshot `{}`; coverage {}.\n\n## Procedure\n\n",
        h.format,
        h.run_id,
        h.result_id,
        h.review_revision,
        h.source_snapshot_id.0,
        tag(&h.coverage)
    );
    for (i, step) in h.procedure.iter().enumerate() {
        let _ = writeln!(text, "{}. {step}", i + 1);
    }
    for f in &h.findings {
        let _ = writeln!(
            text,
            "\n## Finding `{}`: {}\n\n{} · {} · {}\n\n{}\n\n**Consequence:** {}\n",
            f.id,
            md(&f.title),
            tag(&f.severity),
            tag(&f.category),
            tag(&f.support),
            md(&f.explanation),
            md(&f.consequence)
        );
        for c in &f.triggering_conditions {
            let _ = writeln!(text, "- when: {}", md(c));
        }
        for r in &f.anchors {
            let _ = writeln!(text, "- at: `{}:{}-{}`", r.path, r.start_line, r.end_line);
        }
        for r in &f.requirements {
            let _ = writeln!(
                text,
                "- requirement {} v{} ({}): {}",
                md(&r.rule_id),
                r.rule_version,
                status(&r.status),
                md(&r.acceptance_criterion)
            );
        }
        for e in &f.evidence_ids {
            let _ = writeln!(text, "- evidence: `{e}`");
        }
        let _ = writeln!(
            text,
            "\n**Fix criterion:** {}\n\n**Suggested action:** {}",
            md(&f.fix_criterion),
            md(&f.suggested_action)
        );
        for l in &f.limitations {
            let _ = writeln!(text, "- limitation: {}", md(l));
        }
    }
    if !h.gaps.is_empty() {
        let _ = writeln!(text, "\n## Gaps a fix does not settle\n");
        for g in &h.gaps {
            let _ = writeln!(text, "- {}", md(g));
        }
    }

    text
}

pub fn run(args: &HandoffArguments) -> anyhow::Result<ExitCode> {
    let ids = args
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
    let report = saved_report(args.config.as_deref(), &args.run)?;
    let package = handoff::package(&report.review_result, &ids, expected.as_ref())
        .map_err(Failure::invalid)?;
    emit_as(&package, || markdown(&package), &args.out)?;
    Ok(ExitCode::SUCCESS)
}
