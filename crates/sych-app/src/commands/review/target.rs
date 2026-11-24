//! Resolving the local review target before pinning its snapshot.

use std::path::Path;

use sych_adapters::git::Git;

use crate::args::Comparison;
use crate::errors::Failure;

pub(super) fn resolve(comparison: &Comparison, target: Option<&str>) -> anyhow::Result<Comparison> {
    let mut comparison = comparison.clone();
    if let Some(target) = target {
        if let Some((base, range)) = target.split_once("..") {
            let (head, three_dot) = range
                .strip_prefix('.')
                .map_or((range, false), |head| (head, true));
            if base.is_empty() || head.is_empty() || head.starts_with('.') || head.contains("..") {
                return Err(Failure::invalid("expected REF, A..B or A...B").into());
            }

            base.clone_into(&mut comparison.base);
            head.clone_into(&mut comparison.head);
            if !three_dot {
                comparison.diff_base = Some(base.to_owned());
            }
        } else {
            target.clone_into(&mut comparison.head);
        }
    }

    if comparison.base.is_empty() {
        comparison.base = match &comparison.diff_base {
            Some(base) => base.clone(),
            None => default_base(&Git::open(&comparison.repo)?)?,
        };
    }

    Ok(comparison)
}

fn default_base(git: &Git) -> anyhow::Result<String> {
    for candidate in [
        "refs/remotes/origin/HEAD",
        "refs/heads/main",
        "refs/heads/master",
    ] {
        let out = git.output([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{candidate}^{{commit}}"),
        ])?;
        if out.status.success() {
            return Ok(candidate.to_owned());
        }

        if out.status.code() != Some(1) {
            return Err(Failure::no_result(format!(
                "cannot resolve {candidate}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into());
        }
    }

    Err(Failure::invalid("no default branch found; specify --base REF").into())
}

pub(super) fn warn_dirty(repo: &Path) {
    match Git::worktree_dirty(repo) {
        Ok(false) => {}
        Ok(true) => eprintln!(
            "warning: reviewing committed changes only; --staged and --worktree are not yet available"
        ),
        Err(_) => eprintln!(
            "warning: could not check for local changes; reviewing committed changes only"
        ),
    }
}
