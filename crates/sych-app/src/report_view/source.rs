//! Context and display diffs from the pinned blobs named by a finding.

use std::fmt::Write as _;
use std::ops::Range;

use sych_adapters::git::LocalSnapshot;
use sych_core::{
    FileChange, FileSide, Hunk, Redactor, ReportAnchor, ReportExcerpt, SnapshotReader, SourceSide,
    count_lines, is_secret_path, line_range,
};

/// Extra source lines on either side of an anchor.
const EXCERPT_CONTEXT: u32 = 4;
/// Unified diffs use Git's usual context size.
const DIFF_CONTEXT: usize = 3;

pub(super) fn enrich(snap: &LocalSnapshot, anchor: &mut ReportAnchor, redactor: &Redactor) {
    let reference = &anchor.reference;
    let file = snap.manifest.files.iter().find(|f| {
        f.side(reference.side)
            .is_some_and(|side| side.path == reference.path && side.oid == reference.blob_oid)
    });
    if is_secret_path(reference.path.as_str())
        || file.is_some_and(hidden)
        || snap.read_span(reference).is_err()
    {
        return;
    }

    let Ok((_, source)) = snap.blob(reference.side, &reference.path) else {
        return;
    };
    let source = safe_source(&source, redactor);
    let first_line = reference.start_line.saturating_sub(EXCERPT_CONTEXT).max(1);
    let last_line = reference
        .end_line
        .saturating_add(EXCERPT_CONTEXT)
        .min(count_lines(source.as_bytes()));
    if let Some(text) = line_range(&source, first_line, last_line) {
        anchor.excerpt = Some(ReportExcerpt {
            first_line,
            text: text.to_owned(),
        });
    }

    let lines = index(reference.start_line.saturating_sub(1))..index(reference.end_line);
    anchor.diff =
        file.and_then(|file| change_diff(snap, file, reference.side, &source, lines, redactor));
}

fn hidden(file: &FileChange) -> bool {
    file.hidden()
        || [&file.old, &file.new]
            .into_iter()
            .flatten()
            .any(|side| is_secret_path(side.path.as_str()))
}

fn safe_source(source: &str, redactor: &Redactor) -> String {
    let redacted = redactor.redact(source);
    if count_lines(source.as_bytes()) != count_lines(redacted.as_bytes())
        || source.ends_with('\n') != redacted.ends_with('\n')
    {
        // Multiline redaction must preserve coordinates and hide the whole secret,
        // including when an excerpt begins in the middle of a private key.
        return source
            .split_inclusive('\n')
            .map(|line| {
                if line.ends_with('\n') {
                    "[REDACTED]\n"
                } else {
                    "[REDACTED]"
                }
            })
            .collect();
    }

    display_text(&redacted.replace("\r\n", "\n"))
}

