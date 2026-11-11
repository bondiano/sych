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

/// Every block the change moves, in manifest order. The longest blocks are taken first across
/// the whole change, so a shorter block that happens to come earlier never takes the HEAD lines
/// a longer one needs; each removed and each added line belongs to one move at most. A block
/// starts at a line with a letter or digit, so braces and blank lines never anchor one.
// shortcut: a candidate that overlaps a longer accepted block is dropped whole, not trimmed; trim it if a split move shows up as removal.
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

    // (length, removed run, line in it, added run, line in it) of every block long enough.
    let mut blocks: Vec<(usize, usize, usize, usize, usize)> = Vec::new();
    for (q, run) in removed.iter().enumerate() {
        for i in 0..run.lines.len() {
            for &(r, j) in at.get(run.lines[i].1.as_str()).into_iter().flatten() {
                let len = run.lines[i..]
                    .iter()
                    .zip(&added[r].lines[j..])
                    .take_while(|(a, b)| a.1 == b.1)
                    .count();
                if len >= MIN_LINES && alphanumeric(&run.lines[i..i + len]) >= MIN_ALNUM {
                    blocks.push((len, q, i, r, j));
                }
            }
        }
    }
    // Longest first; equals in manifest order, so the result is stable.
    blocks.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (a.1, a.2, a.3, a.4).cmp(&(b.1, b.2, b.3, b.4)))
    });

    let mut taken_from: Vec<Vec<bool>> =
        removed.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut taken_to: Vec<Vec<bool>> = added.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut kept = Vec::new();
    for (len, q, i, r, j) in blocks {
        let free = |taken: &[bool], at: usize| taken[at..at + len].iter().all(|t| !t);
        if free(&taken_from[q], i) && free(&taken_to[r], j) {
            taken_from[q][i..i + len].fill(true);
            taken_to[r][j..j + len].fill(true);
            kept.push((q, i, len, r, j));
        }
    }

    kept.sort_unstable();
    kept.into_iter()
        .map(|(q, i, len, r, j)| {
            let (from, to) = (&removed[q].lines[i..i + len], &added[r].lines[j..j + len]);
            Move {
                from_path: removed[q].path.clone(),
                from: (from[0].0, from[len - 1].0),
                to_path: added[r].path.clone(),
                to: (to[0].0, to[len - 1].0),
            }
        })
        .collect()
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
