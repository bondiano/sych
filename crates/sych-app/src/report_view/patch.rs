//! Apply-ready unified patches for verified edits to the pinned head.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::slice;
use std::sync::Arc;

use sych_adapters::git::LocalSnapshot;
use sych_core::{
    Finding, FindingId, Lifecycle, Redactor, RepoPath, SnapshotReader, SourceSide, SuggestedChange,
    is_secret_path,
};

const CONTEXT: usize = 3;
const HIDDEN: &str =
    "Patch unavailable because the source or replacement contains hidden or sensitive text.";

pub(super) struct FixPreview {
    pub(super) patch: Option<String>,
    pub(super) note: Option<String>,
}

#[derive(PartialEq, Eq)]
struct Edit {
    lines: Range<usize>,
    replacement: String,
}

struct FileEdits {
    text: Arc<str>,
    edits: Vec<Edit>,
    findings: Vec<FindingId>,
}

pub(super) fn fixes(
    snap: Option<&LocalSnapshot>,
    findings: &[&Finding],
    redactor: &Redactor,
) -> (String, BTreeMap<FindingId, FixPreview>) {
    let mut previews = BTreeMap::new();
    let mut files: BTreeMap<&RepoPath, FileEdits> = BTreeMap::new();
    for finding in findings {
        let Some(change) = &finding.suggested_change else {
            continue;
        };
        let path = &change.code.path;
        let prepared = prepare(snap, change, redactor).and_then(|(text, edit)| {
            let diff = file_patch(path, &text, slice::from_ref(&edit), redactor).ok_or(HIDDEN)?;
            Ok((diff, text, edit))
        });
        let preview = match prepared {
            Ok((diff, text, edit)) => {
                let note = if finding.lifecycle == Lifecycle::Open {
                    let file = files.entry(path).or_insert_with(|| FileEdits {
                        text,
                        edits: Vec::new(),
                        findings: Vec::new(),
                    });
                    file.findings.push(finding.id);
                    register(&mut file.edits, edit)
                } else {
                    None
                };
                FixPreview {
                    patch: Some(diff),
                    note: note.map(str::to_owned),
                }
            }
            Err(note) => FixPreview {
                patch: None,
                note: Some(note.to_owned()),
            },
        };
        previews.insert(finding.id, preview);
    }

    let mut combined = String::new();
    for (path, mut file) in files {
        file.edits.sort_unstable_by_key(|edit| edit.lines.start);
        if let Some(diff) = file_patch(path, &file.text, &file.edits, redactor) {
            combined.push_str(&diff);
        } else {
            for id in file.findings {
                previews.get_mut(&id).expect("preview").note = Some(
                    "Merged context contains hidden or sensitive text. Edits to this file are omitted from the combined patch."
                        .into(),
                );
            }
        }
    }

    (combined, previews)
}

/// Fix-first input order decides which of two overlapping, incompatible edits is retained.
fn register(edits: &mut Vec<Edit>, candidate: Edit) -> Option<&'static str> {
    for edit in edits.iter() {
        if *edit == candidate {
            return Some("This edit is already included in the combined patch.");
        }

        if edit.lines.start < candidate.lines.end && candidate.lines.start < edit.lines.end {
            return Some("This edit overlaps another fix and is omitted from the combined patch.");
        }
    }

    edits.push(candidate);
    None
}

fn prepare(
    snap: Option<&LocalSnapshot>,
    change: &SuggestedChange,
    redactor: &Redactor,
) -> Result<(Arc<str>, Edit), &'static str> {
    let code = &change.code;
    if code.side != SourceSide::Head {
        return Err("The suggested change does not target the pinned head.");
    }

    let path = code.path.as_str();
    if is_secret_path(path) || path.chars().any(char::is_control) || !visible(path, redactor) {
        return Err("Patch unavailable because the path contains hidden or sensitive text.");
    }

    let unavailable = "Source unavailable for the suggested change.";
    let snap = snap.ok_or(unavailable)?;
    let source = snap.read_span(code).map_err(|_| unavailable)?;
    let (_, text) = snap
        .blob(SourceSide::Head, &code.path)
        .map_err(|_| unavailable)?;
    let start = usize::try_from(code.start_line - 1).map_err(|_| unavailable)?;
    let end = usize::try_from(code.end_line).map_err(|_| unavailable)?;
    if !visible(&change.replacement, redactor) {
        return Err(HIDDEN);
    }

    let edit = Edit {
        lines: start..end,
        replacement: replacement(&source.text, &change.replacement, &text),
    };
    if source.text == edit.replacement {
        return Err("The suggested change is identical to the original.");
    }

    Ok((text, edit))
}

