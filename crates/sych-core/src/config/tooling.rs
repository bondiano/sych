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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tooling {
    /// Install the lockfile's dependencies (`pnpm install --ignore-scripts`, `cargo fetch`)
    /// in the sandbox before semantic analysis. Off by default.
    pub dependency_install: bool,
    pub sandbox: Sandbox,
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
        if self.dependency_install && s.backends.contains(&SandboxBackend::Docker) && !pinned {
            out.push((
                "tooling.sandbox.docker_image",
                "the docker backend needs an image pinned by @sha256 digest",
            ));
        }

        out
    }
}