fn display_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch != '\n'
            && (ch.is_control()
                || matches!(ch, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        {
            let _ = write!(out, "U+{:04X}", u32::from(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

fn index(line: u32) -> usize {
    usize::try_from(line).expect("Git line coordinates fit usize")
}

fn changed_range(start: u32, count: u32) -> Range<usize> {
    let start = index(start.saturating_sub(u32::from(count > 0)));
    start..start + index(count)
}

fn sides(hunk: Hunk) -> (Range<usize>, Range<usize>) {
    (
        changed_range(hunk.old_start, hunk.old_lines),
        changed_range(hunk.new_start, hunk.new_lines),
    )
}

fn change_diff(
    snap: &LocalSnapshot,
    file: &FileChange,
    side: SourceSide,
    source: &str,
    anchor: Range<usize>,
    redactor: &Redactor,
) -> Option<String> {
    if file.hunks.is_empty() {
        return None;
    }

    let (other_side, other_file) = match side {
        SourceSide::DiffBase => (SourceSide::Head, file.new.as_ref()),
        SourceSide::Head => (SourceSide::DiffBase, file.old.as_ref()),
        SourceSide::TargetTip => return None,
    };
    let other_source = if let Some(other) = other_file {
        let (oid, text) = snap.blob(other_side, &other.path).ok()?;
        if oid != other.oid {
            return None;
        }

        safe_source(&text, redactor)
    } else {
        String::new()
    };
    let (old, new) = if side == SourceSide::DiffBase {
        (source, other_source.as_str())
    } else {
        (other_source.as_str(), source)
    };
    let old: Vec<_> = old.split_inclusive('\n').collect();
    let new: Vec<_> = new.split_inclusive('\n').collect();
    let body = unified_hunks(&file.hunks, &old, &new, side, &anchor)?;
    let old_path = display_path(file.old.as_ref(), "a", redactor);
    let new_path = display_path(file.new.as_ref(), "b", redactor);
    Some(format!("--- {old_path}\n+++ {new_path}\n{body}"))
}

fn display_path(file: Option<&FileSide>, prefix: &str, redactor: &Redactor) -> String {
    let Some(file) = file else {
        return "/dev/null".into();
    };
    let path = display_text(&redactor.redact(file.path.as_str()).replace('\n', "U+000A"));
    let path = format!("{prefix}/{path}");
    if path
        .chars()
        .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\\'))
    {
        format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        path
    }
}

fn connected(left: Hunk, right: Hunk) -> bool {
    let (old_left, new_left) = sides(left);
    let (old_right, new_right) = sides(right);
    old_right.start.saturating_sub(old_left.end) <= DIFF_CONTEXT * 2
        && new_right.start.saturating_sub(new_left.end) <= DIFF_CONTEXT * 2
}

fn unified_hunks(
    hunks: &[Hunk],
    old: &[&str],
    new: &[&str],
    side: SourceSide,
    anchor: &Range<usize>,
) -> Option<String> {
    let mut out = String::new();
    let mut start = 0;
    while start < hunks.len() {
        let mut end = start + 1;
        while end < hunks.len() && connected(hunks[end - 1], hunks[end]) {
            end += 1;
        }
        let (first_old, first_new) = sides(hunks[start]);
        let (last_old, last_new) = sides(hunks[end - 1]);
        let before = first_old.start.min(first_new.start).min(DIFF_CONTEXT);
        let after = old
            .len()
            .checked_sub(last_old.end)?
            .min(new.len().checked_sub(last_new.end)?)
            .min(DIFF_CONTEXT);
        let old_range = first_old.start - before..last_old.end + after;
        let new_range = first_new.start - before..last_new.end + after;
        let visible = if side == SourceSide::DiffBase {
            &old_range
        } else {
            &new_range
        };
        if visible.start < anchor.end && anchor.start < visible.end {
            let _ = writeln!(
                out,
                "@@ -{} +{} @@",
                coordinates(&old_range),
                coordinates(&new_range)
            );
            let mut cursor = old_range.start;
            for hunk in &hunks[start..end] {
                let (old_edit, new_edit) = sides(*hunk);
                write_lines(&mut out, ' ', old.get(cursor..old_edit.start)?);
                cursor = old_edit.end;
                write_lines(&mut out, '-', old.get(old_edit)?);
                write_lines(&mut out, '+', new.get(new_edit)?);
            }
            write_lines(&mut out, ' ', old.get(cursor..old_range.end)?);
        }

        start = end;
    }

    (!out.is_empty()).then_some(out)
}

fn coordinates(lines: &Range<usize>) -> String {
    match lines.len() {
        0 => format!("{},0", lines.start),
        1 => (lines.start + 1).to_string(),
        count => format!("{},{count}", lines.start + 1),
    }
}

fn write_lines(out: &mut String, prefix: char, lines: &[&str]) {
    for line in lines {
        out.push(prefix);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use sych_adapters::git::SnapshotRequest;
    use sych_core::RepoPath;
    use tempfile::TempDir;

    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args([
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .arg("--literal-pathspecs")
            .args(args)
            .output()
            .expect("run Git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn snap(path: &str, before: &str, after: Option<&str>) -> (TempDir, LocalSnapshot) {
        let dir = tempfile::tempdir().expect("temporary repository");
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join(path), before).expect("write base");
        git(repo, &["add", "--", path]);
        git(repo, &["commit", "-qm", "base"]);
        git(repo, &["switch", "-qc", "change"]);
        if let Some(after) = after {
            fs::write(repo.join(path), after).expect("write head");
        } else {
            fs::remove_file(repo.join(path)).expect("delete file");
        }

        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-qm", "head", "--allow-empty"]);
        let snapshot = LocalSnapshot::capture(&SnapshotRequest {
            repo,
            base: "main",
            head: "HEAD",
            diff_base: None,
            max_blob_bytes: 1 << 20,
        })
        .expect("snapshot");
        (dir, snapshot)
    }

    fn anchor(snap: &LocalSnapshot, path: &str, side: SourceSide, line: u32) -> ReportAnchor {
        let path: RepoPath = path.parse().expect("repository path");
        ReportAnchor {
            reference: snap.code_ref(side, &path, line, line).expect("reference"),
            excerpt: None,
            diff: None,
        }
    }

    fn numbered_lines(count: u32) -> String {
        let mut text = String::new();
        for n in 1..=count {
            let _ = writeln!(&mut text, "line {n}");
        }
        text
    }

    #[test]
    fn excerpt_clamps_to_start_and_end_and_a_bad_anchor_stays_unavailable() {
        let stem = numbered_lines(12);
        let head = stem.replacen("line 1\n", "changed 1\n", 1);
        let (_dir, snap) = snap("file.txt", &stem, Some(&head));
        let redactor = Redactor::builtin();
        let mut first = anchor(&snap, "file.txt", SourceSide::Head, 1);
        enrich(&snap, &mut first, redactor);
        let first = first.excerpt.expect("first excerpt");
        assert_eq!(first.first_line, 1);
        assert_eq!(first.text, "changed 1\nline 2\nline 3\nline 4\nline 5\n");
        let mut last = anchor(&snap, "file.txt", SourceSide::Head, 12);
        enrich(&snap, &mut last, redactor);
        assert!(last.diff.is_none(), "the hunk is remote from the last line");
        let excerpt = last.excerpt.expect("last excerpt");
        assert_eq!(excerpt.first_line, 8);
        assert_eq!(excerpt.text, "line 8\nline 9\nline 10\nline 11\nline 12\n");

        let mut drift = anchor(&snap, "file.txt", SourceSide::Head, 1);
        drift.reference.content_sha256 = sych_core::sha256(b"unrelated");
        enrich(&snap, &mut drift, redactor);
        assert!(drift.excerpt.is_none() && drift.diff.is_none());
    }

    #[test]
    fn a_deleted_anchor_uses_base_and_the_diff_preserves_the_eof_marker() {
        let (_dir, snap) = snap("deleted file.txt", "context\r\ndeleted", None);
        let mut anchor = anchor(&snap, "deleted file.txt", SourceSide::DiffBase, 2);
        enrich(&snap, &mut anchor, Redactor::builtin());
        assert_eq!(anchor.excerpt.expect("base code").text, "context\ndeleted");
        assert_eq!(
            anchor.diff.expect("deletion"),
            concat!(
                "--- \"a/deleted file.txt\"\n+++ /dev/null\n@@ -1,2 +0,0 @@\n",
                "-context\n-deleted\n\\ No newline at end of file\n",
            )
        );
    }

    #[test]
    fn nearby_edits_share_three_context_lines_and_remote_hunks_are_omitted() {
        let base = numbered_lines(22);
        let head = format!(
            "lead\n{}",
            base.replacen("line 3\n", "rewrite\n", 1)
                .replacen("line 22\n", "remote\n", 1)
        );
        let (_dir, snap) = snap("file.txt", &base, Some(&head));
        let mut anchor = anchor(&snap, "file.txt", SourceSide::Head, 1);
        enrich(&snap, &mut anchor, Redactor::builtin());
        assert_eq!(
            anchor.diff.expect("grouped hunk"),
            concat!(
                "--- a/file.txt\n+++ b/file.txt\n@@ -1,6 +1,7 @@\n",
                "+lead\n line 1\n line 2\n-line 3\n+rewrite\n line 4\n line 5\n line 6\n",
            ),
        );
    }

    #[test]
    fn unchanged_files_with_pathspec_punctuation_resolve_as_literal_paths() {
        let path = ":(literal)source.txt";
        let (_dir, snap) = snap(path, "pinned\n", Some("pinned\n"));
        let mut anchor = anchor(&snap, path, SourceSide::Head, 1);
        enrich(&snap, &mut anchor, Redactor::builtin());
        assert_eq!(anchor.excerpt.expect("literal path").text, "pinned\n");
        assert!(anchor.diff.is_none());
    }

    #[test]
    fn display_masks_multiline_configured_secrets_and_visible_controls() {
        let redactor =
            Redactor::new(&[], &[], &["(?s)start secret.*end secret".into()]).expect("redactor");
        assert_eq!(
            safe_source("one\nstart secret\nbody\nend secret\ntwo", &redactor),
            "[REDACTED]\n[REDACTED]\n[REDACTED]\n[REDACTED]\n[REDACTED]",
        );
        assert_eq!(
            safe_source("left\u{202e}\u{001b}\r\nright", &redactor),
            "leftU+202EU+001B\nright"
        );
    }
}
