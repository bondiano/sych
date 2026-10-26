//! Reads the `.sych/` policy tree of one pinned commit from Git objects (spec §6.2 step 8).

use sych_core::RepoPath;
use sych_core::config::{POLICY_DIR, PolicyFiles};

use super::parse::{TreeRecord, tree_records};
use super::{Git, SnapshotError};

/// Policy files are small; anything larger is reported, not read.
pub const MAX_POLICY_FILE_BYTES: u64 = 1 << 20;

/// Lists `.sych/` at `commit_hex` with `ls-tree`; symlinks and submodules are never followed.
pub fn read_policy_files(git: &Git, commit_hex: &str) -> Result<PolicyFiles, SnapshotError> {
    let out = git.stdout([
        "ls-tree",
        "-r",
        "-z",
        "-l",
        "--full-tree",
        commit_hex,
        "--",
        POLICY_DIR.trim_end_matches('/'),
    ])?;
    let mut files = PolicyFiles::default();
    for record in tree_records(&out) {
        let bad = || SnapshotError::Git("unparseable ls-tree output".into());
        let TreeRecord {
            mode,
            oid,
            size,
            path: raw_path,
            ..
        } = record.ok_or_else(bad)?;
        let Ok(path) = String::from_utf8(raw_path.to_vec()) else {
            return Err(SnapshotError::Unavailable {
                reason_code: "POLICY_PATH_NOT_UTF8",
                detail: format!("{} under {POLICY_DIR}", String::from_utf8_lossy(raw_path)),
            });
        };
        let path: RepoPath = path
            .parse()
            .map_err(|e: sych_core::FormatError| SnapshotError::Git(e.to_string()))?;
        let unreadable = match (mode, size.parse::<u64>()) {
            ("120000", _) => Some("symlink is not followed".to_owned()),
            ("160000", _) => Some("submodule is not a policy file".to_owned()),
            (_, Ok(n)) if n > MAX_POLICY_FILE_BYTES => Some(format!(
                "{n} bytes exceeds the {MAX_POLICY_FILE_BYTES} byte policy file limit"
            )),
            (_, Ok(_)) => None,
            (_, Err(_)) => return Err(bad()),
        };
        match unreadable {
            Some(reason) => {
                files.unreadable.insert(path, reason);
            }
            None => {
                files
                    .files
                    .insert(path, git.stdout(["cat-file", "blob", oid])?);
            }
        }
    }
    Ok(files)
}
