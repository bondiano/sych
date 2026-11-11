//! Code the change moves: lines it removes on BASE that reappear, in order and with the same text
//! (indentation aside), among the lines it adds on HEAD. A moved line is neither a removal nor
//! new code, so a check that a test was dropped or behaviour kept reads the move as such.

use std::collections::HashMap;
use std::fmt::Write;

use super::{Context, hunk_lines, side_text};
use crate::model::DataBlock;
use crate::{DiffManifest, RepoPath, SnapshotReader, SourceSide};

/// A shorter block is a coincidence; git's `--color-moved` uses the same 20 characters.
const MIN_LINES: usize = 3;
const MIN_ALNUM: usize = 20;
/// Places one line of text is matched at; boilerplate repeats.
const MAX_CANDIDATES: usize = 64;
/// Moves one block lists; the rest are counted.
const MAX_LISTED: usize = 40;

/// BASE lines `from` of `from_path` reappear as HEAD lines `to` of `to_path` (inclusive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from_path: RepoPath,
    pub from: (u32, u32),
    pub to_path: RepoPath,
    pub to: (u32, u32),
}

/// The lines one hunk side covers, trimmed: a run that can match only in order.
struct Run {
    path: RepoPath,
    lines: Vec<(u32, String)>,
}

fn runs(manifest: &DiffManifest, reader: &dyn SnapshotReader, side: SourceSide) -> Vec<Run> {
    let mut out = Vec::new();
    for file in manifest.files.iter().filter(|f| !f.hidden()) {
        let Some(at) = file.side(side) else {
            continue;
        };
        let Some(text) = side_text(reader, side, at) else {
            continue;
        };
        for (start, count) in file.hunks.iter().map(|h| h.on(side)) {
            let lines: Vec<(u32, String)> = hunk_lines(&text, start, count)
                .map(|(n, line)| (n, line.trim().to_owned()))
                .collect();
            if !lines.is_empty() {
                out.push(Run {
                    path: at.path.clone(),
                    lines,
                });
            }
        }
    }
    out
}

fn alphanumeric(lines: &[(u32, String)]) -> usize {
    lines
        .iter()
        .map(|(_, t)| t.chars().filter(|c| c.is_alphanumeric()).count())
        .sum()
}

/// Every block the change moves, in manifest order. Each added line belongs to one move at most;
/// a block starts at a line with a letter or digit, so braces and blank lines never anchor one.
// shortcut: recomputed for each context it is shown in; keep it on the engine if a profile shows the cost.
pub fn moves(manifest: &DiffManifest, reader: &dyn SnapshotReader) -> Vec<Move> {
    let removed = runs(manifest, reader, SourceSide::DiffBase);
    let added = runs(manifest, reader, SourceSide::Head);
    let mut at: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (r, run) in added.iter().enumerate() {
        for (i, (_, text)) in run.lines.iter().enumerate() {
            if text.chars().any(char::is_alphanumeric) {
                let places = at.entry(text.as_str()).or_default();
                if places.len() < MAX_CANDIDATES {
                    places.push((r, i));
                }
            }
        }
    }

    let mut used: Vec<Vec<bool>> = added.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut out = Vec::new();
    for run in &removed {
        let mut i = 0;
        while i < run.lines.len() {
            let from = &run.lines[i..];
            // The longest block from here; the first of equals, so the result is stable.
            let mut best: Option<(usize, usize, usize)> = None;
            for &(r, j) in at.get(from[0].1.as_str()).into_iter().flatten() {
                let len = from
                    .iter()
                    .zip(&added[r].lines[j..])
                    .zip(&used[r][j..])
                    .take_while(|((a, b), used)| !**used && a.1 == b.1)
                    .count();
                if best.is_none_or(|(_, _, longest)| len > longest) {
                    best = Some((r, j, len));
                }
            }
            match best {
                Some((r, j, len))
                    if len >= MIN_LINES && alphanumeric(&from[..len]) >= MIN_ALNUM =>
                {
                    used[r][j..j + len].fill(true);
                    let to = &added[r].lines[j..j + len];
                    out.push(Move {
                        from_path: run.path.clone(),
                        from: (from[0].0, from[len - 1].0),
                        to_path: added[r].path.clone(),
                        to: (to[0].0, to[len - 1].0),
                    });
                    i += len;
                }
                _ => i += 1,
            }
        }
    }
    out
}

/// The parts of BASE lines `start..start+count` of `path` that no move covers.
pub(super) fn unmoved(moves: &[Move], path: &RepoPath, start: u32, count: u32) -> Vec<(u32, u32)> {
    let mut parts = Vec::new();
    let mut line = start;
    let end = start + count;
    while line < end {
        if let Some(m) = moves
            .iter()
            .find(|m| &m.from_path == path && (m.from.0..=m.from.1).contains(&line))
        {
            line = m.from.1 + 1;
            continue;
        }

        let next = moves
            .iter()
            .filter(|m| &m.from_path == path && m.from.0 > line)
            .map(|m| m.from.0)
            .min()
            .unwrap_or(end)
            .min(end);
        parts.push((line, next - line));
        line = next;
    }
    parts
}

pub(super) fn describe(m: &Move) -> String {
    format!(
        "{}:{}-{} moved to {}:{}-{}",
        m.from_path, m.from.0, m.from.1, m.to_path, m.to.0, m.to.1
    )
}

impl Context {
    /// The blocks the change moves from or into the files shown here, so neither side of a move
    /// is read as a removal or as new code.
    pub fn add_moves(&mut self, manifest: &DiffManifest, reader: &dyn SnapshotReader) {
        let shown = |p: &RepoPath| self.refs.values().any(|r| &r.path == p);
        let relevant: Vec<Move> = moves(manifest, reader)
            .into_iter()
            .filter(|m| shown(&m.from_path) || shown(&m.to_path))
            .collect();
        if relevant.is_empty() {
            return;
        }

        let mut content = String::new();
        for m in relevant.iter().take(MAX_LISTED) {
            let _ = writeln!(content, "{}", describe(m));
        }
        if relevant.len() > MAX_LISTED {
            let _ = writeln!(content, "{} more moved blocks", relevant.len() - MAX_LISTED);
        }

        self.blocks.push(DataBlock {
            label:
                "blocks this change moves: removed BASE lines that reappear on HEAD in order with \
                    the same text, indentation aside; the moved code is neither removed nor new"
                    .into(),
            content,
        });
    }
}
