//! `linguist-generated` from `.gitattributes` of the target tip: the trusted revision, as for
//! policy (INV-12), so a change cannot mark its own code as generated to keep it unread.

use std::str;

use sych_core::{FileChange, GitObjectId};

use super::{Git, SnapshotError};

const ATTRIBUTE: &str = "linguist-generated";

/// Marks `files` whose old or new path the target tip's attributes declare generated. A git
/// without `check-attr --source` (before 2.40) marks nothing, so every file stays read.
pub(super) fn mark_generated(
    git: &Git,
    target_tree: &GitObjectId,
    files: &mut [FileChange],
) -> Result<(), SnapshotError> {
    let paths: Vec<&str> = files
        .iter()
        .flat_map(|f| f.old.iter().chain(f.new.iter()))
        .map(|s| s.path.as_str())
        .collect();
    if paths.is_empty() {
        return Ok(());
    }

    // shortcut: paths go through argv; switch to `--stdin` if a change ever nears ARG_MAX.
    let out = git.output(
        [
            "check-attr",
            "-z",
            "--source",
            target_tree.hex(),
            ATTRIBUTE,
            "--",
        ]
        .into_iter()
        .chain(paths),
    )?;
    if !out.status.success() {
        tracing::warn!(
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "git check-attr failed; linguist-generated is not applied"
        );
        return Ok(());
    }

    let generated = generated_paths(&out.stdout);
    for file in files {
        if file
            .old
            .iter()
            .chain(file.new.iter())
            .any(|s| generated.contains(&s.path.as_str()))
        {
            file.generated = true;
        }
    }
    Ok(())
}

/// Paths of `check-attr -z` output (`path NUL attribute NUL value NUL`) whose value is set.
fn generated_paths(out: &[u8]) -> Vec<&str> {
    out.split(|b| *b == 0)
        .collect::<Vec<_>>()
        .chunks_exact(3)
        .filter(|r| matches!(r[2], b"set" | b"true"))
        .filter_map(|r| str::from_utf8(r[0]).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_set_or_true_values_mark_a_path() {
        let out = b"a.json\0linguist-generated\0true\0b.rs\0linguist-generated\0unspecified\0\
                    c.json\0linguist-generated\0set\0d.json\0linguist-generated\0false\0";
        assert_eq!(generated_paths(out), ["a.json", "c.json"]);
    }
}
