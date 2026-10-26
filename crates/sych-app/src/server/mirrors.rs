//! Bare mirrors of reviewed repositories: the source cache (spec §20.4), kept 7 days after its
//! last use and never while a run of that repository is active.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Source cache retention after the last fetch.
pub const UNUSED_SECONDS: i64 = 7 * 86_400;
const MARKER: &str = "sych-last-used";

/// `<data>/mirrors/<connection>/<owner>__<repo>.git`, next to the artifact store.
pub fn path(artifacts: &Path, connection: &str, repo: &str) -> PathBuf {
    let root = artifacts.parent().map_or_else(PathBuf::new, PathBuf::from);
    root.join("mirrors")
        .join(connection)
        .join(format!("{}.git", repo.replace('/', "__")))
}

/// Records a use; the time is the server clock, not the file's mtime.
pub fn touch(mirror: &Path, now: i64) -> io::Result<()> {
    fs::write(mirror.join(MARKER), now.to_string())
}

/// Mirrors unused for longer than [`UNUSED_SECONDS`] and not in `active`; deleted unless
/// `dry_run`. A mirror without a marker counts as unused since forever.
pub fn prune(
    artifacts: &Path,
    active: &[(String, String)],
    now: i64,
    dry_run: bool,
) -> io::Result<Vec<PathBuf>> {
    let pinned: Vec<PathBuf> = active.iter().map(|(c, r)| path(artifacts, c, r)).collect();
    let root = path(artifacts, "_", "_/_");
    let Some(mirrors) = root.parent().and_then(Path::parent) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let connections = match fs::read_dir(mirrors) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for connection in connections {
        for mirror in fs::read_dir(connection?.path())? {
            let mirror = mirror?.path();
            let used = fs::read_to_string(mirror.join(MARKER))
                .ok()
                .and_then(|t| t.trim().parse::<i64>().ok())
                .unwrap_or(i64::MIN);
            if pinned.contains(&mirror) || now.saturating_sub(used) <= UNUSED_SECONDS {
                continue;
            }

            if !dry_run {
                fs::remove_dir_all(&mirror)?;
            }

            out.push(mirror);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unused_mirrors_go_and_active_or_recent_ones_stay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let artifacts = dir.path().join("artifacts");
        for (c, r, used) in [
            ("gh", "o/old", 0),
            ("gh", "o/busy", 0),
            ("gh", "o/recent", 700_000),
        ] {
            let m = path(&artifacts, c, r);
            fs::create_dir_all(&m).expect("mirror");
            touch(&m, used).expect("touch");
        }
        let active = vec![("gh".to_owned(), "o/busy".to_owned())];
        let now = UNUSED_SECONDS + 10;
        let listed = prune(&artifacts, &active, now, true).expect("list");
        assert_eq!(listed, [path(&artifacts, "gh", "o/old")]);
        assert!(listed[0].exists(), "dry run keeps it");
        prune(&artifacts, &active, now, false).expect("prune");
        assert!(!listed[0].exists());
        assert!(path(&artifacts, "gh", "o/busy").exists());
    }
}
