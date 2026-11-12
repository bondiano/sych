//! `read_diff`: the hunks of one changed file as spans with three lines around each, so a review
//! of one file can see what the change did to another one it names.

use std::fmt::Write;

use super::{Context, change_line, side_name};
use crate::analysis::Answered;
use crate::broker::OpStatus;
use crate::model::DataBlock;
use crate::{FileChange, RepoPath, SnapshotReader, SourceSide};

/// Unchanged lines shown on each side of a hunk.
const AROUND: u32 = 3;

/// One window of a hunk side, read at that side's own path (a rename's BASE has the old one).
struct Window {
    side: SourceSide,
    path: RepoPath,
    start: u32,
    end: u32,
    /// The hunk side and its context were longer than `max_lines`.
    cut: bool,
}

/// `start..=end` of `lines` hunk lines on `side` and [`AROUND`] lines each way, cut to `max_lines`.
fn window(
    file: &FileChange,
    side: SourceSide,
    start: u32,
    lines: u32,
    max_lines: u32,
) -> Option<Window> {
    let shown = file.side(side)?;
    let count = shown.line_count?;
    if lines == 0 || count == 0 {
        return None;
    }

    let first = start.saturating_sub(AROUND).max(1);
    let last = start
        .saturating_add(lines - 1)
        .saturating_add(AROUND)
        .min(count);
    // A hunk past the side's end (a manifest of other blobs) has nothing to show.
    if first > last {
        return None;
    }

    let cut = last - first + 1 > max_lines;
    Some(Window {
        side,
        path: shown.path.clone(),
        start: first,
        end: if cut { first + max_lines - 1 } else { last },
        cut,
    })
}

/// Every hunk side of `file` as a window, HEAD before BASE within each hunk.
fn windows(file: &FileChange, max_lines: u32) -> Vec<Window> {
    file.hunks
        .iter()
        .flat_map(|h| {
            [
                window(file, SourceSide::Head, h.new_start, h.new_lines, max_lines),
                window(
                    file,
                    SourceSide::DiffBase,
                    h.old_start,
                    h.old_lines,
                    max_lines,
                ),
            ]
        })
        .flatten()
        .collect()
}

/// The file's change line and each hunk as `-start,count +start,count`.
fn summary(file: &FileChange) -> String {
    let mut body = change_line(file);
    for h in &file.hunks {
        let _ = write!(
            body,
            " -{},{} +{},{}",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        );
    }
    body
}

impl Context {
    /// Answers `read_diff` for `path` from the change [`Context::add_changes`] recorded: at most
    /// `max_refs` windows, HEAD before BASE within each hunk, each at most `max_lines` long.
    pub(crate) fn add_file_diff(
        &mut self,
        path: &RepoPath,
        question: &str,
        reader: &dyn SnapshotReader,
        (max_refs, max_lines): (usize, u32),
    ) -> Answered {
        let reason = format!("answer to: {}", question.replace('\n', " "));
        let Some(changes) = &self.changes else {
            let body = "this context has no list of the change's files".to_owned();
            self.blocks.push(DataBlock {
                label: reason,
                content: body,
            });
            return Answered {
                status: Some(OpStatus::Unsupported),
                refs: 0,
                limitations: Vec::new(),
            };
        };
        let file = changes
            .iter()
            .find(|f| f.old.iter().chain(&f.new).any(|s| &s.path == path))
            .cloned();
        let mut limitations = Vec::new();
        let (mut status, mut body, windows) = match file {
            None => (
                OpStatus::CompleteForQuery,
                format!("{path} is not changed by this change"),
                Vec::new(),
            ),
            Some(f) if f.hidden() => (
                OpStatus::Partial,
                format!("{path} is generated: never shown or reviewed"),
                Vec::new(),
            ),
            Some(f) => {
                let windows = windows(&f, max_lines);
                // Hunks with no window (no line count, past the side's end) are a change unseen.
                if windows.is_empty() && !f.hunks.is_empty() {
                    let mut lost = format!("no hunk of {path} can be shown");
                    for l in &f.limitations {
                        lost.push_str("; ");
                        lost.push_str(l);
                    }
                    limitations.push(lost);
                }

                let partial = !limitations.is_empty()
                    || windows.len() > max_refs
                    || windows.iter().any(|w| w.cut);
                let status = if partial {
                    OpStatus::Partial
                } else {
                    OpStatus::CompleteForQuery
                };
                (status, summary(&f), windows)
            }
        };
        let mut refs = 0;
        for w in windows.iter().take(max_refs) {
            let (side, at) = (
                side_name(w.side),
                format!("{} {}-{}", w.path, w.start, w.end),
            );
            match reader.code_ref(w.side, &w.path, w.start, w.end) {
                Ok(code_ref) => {
                    self.add_span(&code_ref, &reason, reader);
                    refs += 1;
                }
                Err(err) => {
                    // A window not shown leaves the answer short of the change.
                    status = OpStatus::Partial;
                    limitations.push(format!("{side} {at}: {err}"));
                }
            }
            if w.cut {
                limitations.push(format!(
                    "{side} {at}: the hunk and its context are longer than {max_lines} lines, so \
                     this window is cut; read_lines shows the rest"
                ));
            }
        }
        if windows.len() > max_refs {
            limitations.push(format!(
                "{max_refs} of {} hunk sides shown; read_lines shows the others",
                windows.len()
            ));
        }

        for l in &limitations {
            body.push('\n');
            body.push_str(l);
        }
        self.blocks.push(DataBlock {
            label: reason,
            content: body,
        });
        Answered {
            status: Some(status),
            refs,
            limitations,
        }
    }
}