/// A suggestion is the new content of whole lines. Use the original line ending for the
/// replacement lines and preserve the end-of-file newline state.
fn replacement(old: &str, proposed: &str, file: &str) -> String {
    if proposed.is_empty() {
        return String::new();
    }

    let style = if old.contains('\n') { old } else { file };
    let newline = if style.contains("\r\n") { "\r\n" } else { "\n" };
    let logical = proposed
        .strip_suffix("\r\n")
        .or_else(|| proposed.strip_suffix('\n'))
        .unwrap_or(proposed);
    let mut lines = logical.split('\n').peekable();
    let mut framed = String::new();
    while let Some(line) = lines.next() {
        let terminated = lines.peek().is_some();
        let contents = if terminated {
            line.strip_suffix('\r').unwrap_or(line)
        } else {
            line
        };
        framed.push_str(contents);
        if terminated || old.ends_with('\n') {
            framed.push_str(newline);
        }
    }

    framed
}

fn visible(text: &str, redactor: &Redactor) -> bool {
    visible_chars(text) && redactor.redact(text) == text
}

fn visible_chars(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\t' => {}
            '\r' if chars.peek() == Some(&'\n') => {}
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => return false,
            c if c.is_control() => return false,
            _ => {}
        }
    }

    true
}

/// Git's quoted pathname syntax, with octal escapes for each non-ASCII byte.
fn quoted_path(prefix: char, path: &RepoPath) -> String {
    let path = path.as_str();
    if path
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'))
    {
        return format!("{prefix}/{path}");
    }

    let mut quoted = format!("\"{prefix}/");
    for b in path.bytes() {
        match b {
            b'"' | b'\\' => {
                quoted.push('\\');
                quoted.push(char::from(b));
            }
            b' '..=b'~' => quoted.push(char::from(b)),
            _ => {
                let _ = write!(quoted, "\\{b:03o}");
            }
        }
    }
    quoted.push('"');
    quoted
}

fn file_patch(path: &RepoPath, text: &str, edits: &[Edit], redactor: &Redactor) -> Option<String> {
    let old = quoted_path('a', path);
    let new = quoted_path('b', path);
    let mut diff = format!("diff --git {old} {new}\n--- {old}\n+++ {new}\n");
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    // A patch may show part of a private key or a custom match whose start is outside its hunk.
    if redactor.redact(text) != text {
        return None;
    }

    let updated = edited_text(&lines, edits);
    if redactor.redact(&updated) != updated {
        return None;
    }

    let mut groups = edits.iter().peekable();
    let mut previous_end = 0;
    let mut previous_new_end = 0;
    while let Some(first) = groups.next() {
        let start = first.lines.start.saturating_sub(CONTEXT);
        let mut end = (first.lines.end + CONTEXT).min(lines.len());
        let mut hunk = vec![first];
        while let Some(next) =
            groups.next_if(|next| next.lines.start.saturating_sub(CONTEXT) <= end)
        {
            end = (next.lines.end + CONTEXT).min(lines.len());
            hunk.push(next);
        }

        let new_start = previous_new_end + start - previous_end;
        if lines[start..end].iter().any(|line| !visible_chars(line)) {
            return None;
        }

        let new_len = emit_hunk(&mut diff, &lines, start..end, new_start, &hunk);
        previous_end = end;
        previous_new_end = new_start + new_len;
    }

    Some(diff)
}

fn edited_text(lines: &[&str], edits: &[Edit]) -> String {
    let mut updated = String::new();
    let mut cursor = 0;
    for edit in edits {
        updated.extend(lines[cursor..edit.lines.start].iter().copied());
        updated.push_str(&edit.replacement);
        cursor = edit.lines.end;
    }
    updated.extend(lines[cursor..].iter().copied());
    updated
}

fn emit_hunk(
    patch: &mut String,
    lines: &[&str],
    original: Range<usize>,
    new_start: usize,
    edits: &[&Edit],
) -> usize {
    let removed: usize = edits.iter().map(|edit| edit.lines.len()).sum();
    let added: usize = edits
        .iter()
        .map(|edit| edit.replacement.split_inclusive('\n').count())
        .sum();
    let new_len = original.len() - removed + added;
    let new_line = new_start + usize::from(new_len > 0);
    let _ = writeln!(
        patch,
        "@@ -{},{} +{new_line},{new_len} @@",
        original.start + 1,
        original.len()
    );
    let mut cursor = original.start;
    for edit in edits {
        for line in &lines[cursor..edit.lines.start] {
            emit_line(patch, ' ', line);
        }
        for line in &lines[edit.lines.clone()] {
            emit_line(patch, '-', line);
        }
        for line in edit.replacement.split_inclusive('\n') {
            emit_line(patch, '+', line);
        }
        cursor = edit.lines.end;
    }

    for line in &lines[cursor..original.end] {
        emit_line(patch, ' ', line);
    }

    new_len
}

fn emit_line(patch: &mut String, sign: char, line: &str) {
    patch.push(sign);
    patch.push_str(line);
    if !line.ends_with('\n') {
        patch.push_str("\n\\ No newline at end of file\n");
    }
}

#[cfg(test)]
#[path = "patch/tests.rs"]
mod tests;
