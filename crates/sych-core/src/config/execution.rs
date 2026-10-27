//! Targeted checks (spec §22.4, roadmap M3.8): off by default. When an admin enables it, a
//! command comes only from an approved profile, an absolute executable with arguments that
//! are literals or a choice from a closed list; neither the model nor the repository writes
//! a command line. This module is the authorization boundary; no runner ships yet, so
//! existing CI reports remain the evidence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::CiFormat;

/// One argument of a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ProfileArg {
    Literal(String),
    /// Filled from a request parameter, which must be one of `one_of`.
    Choice {
        param: String,
        one_of: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProfile {
    /// The docker image it runs in, pinned by digest (`…@sha256:…`); docker is the only backend.
    pub image: String,
    /// Absolute path inside the image; no lookup in `PATH`.
    pub executable: String,
    #[serde(default)]
    pub args: Vec<ProfileArg>,
    pub timeout_seconds: u32,
    /// Format of what the command prints on stdout; imported like a CI report of the head.
    pub report: Option<CiFormat>,
    #[serde(default = "memory_mb")]
    pub memory_mb: u32,
    /// CPU share in thousandths of a core.
    #[serde(default = "cpu_millis")]
    pub cpu_millis: u32,
    #[serde(default = "pids")]
    pub pids: u32,
}

const fn memory_mb() -> u32 {
    2048
}

const fn cpu_millis() -> u32 {
    2000
}

const fn pids() -> u32 {
    512
}

/// Repository code execution; off unless the server explicitly allows profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Execution {
    pub enabled: bool,
    pub allowed_profiles: Vec<String>,
    /// Skipped when empty, so configs without profiles keep their hash.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ExecutionProfile>,
}

/// Why a targeted check did not run; recorded as such, never as a passed check.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutionRefused {
    #[error("execution is disabled on this server")]
    Disabled,
    #[error("profile `{0}` is not allowed")]
    NotAllowed(String),
    #[error("parameter `{0}` is missing")]
    Missing(String),
    #[error("parameter `{param}` value `{value}` is not one of the profile's choices")]
    Invalid { param: String, value: String },
}

impl Execution {
    /// The argv of an allowed profile with checked parameters; parameters a profile does not
    /// name are ignored, so a request cannot add arguments.
    pub fn command(
        &self,
        profile: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<Vec<String>, ExecutionRefused> {
        if !self.enabled {
            return Err(ExecutionRefused::Disabled);
        }

        let p = self
            .profiles
            .get(profile)
            .filter(|_| self.allowed_profiles.iter().any(|a| a == profile))
            .ok_or_else(|| ExecutionRefused::NotAllowed(profile.to_owned()))?;
        let mut argv = vec![p.executable.clone()];
        for arg in &p.args {
            argv.push(match arg {
                ProfileArg::Literal(s) => s.clone(),
                ProfileArg::Choice { param, one_of } => {
                    let value = params
                        .get(param)
                        .ok_or_else(|| ExecutionRefused::Missing(param.clone()))?;
                    if !one_of.contains(value) {
                        return Err(ExecutionRefused::Invalid {
                            param: param.clone(),
                            value: value.clone(),
                        });
                    }

                    value.clone()
                }
            });
        }
        Ok(argv)
    }

    /// Config problems as `(field, message)`.
    pub(crate) fn problems(&self) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        if self.enabled && self.allowed_profiles.is_empty() {
            out.push((
                "execution.allowed_profiles".to_owned(),
                "enabled execution needs at least one admin-approved profile",
            ));
        }

        for name in &self.allowed_profiles {
            if !self.profiles.contains_key(name) {
                out.push((
                    format!("execution.allowed_profiles.{name}"),
                    "names a profile that is not defined",
                ));
            }
        }
        for (name, p) in &self.profiles {
            if !p.image.contains("@sha256:") {
                out.push((
                    format!("execution.profiles.{name}.image"),
                    "must be pinned by digest (image@sha256:…)",
                ));
            }

            if !p.executable.starts_with('/') {
                out.push((
                    format!("execution.profiles.{name}.executable"),
                    "must be an absolute path",
                ));
            }

            if p.timeout_seconds == 0 {
                out.push((
                    format!("execution.profiles.{name}.timeout_seconds"),
                    "must be greater than zero",
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execution(enabled: bool) -> Execution {
        let yaml = "enabled: true\nallowed_profiles: [go-test]\nprofiles:\n  go-test:\n    image: golang:1.26@sha256:0000000000000000000000000000000000000000000000000000000000000000\n    executable: /usr/local/go/bin/go\n    args: [test, {param: race, one_of: [-race, -count=1]}, ./...]\n    timeout_seconds: 300\n";
        Execution {
            enabled,
            ..serde_yaml_ng::from_str(yaml).expect("execution")
        }
    }

    fn params(race: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("race".to_owned(), race.to_owned())])
    }

    #[test]
    fn only_an_allowed_profile_with_listed_values_builds_a_command() {
        assert_eq!(
            execution(false).command("go-test", &params("-race")),
            Err(ExecutionRefused::Disabled)
        );
        let on = execution(true);
        assert_eq!(
            on.command("go-test", &params("-race")).expect("argv"),
            ["/usr/local/go/bin/go", "test", "-race", "./..."]
        );
        assert!(matches!(
            on.command("go-test", &params("-race; rm -rf /")),
            Err(ExecutionRefused::Invalid { .. })
        ));
        assert_eq!(
            on.command("sh", &params("-race")),
            Err(ExecutionRefused::NotAllowed("sh".into()))
        );
        assert_eq!(
            on.command("go-test", &BTreeMap::new()),
            Err(ExecutionRefused::Missing("race".into()))
        );
        assert!(on.problems().is_empty());
    }

    #[test]
    fn profiles_must_be_defined_absolute_and_bounded() {
        let mut e = execution(true);
        e.allowed_profiles.push("missing".into());
        e.profiles
            .get_mut("go-test")
            .map(|p| {
                p.executable = "go".into();
                p.timeout_seconds = 0;
            })
            .expect("profile");
        let fields: Vec<String> = e.problems().into_iter().map(|(f, _)| f).collect();
        assert_eq!(
            fields,
            [
                "execution.allowed_profiles.missing",
                "execution.profiles.go-test.executable",
                "execution.profiles.go-test.timeout_seconds",
            ]
        );
    }
}
