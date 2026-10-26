//! Pure parsers for `git diff-tree --raw -z` and `-U0` hunk headers.

use std::str;

use sych_core::{ChangeStatus, Hunk};

/// One entry of `git diff-tree -r -z --raw --no-abbrev`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    pub old_mode: String,
    pub new_mode: String,
    pub old_oid: String,
    pub new_oid: String,
    pub status: ChangeStatus,
    pub similarity: Option<u8>,
    pub old_path: Vec<u8>,
    pub new_path: Vec<u8>,
}

fn status(code: &str) -> (ChangeStatus, Option<u8>) {
    let (letter, score) = code.split_at(code.len().min(1));
    let status = match letter {
        "A" => ChangeStatus::Added,
        "D" => ChangeStatus::Deleted,
        "M" => ChangeStatus::Modified,
        "R" => ChangeStatus::Renamed,
        "C" => ChangeStatus::Copied,
        "T" => ChangeStatus::TypeChanged,
        _ => ChangeStatus::Unknown,
    };
    (status, score.parse().ok())
}

/// Parses NUL-separated raw output. Returns `None` on any malformed record instead of
/// silently skipping files.
pub fn raw_diff(out: &[u8]) -> Option<Vec<RawEntry>> {
    let mut fields = out.split(|&b| b == 0);
    let mut entries = Vec::new();
    while let Some(header) = fields.next() {
        if header.is_empty() {
            // Only the trailing terminator may be empty.
            return fields.next().is_none().then_some(entries);
        }

        let header = str::from_utf8(header.strip_prefix(b":")?).ok()?;
        let [old_mode, new_mode, old_oid, new_oid, code] =
            <[&str; 5]>::try_from(header.split(' ').collect::<Vec<_>>()).ok()?;
        let (status, similarity) = status(code);
        let mut path = || fields.next().filter(|p| !p.is_empty()).map(<[u8]>::to_vec);
        let first = path()?;
        let second = match status {
            ChangeStatus::Renamed | ChangeStatus::Copied => path()?,
            _ => first.clone(),
        };
        entries.push(RawEntry {
            old_mode: old_mode.to_owned(),
            new_mode: new_mode.to_owned(),
            old_oid: old_oid.to_owned(),
            new_oid: new_oid.to_owned(),
            status,
            similarity,
            old_path: first,
            new_path: second,
        });
    }
    Some(entries)
}

fn range(spec: &str) -> Option<(u32, u32)> {
    match spec.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((spec.parse().ok()?, 1)),
    }
}

/// Extracts `@@ -a,b +c,d @@` headers from unified diff output.
pub fn hunks(patch: &[u8]) -> Option<Vec<Hunk>> {
    patch
        .split(|&b| b == b'\n')
        .filter(|line| line.starts_with(b"@@ -"))
        .map(|line| {
            let line = str::from_utf8(line).ok()?;
            let mut parts = line.split(' ');
            let (old_start, old_lines) = range(parts.nth(1)?.strip_prefix('-')?)?;
            let (new_start, new_lines) = range(parts.next()?.strip_prefix('+')?)?;
            Some(Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
            })
        })
        .collect()
}

/// One record of `git ls-tree -z -l`: `<mode> <type> <oid> <size>\t<path>`.
pub struct TreeRecord<'a> {
    pub mode: &'a str,
    pub kind: &'a str,
    pub oid: &'a str,
    pub size: &'a str,
    pub path: &'a [u8],
}

/// Records of `ls-tree -z -l` output; `None` for one that does not have that shape. Each
/// caller keeps its own policy for malformed records and non-UTF-8 paths.
pub fn tree_records(out: &[u8]) -> impl Iterator<Item = Option<TreeRecord<'_>>> {
    out.split(|&b| b == 0).filter(|r| !r.is_empty()).map(|r| {
        let tab = r.iter().position(|&b| b == b'\t')?;
        let mut fields = str::from_utf8(&r[..tab]).ok()?.split_ascii_whitespace();
        let record = TreeRecord {
            mode: fields.next()?,
            kind: fields.next()?,
            oid: fields.next()?,
            size: fields.next()?,
            path: &r[tab + 1..],
        };
        fields.next().is_none().then_some(record)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z40: &str = "0000000000000000000000000000000000000000";
    const A40: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn parses_rename_and_plain_records() {
        let out = format!(
            ":100644 100644 {A40} {A40} R087\0old name.rs\0новое.rs\0:000000 120000 {Z40} {A40} A\0link\0"
        );
        let entries = raw_diff(out.as_bytes()).expect("parses");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, ChangeStatus::Renamed);
        assert_eq!(entries[0].similarity, Some(87));
        assert_eq!(entries[0].old_path, b"old name.rs");
        assert_eq!(entries[0].new_path, "новое.rs".as_bytes());
        assert_eq!(entries[1].status, ChangeStatus::Added);
        assert_eq!(entries[1].new_mode, "120000");
        assert_eq!(raw_diff(b""), Some(vec![]));
    }

    #[test]
    fn tree_records_keep_paths_raw_and_flag_bad_shapes() {
        let out = format!("100644 blob {A40}      12\ta b.rs\0160000 commit {A40} -\tsub\0bad\0");
        let records: Vec<_> = tree_records(out.as_bytes()).collect();
        let first = records[0].as_ref().expect("record");
        assert_eq!(
            (first.mode, first.kind, first.size),
            ("100644", "blob", "12")
        );
        assert_eq!(first.path, b"a b.rs");
        assert_eq!(records[1].as_ref().map(|r| r.kind), Some("commit"));
        assert!(records[2].is_none());
    }

    #[test]
    fn malformed_raw_output_is_rejected() {
        assert_eq!(raw_diff(b":100644 100644 x M\0a\0"), None);
        assert_eq!(
            raw_diff(format!(":100644 100644 {A40} {A40} R100\0only-old\0").as_bytes()),
            None
        );
    }

    #[test]
    fn parses_zero_context_hunks() {
        let patch = b"diff --git a/x b/x\n@@ -3 +3,2 @@ fn x\n-a\n+b\n+c\n@@ -10,2 +11,0 @@\n";
        assert_eq!(
            hunks(patch),
            Some(vec![
                Hunk {
                    old_start: 3,
                    old_lines: 1,
                    new_start: 3,
                    new_lines: 2
                },
                Hunk {
                    old_start: 10,
                    old_lines: 2,
                    new_start: 11,
                    new_lines: 0
                },
            ])
        );
    }
}
