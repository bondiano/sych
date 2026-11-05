//! What a verifier sees of a hypothesis anchored on removed lines: the names they lose and
//! where else the change removes or adds them.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::{Context, MAX_REMOVED_LINES};
use crate::model::DataBlock;
use crate::{CodeRef, DiffManifest, FileSide, RepoPath, SnapshotReader, SourceSide};

/// Names a hypothesis's removed lines lose that one block follows.
const MAX_LOST_NAMES: usize = 12;
/// Removed lines of one other file shown per lost name; the rest are counted.
const MAX_LOST_LINES_PER_FILE: usize = 3;

/// The identifiers of `text` of three characters or more.
fn names(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.len() >= 3 && !w.starts_with(|c: char| c.is_ascii_digit()))
}

/// The whole text of one side of a changed file, when it is readable text.
fn side_text(reader: &dyn SnapshotReader, side: SourceSide, file: &FileSide) -> Option<String> {
    let lines = file.line_count.filter(|&n| n > 0)?;
    let span = reader
        .code_ref(side, &file.path, 1, lines)
        .and_then(|r| reader.read_span(&r))
        .ok()?;
    Some(span.text)
}

/// The lines of `text` a hunk side covers, numbered as in the file.
fn hunk_lines(text: &str, start: u32, count: u32) -> impl Iterator<Item = (u32, &str)> {
    (1..)
        .zip(text.lines())
        .skip(start.saturating_sub(1) as usize)
        .take(count as usize)
}

/// Where the rest of a change removes or adds one lost name.
#[derive(Default)]
struct Uses<'a> {
    removed: BTreeMap<&'a RepoPath, Vec<(u32, String)>>,
    /// Removed lines naming it in files the change deletes.
    deleted: BTreeMap<&'a RepoPath, u32>,
    added: BTreeMap<&'a RepoPath, u32>,
}

/// The names the removed lines under BASE `anchors` use that their file's HEAD no longer has,
/// in order of appearance, and the files of those anchors.
fn lost_names<'m>(
    anchors: &[&CodeRef],
    manifest: &'m DiffManifest,
    reader: &dyn SnapshotReader,
) -> (Vec<String>, Vec<&'m RepoPath>) {
    let mut lost: Vec<String> = Vec::new();
    let mut anchored: Vec<&RepoPath> = Vec::new();
    for anchor in anchors.iter().filter(|a| a.side == SourceSide::DiffBase) {
        // A deleted file loses every name; the block is about a file that stays.
        let Some((file, old, new)) = manifest.files.iter().find_map(|f| {
            let old = f.old.as_ref().filter(|o| o.path == anchor.path)?;
            Some((f, old, f.new.as_ref()?))
        }) else {
            continue;
        };
        let removed = |line: u32| {
            file.hunks
                .iter()
                .any(|h| (h.old_start..h.old_start + h.old_lines).contains(&line))
        };
        let Some(base) = side_text(reader, SourceSide::DiffBase, old) else {
            continue;
        };
        // An emptied file loses every name; one no longer readable as text says nothing.
        let empty = (new.line_count == Some(0)).then(String::new);
        let Some(head) = empty.or_else(|| side_text(reader, SourceSide::Head, new)) else {
            continue;
        };
        let kept: Vec<&str> = names(&head).collect();
        let span = anchor.end_line.saturating_sub(anchor.start_line) + 1;
        for (_, line) in hunk_lines(&base, anchor.start_line, span).filter(|(n, _)| removed(*n)) {
            for name in names(line) {
                if !kept.contains(&name) && !lost.iter().any(|l| l == name) {
                    lost.push(name.to_owned());
                }
            }
        }
        anchored.push(&old.path);
    }
    (lost, anchored)
}

