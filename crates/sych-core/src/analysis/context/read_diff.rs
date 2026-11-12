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

/// One window of a hunk side: `start..=end` on `side`, cut to `max_lines`.
fn window(
    file: &FileChange,
    side: SourceSide,
    start: u32,
    lines: u32,
    max_lines: u32,
) -> Option<(SourceSide, u32, u32, bool)> {
    let count = file.side(side)?.line_count?;
    if lines == 0 || count == 0 {
        return None;
    }

    let first = start.saturating_sub(AROUND).max(1);
    let last = (start + lines - 1 + AROUND).min(count);
    let cut = last - first + 1 > max_lines;
    Some((
        side,
        first,
        if cut { first + max_lines - 1 } else { last },
        cut,
    ))
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
        let (status, mut body, windows) = match file {
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
                let windows: Vec<_> = f
                    .hunks
                    .iter()
                    .flat_map(|h| {
                        [
                            window(&f, SourceSide::Head, h.new_start, h.new_lines, max_lines),
                            window(
                                &f,
                                SourceSide::DiffBase,
                                h.old_start,
                                h.old_lines,
                                max_lines,
                            ),
                        ]
                    })
                    .flatten()
                    .collect();
                let mut body = change_line(&f);
                for h in &f.hunks {
                    let _ = write!(
                        body,
                        " -{},{} +{},{}",
                        h.old_start, h.old_lines, h.new_start, h.new_lines
                    );
                }
                let status = if windows.len() > max_refs || windows.iter().any(|w| w.3) {
                    OpStatus::Partial
                } else {
                    OpStatus::CompleteForQuery
                };
                (status, body, windows)
            }
        };
        let mut refs = 0;
        for (side, start, end, cut) in windows.iter().take(max_refs) {
            match reader.code_ref(*side, path, *start, *end) {
                Ok(code_ref) => {
                    self.add_span(&code_ref, &reason, reader);
                    refs += 1;
                }
                Err(err) => {
                    limitations.push(format!("{path} {} {start}-{end}: {err}", side_name(*side)));
                }
            }
            if *cut {
                limitations.push(format!(
                    "{path} {} from line {start}: a hunk longer than {max_lines} lines is cut; read_lines shows the rest",
                    side_name(*side)
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
