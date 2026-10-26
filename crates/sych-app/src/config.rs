//! Reading the server config and the working-tree policy for authoring checks.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;

use sych_adapters::git::MAX_POLICY_FILE_BYTES;
use sych_core::RepoPath;
use sych_core::config::{
    EffectiveConfig, POLICY_DIR, PolicyFiles, PolicyInput, ServerConfig, ServerInput,
};

use crate::errors::Failure;

/// The personal example `sych config init` writes.
pub const PERSONAL_EXAMPLE: &str = include_str!("../../../examples/personal/sych.yaml");

/// Server config text, or `None` for engine defaults.
pub fn read_server(config: Option<&Path>) -> anyhow::Result<Option<String>> {
    config
        .map(|path| {
            fs::read_to_string(path)
                .map_err(|err| Failure::invalid(format!("cannot read {}: {err}", path.display())))
        })
        .transpose()
        .map_err(Into::into)
}

/// Parsed server config at `config`, or engine defaults.
pub fn server(config: Option<&Path>) -> anyhow::Result<ServerConfig> {
    match read_server(config)? {
        Some(text) => {
            let origin = config.map_or_else(String::new, |p| p.display().to_string());
            Ok(ServerConfig::parse(&origin, &text).map_err(|errors| Failure::config(&errors))?)
        }
        None => Ok(ServerConfig::engine_default()),
    }
}

/// Reads `<repo>/.sych` for authoring checks. Symlinks are reported, never followed.
fn read_policy_dir(repo: &Path) -> io::Result<PolicyFiles> {
    let mut files = PolicyFiles::default();
    let mut dirs = vec![repo.join(POLICY_DIR)];
    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            other => other?,
        };
        for entry in entries {
            let full = entry?.path();
            let meta = fs::symlink_metadata(&full)?;
            let invalid = |why: &str| io::Error::new(ErrorKind::InvalidData, why.to_owned());
            let rel = full
                .strip_prefix(repo)
                .ok()
                .and_then(Path::to_str)
                .ok_or_else(|| invalid(&format!("{} is not a UTF-8 path", full.display())))?;
            let path: RepoPath = rel.parse().map_err(|e| invalid(&format!("{e}")))?;
            if meta.is_dir() {
                dirs.push(full);
            } else if meta.file_type().is_symlink() {
                files
                    .unreadable
                    .insert(path, "symlink is not followed".into());
            } else if !meta.is_file() || meta.len() > MAX_POLICY_FILE_BYTES {
                files.unreadable.insert(
                    path,
                    format!("not a regular file of at most {MAX_POLICY_FILE_BYTES} bytes"),
                );
            } else {
                files.files.insert(path, fs::read(&full)?);
            }
        }
    }

    Ok(files)
}

/// Effective config of the server config and, when given, the working-tree `.sych/`.
pub fn effective(config: Option<&Path>, repo: Option<&Path>) -> anyhow::Result<EffectiveConfig> {
    let server = read_server(config)?;
    let files = repo
        .map(read_policy_dir)
        .transpose()
        .map_err(|err| Failure::invalid(format!("cannot read {POLICY_DIR}: {err}")))?
        .unwrap_or_default();
    let origin = config.map_or_else(|| "<engine defaults>".into(), |p| p.display().to_string());
    let policy_origin = repo.map_or_else(String::new, |r| {
        format!("{} (working tree, not a trusted ref)", r.display())
    });
    EffectiveConfig::build(
        &ServerInput {
            origin: &origin,
            text: server.as_deref(),
        },
        &PolicyInput {
            origin: &policy_origin,
            files: &files,
        },
    )
    .map_err(|errors| Failure::config(&errors).into())
}