/// Where the changed files other than `anchored` remove or add each of `lost`.
fn uses_elsewhere<'m>(
    lost: &[String],
    anchored: &[&RepoPath],
    manifest: &'m DiffManifest,
    reader: &dyn SnapshotReader,
) -> Vec<Uses<'m>> {
    let mut uses: Vec<Uses> = lost.iter().map(|_| Uses::default()).collect();
    let others = manifest
        .files
        .iter()
        .filter(|f| !f.hidden() && !f.old.as_ref().is_some_and(|o| anchored.contains(&&o.path)));
    for file in others {
        if let Some(old) = &file.old
            && let Some(base) = side_text(reader, SourceSide::DiffBase, old)
        {
            for h in file.hunks.iter().filter(|h| h.old_lines > 0) {
                for (n, line) in hunk_lines(&base, h.old_start, h.old_lines) {
                    let in_line: Vec<&str> = names(line).collect();
                    for (name, u) in lost.iter().zip(&mut uses) {
                        if !in_line.contains(&name.as_str()) {
                            continue;
                        }

                        if file.new.is_some() {
                            u.removed
                                .entry(&old.path)
                                .or_default()
                                .push((n, line.to_owned()));
                        } else {
                            *u.deleted.entry(&old.path).or_default() += 1;
                        }
                    }
                }
            }
        }

        let Some(new) = &file.new else { continue };
        let Some(head) = side_text(reader, SourceSide::Head, new) else {
            continue;
        };
        for h in file.hunks.iter().filter(|h| h.new_lines > 0) {
            for (_, line) in hunk_lines(&head, h.new_start, h.new_lines) {
                let in_line: Vec<&str> = names(line).collect();
                for (name, u) in lost.iter().zip(&mut uses) {
                    if in_line.contains(&name.as_str()) {
                        *u.added.entry(&new.path).or_default() += 1;
                    }
                }
            }
        }
    }
    uses
}

impl Context {
    /// For hypotheses anchored on removed BASE lines of a file that stays: the names those lines
    /// use that the file's HEAD no longer has, and where else the change removes or adds them,
    /// so a guard removed together with the field it checks reads as one change.
    pub fn add_lost_names(
        &mut self,
        anchors: &[&CodeRef],
        manifest: &DiffManifest,
        reader: &dyn SnapshotReader,
    ) {
        let (lost, anchored) = lost_names(anchors, manifest, reader);
        if lost.is_empty() {
            return;
        }

        let followed = lost.len().min(MAX_LOST_NAMES);
        let uses = uses_elsewhere(&lost[..followed], &anchored, manifest, reader);

        let mut content = String::new();
        let mut shown = 0;
        for (name, u) in lost.iter().zip(&uses) {
            if u.removed.is_empty() && u.deleted.is_empty() && u.added.is_empty() {
                let _ = writeln!(
                    content,
                    "`{name}`: no other file of the change removes or adds it"
                );
                continue;
            }

            let _ = writeln!(content, "`{name}`");
            for (path, lines) in &u.removed {
                let take = lines
                    .len()
                    .min(MAX_LOST_LINES_PER_FILE)
                    .min(MAX_REMOVED_LINES as usize - shown);
                for (n, line) in &lines[..take] {
                    let _ = writeln!(content, "  {path}:{n} - {line}");
                }
                shown += take;
                if lines.len() > take {
                    let _ = writeln!(
                        content,
                        "  {path}: {} more removed lines name it",
                        lines.len() - take
                    );
                }
            }

            for (path, count) in &u.deleted {
                let _ = writeln!(content, "  {path}: deleted, {count} removed lines name it");
            }

            for (path, count) in &u.added {
                let _ = writeln!(content, "  added: {path} ({count} lines)");
            }
        }
        if lost.len() > followed {
            let _ = writeln!(content, "{} more names not followed", lost.len() - followed);
        }

        self.blocks.push(DataBlock {
            label: "names the removed lines of a hypothesis use that their file's HEAD no \
                    longer has, and where else this change removes (BASE `-`) or adds them"
                .into(),
            content,
        });
    }
}
