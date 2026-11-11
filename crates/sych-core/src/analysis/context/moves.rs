//! Code the change moves: lines it removes on BASE that reappear, in order and with the same text
//! (indentation aside), among the lines it adds on HEAD. A moved line is neither a removal nor
//! new code, so a check that a test was dropped or behaviour kept reads the move as such.

use std::array;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::fmt::Write;

use super::{Context, hunk_lines, side_text};
use crate::model::DataBlock;
use crate::{DiffManifest, RepoPath, SnapshotReader, SourceSide};

/// A block of fewer lines is a coincidence (Sych's own rule); blocks are found by their first
/// `MIN_LINES` lines.
const MIN_LINES: usize = 3;
/// Letters and digits a block needs, as git's `--color-moved` requires.
const MIN_ALNUM: usize = 20;
/// Candidate places compared at most, so a change of many identical lines cannot make the search
/// quadratic; past it the search stops and says so.
const MAX_PAIRS: usize = 1_000_000;
/// Moves one block lists; the rest are counted.
const MAX_LISTED: usize = 40;
pub(super) const CUT: &str =
    "move detection stopped at its work bound: later moved blocks may read as removed and new";

/// BASE lines `from` of `from_path` reappear as HEAD lines `to` of `to_path` (inclusive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub from_path: RepoPath,
    pub from: (u32, u32),
    pub to_path: RepoPath,
    pub to: (u32, u32),
}

/// Every block a change moves, and whether the search stopped at its work bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Moves {
    pub blocks: Vec<Move>,
    /// The search compared `MAX_PAIRS` places and stopped: later blocks may read as removed.
    pub cut: bool,
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

/// A block's length and where it starts: removed run, line in it, added run, line in it.
type Ranked = (usize, Reverse<(usize, usize, usize, usize)>);

fn significant(text: &str) -> bool {
    text.chars().any(char::is_alphanumeric)
}

/// The first `MIN_LINES` lines from `i`, when the first has a letter or digit: a block starts
/// there, never at a brace or a blank line.
fn key(lines: &[(u32, String)], i: usize) -> Option<[&str; MIN_LINES]> {
    let window = lines.get(i..i + MIN_LINES)?;
    significant(&window[0].1).then(|| array::from_fn(|k| window[k].1.as_str()))
}

/// Every block the change moves, in manifest order. The longest blocks are taken first across
/// the whole change; a block partly taken by a longer one gives back its free parts, which
/// compete again on their own length. Each removed and each added line belongs to one move at
/// most.
// shortcut: recomputed for each context it is shown in; keep it on the engine if a profile shows the cost.
pub fn moves(manifest: &DiffManifest, reader: &dyn SnapshotReader) -> Moves {
    let removed = runs(manifest, reader, SourceSide::DiffBase);
    let added = runs(manifest, reader, SourceSide::Head);
    let mut at: HashMap<[&str; MIN_LINES], Vec<(usize, usize)>> = HashMap::new();
    for (r, run) in added.iter().enumerate() {
        for j in 0..run.lines.len() {
            if let Some(k) = key(&run.lines, j) {
                at.entry(k).or_default().push((r, j));
            }
        }
    }

    // The longest pops first, equals in manifest order, so the result is stable.
    let mut queue: BinaryHeap<Ranked> = BinaryHeap::new();
    // How far each diagonal of a removed and an added run is already matched: a place inside a
    // block found from an earlier place is that block's tail, not a block of its own.
    let mut reached: HashMap<(usize, usize, usize), usize> = HashMap::new();
    let (mut pairs, mut cut) = (0, false);
    'search: for (q, run) in removed.iter().enumerate() {
        for i in 0..run.lines.len() {
            let Some(k) = key(&run.lines, i) else {
                continue;
            };
            for &(r, j) in at.get(&k).into_iter().flatten() {
                pairs += 1;
                if pairs > MAX_PAIRS {
                    cut = true;
                    break 'search;
                }

                // `i - j`, shifted to stay unsigned.
                let diagonal = (q, r, i + added[r].lines.len() - j);
                if reached.get(&diagonal).is_some_and(|&end| i < end) {
                    continue;
                }

                let len = run.lines[i..]
                    .iter()
                    .zip(&added[r].lines[j..])
                    .take_while(|(a, b)| a.1 == b.1)
                    .count();
                reached.insert(diagonal, i + len);
                if alphanumeric(&run.lines[i..i + len]) >= MIN_ALNUM {
                    queue.push((len, Reverse((q, i, r, j))));
                }
            }
        }
    }

    let mut taken_from: Vec<Vec<bool>> =
        removed.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut taken_to: Vec<Vec<bool>> = added.iter().map(|r| vec![false; r.lines.len()]).collect();
    let mut kept = Vec::new();
    while let Some((len, Reverse((q, i, r, j)))) = queue.pop() {
        let free: Vec<bool> = (0..len)
            .map(|k| !taken_from[q][i + k] && !taken_to[r][j + k])
            .collect();
        if free.iter().all(|&f| f) {
            taken_from[q][i..i + len].fill(true);
            taken_to[r][j..j + len].fill(true);
            kept.push((q, i, len, r, j));
            continue;
        }

        // The free parts, each again from a line with a letter or digit.
        let mut k = 0;
        while k < len {
            if !free[k] {
                k += 1;
                continue;
            }

            let mut start = k;
            while k < len && free[k] {
                k += 1;
            }
            while start < k && !significant(&removed[q].lines[i + start].1) {
                start += 1;
            }
            let part = &removed[q].lines[i + start..i + k];
            if part.len() >= MIN_LINES && alphanumeric(part) >= MIN_ALNUM {
                queue.push((part.len(), Reverse((q, i + start, r, j + start))));
            }
        }
    }

    kept.sort_unstable();
    let blocks = kept
        .into_iter()
        .map(|(q, i, len, r, j)| {
            let (from, to) = (&removed[q].lines[i..i + len], &added[r].lines[j..j + len]);
            Move {
                from_path: removed[q].path.clone(),
                from: (from[0].0, from[len - 1].0),
                to_path: added[r].path.clone(),
                to: (to[0].0, to[len - 1].0),
            }
        })
        .collect();
    Moves { blocks, cut }
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
        let found = moves(manifest, reader);
        let relevant: Vec<Move> = found
            .blocks
            .into_iter()
            .filter(|m| shown(&m.from_path) || shown(&m.to_path))
            .collect();
        if relevant.is_empty() && !found.cut {
            return;
        }

        let mut content = String::new();
        for m in relevant.iter().take(MAX_LISTED) {
            let _ = writeln!(content, "{}", describe(m));
        }
        if relevant.len() > MAX_LISTED {
            let _ = writeln!(content, "{} more moved blocks", relevant.len() - MAX_LISTED);
        }

        if found.cut {
            let _ = writeln!(content, "{CUT}");
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
