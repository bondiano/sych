//! Isolated tooling profile: whether dependencies of the
//! reviewed repository are installed for semantic analysis, and in which sandbox.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackend {
    Bwrap,
    Docker,
}

/// What happens when no listed backend can run or the install fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnUnavailable {
    /// The review stops with an error.
    #[default]
    Fail,
    /// Analysis continues without dependencies and says so.
    Degrade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Sandbox {
    /// Tried in order; the first that runs is used.
    pub backends: Vec<SandboxBackend>,
    pub on_unavailable: OnUnavailable,
    /// Image for the `docker` backend, pinned by digest (`name@sha256:…`).
    pub docker_image: Option<String>,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self {
            backends: vec![SandboxBackend::Bwrap, SandboxBackend::Docker],
            on_unavailable: OnUnavailable::Fail,
            docker_image: None,
        }
    }
}

/// Bounds of each run of the pinned SCIP indexers, architecture review's and easement's. Past
/// one the run is killed or its index left unread, and the project is reported as not
/// analysed, never as clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Indexers {
    /// Wall-clock seconds of one run.
    pub timeout_seconds: u32,
    /// Resident memory the run and every process it starts hold together.
    pub memory_mb: u32,
    /// A larger index is not read.
    pub max_index_mb: u32,
    /// Projects one review analyses; the rest of the change is analysed in part.
    pub max_projects: u32,
    /// Source files of one project; a larger one is not indexed.
    pub max_files: u32,
}

impl Indexers {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for Indexers {
    fn default() -> Self {
        Self {
            timeout_seconds: 600,
            memory_mb: 4096,
            max_index_mb: 512,
            max_projects: 4,
            max_files: 5000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tooling {
    /// Install the lockfile's dependencies (`pnpm install --ignore-scripts`, `cargo fetch`)
    /// in the sandbox before semantic analysis. Off by default.
    pub dependency_install: bool,
    pub sandbox: Sandbox,
    /// Left out of the config hash while it is the default, so existing policy ids stay.
    #[serde(skip_serializing_if = "Indexers::is_default")]
    pub indexers: Indexers,
}

impl Tooling {
    /// `(field, problem)` for every violated rule.
    pub(super) fn problems(&self) -> Vec<(&'static str, &'static str)> {
        let s = &self.sandbox;
        let mut out = Vec::new();
        if self.dependency_install && s.backends.is_empty() {
            out.push((
                "tooling.sandbox.backends",
                "dependency install needs at least one backend",
            ));
        }

        let pinned = s
            .docker_image
            .as_deref()
            .is_some_and(|i| i.split_once("@sha256:").is_some_and(|(_, d)| d.len() == 64));
        let i = &self.indexers;
        if [
            i.timeout_seconds,
            i.memory_mb,
            i.max_index_mb,
            i.max_projects,
            i.max_files,
        ]
        .contains(&0)
        {
            out.push((
                "tooling.indexers",
                "timeout_seconds, memory_mb, max_index_mb, max_projects and max_files must be \
                 greater than zero",
            ));
        }

        if self.dependency_install && s.backends.contains(&SandboxBackend::Docker) && !pinned {
            out.push((
                "tooling.sandbox.docker_image",
                "the docker backend needs an image pinned by @sha256 digest",
            ));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_yaml;

    #[test]
    fn indexer_bounds_default_and_none_may_be_zero() {
        let parsed: Tooling =
            parse_yaml("sych.yaml", "indexers:\n  memory_mb: 8192\n").expect("tooling");
        assert_eq!(parsed.indexers.memory_mb, 8192);
        assert_eq!(
            parsed.indexers,
            Indexers {
                memory_mb: 8192,
                ..Indexers::default()
            }
        );
        assert_eq!(parsed.problems(), Vec::new());
        let zero: Tooling =
            parse_yaml("sych.yaml", "indexers:\n  timeout_seconds: 0\n").expect("tooling");
        assert_eq!(zero.problems()[0].0, "tooling.indexers");
        assert!(parse_yaml::<Tooling>("sych.yaml", "indexers:\n  memory: 1\n").is_err());
    }
}
