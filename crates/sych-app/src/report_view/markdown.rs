//! Markdown material for one finding, with code fences selected from the content.

use std::fmt::Write as _;

use sych_core::{ReportAnchor, ReportFinding};

use crate::render::md;

fn block(out: &mut String, kind: &str, text: &str) {
    let mut longest = 0;
    let mut run = 0;
    for byte in text.bytes() {
        if byte == b'`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    let _ = writeln!(out, "\n    {fence}{kind}");
    for line in text.lines() {
        let _ = writeln!(out, "    {line}");
    }
    let _ = writeln!(out, "    {fence}\n");
}

pub(crate) fn anchor(out: &mut String, a: &ReportAnchor) {
    let Some(excerpt) = &a.excerpt else {
        out.push_str("  - source unavailable\n");
        return;
    };
    let mut numbered = String::new();
    for (offset, text) in excerpt.text.lines().enumerate() {
        let line = excerpt
            .first_line
            .saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
        let marked = if (a.reference.start_line..=a.reference.end_line).contains(&line) {
            '>'
        } else {
            ' '
        };
        let _ = writeln!(numbered, "{marked}{line:>5} | {text}");
    }
    block(out, "text", &numbered);
    if let Some(diff) = &a.diff {
        out.push_str("  - change diff\n");
        block(out, "diff", diff);
    }
}

pub(crate) fn fix(out: &mut String, f: &ReportFinding) {
    if let Some(patch) = &f.suggested_patch {
        out.push_str("  - suggested fix\n");
        block(out, "diff", patch);
    }

    if let Some(note) = &f.fix_note {
        let _ = writeln!(out, "  - fix: {}", md(note));
    }
}
