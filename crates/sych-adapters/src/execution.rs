//! Targeted checks (spec §22.4, roadmap M3.8). Every allowed profile runs on the pinned head,
//! materialized into a fresh workspace, in docker with no network, bounded memory, CPU and
//! processes, and no secrets in its environment or mounts. Its stdout is imported as a CI
//! report bound to the head; a refusal or a failure to run is a gap, never a passed check.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use sych_core::config::Execution;
use sych_core::{CiFacts, CiGap, CiManifest, SchemaVersion, SourceSide};

use crate::ci;
use crate::git::LocalSnapshot;
use crate::sandbox::{self, Isolation, Job, Mount, SandboxError};

/// Largest single file written into the check's workspace.
const MAX_BLOB_BYTES: u64 = 8 << 20;

/// Facts of the profiles that ran and gaps of those that did not.
pub fn run_profiles(snapshot: &LocalSnapshot, execution: &Execution) -> (Vec<CiFacts>, Vec<CiGap>) {
    let (mut facts, mut gaps) = (Vec::new(), Vec::new());
    if !execution.enabled {
        return (facts, gaps);
    }

    for name in &execution.allowed_profiles {
        match run_one(snapshot, execution, name) {
            Ok(Some(f)) => facts.push(f),
            Ok(None) => {}
            Err(gap) => gaps.push(gap),
        }
    }
    (facts, gaps)
}

fn run_one(
    snapshot: &LocalSnapshot,
    execution: &Execution,
    name: &str,
) -> Result<Option<CiFacts>, CiGap> {
    let gap = |code: &str, detail: String| CiGap {
        report: format!("execution:{name}"),
        code: code.into(),
        detail,
    };
    // shortcut: no request carries parameters yet, so a profile with choices is refused as
    // missing them; pass them through here once the verifier can ask for a check.
    let argv = execution
        .command(name, &BTreeMap::new())
        .map_err(|e| gap("EXECUTION_REFUSED", e.to_string()))?;
    let Some(profile) = execution.profiles.get(name) else {
        return Err(gap("EXECUTION_REFUSED", "profile is not defined".into()));
    };
    let io = |e: io::Error| gap("EXECUTION_FAILED", e.to_string());
    let dir = tempfile::tempdir().map_err(io)?;
    let root = dir.path().canonicalize().map_err(io)?;
    let workspace = root.join("src");
    fs::create_dir(&workspace).map_err(io)?;
    let written = snapshot
        .materialize(SourceSide::Head, &workspace, MAX_BLOB_BYTES)
        .map_err(|e| gap("EXECUTION_FAILED", e.to_string()))?;
    let stdout = root.join("stdout");
    let (program, rest) = argv.split_first().expect("argv starts with the executable");
    let job = Job {
        program: Path::new(program),
        args: rest.to_vec(),
        cwd: &workspace,
        mounts: vec![Mount {
            path: workspace.clone(),
            writable: true,
        }],
        env: Vec::new(),
        timeout: Duration::from_secs(profile.timeout_seconds.into()),
    };
    let isolation = Isolation {
        memory_mb: profile.memory_mb,
        cpu_millis: profile.cpu_millis,
        pids: profile.pids,
    };
    // A non-zero exit is a result (tests failed), not a failure to run.
    let exit = match sandbox::run_isolated(&profile.image, &job, &isolation, &stdout) {
        Ok(()) => None,
        Err(SandboxError::Exited { status, .. }) => Some(format!("{name} exited with {status}")),
        Err(e) => return Err(gap("EXECUTION_FAILED", e.to_string())),
    };
    let Some(format) = profile.report else {
        return Ok(None);
    };
    let text = fs::read_to_string(&stdout).map_err(io)?;
    let manifest = CiManifest {
        schema_version: SchemaVersion,
        job: format!("execution:{name}"),
        tested_oid: snapshot.snapshot.head.hex().to_owned(),
        toolchain: profile.image.clone(),
        target: "docker".into(),
        features: Vec::new(),
    };
    let mut facts = ci::import_as(
        snapshot,
        &format!("{name}.out"),
        &text,
        Some(&manifest),
        Some(format),
        None,
    )
    .map_err(|e| gap("CI_REPORT_MALFORMED", e.to_string()))?;
    facts.runner = Some(format!("{name}@{}", profile.image));
    facts.limitations.extend(exit);
    let left_out = written.skipped.len() + written.tool_configs.len() + written.over_budget;
    if left_out > 0 {
        facts.limitations.push(format!(
            "{left_out} paths of the head were not in the check's workspace (secrets, links, \
             submodules, tool configs or size)"
        ));
    }

    Ok(Some(facts))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use sych_core::CiFormat;
    use sych_core::config::{ExecutionProfile, ProfileArg};

    use super::*;
    use crate::git::SnapshotRequest;

    fn snapshot(dir: &Path) -> LocalSnapshot {
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .current_dir(dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        fs::write(dir.join("a.txt"), "a\n").expect("write");
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["branch", "target"]);
        fs::write(dir.join("a.txt"), "b\n").expect("write");
        git(&["commit", "-qam", "head"]);
        LocalSnapshot::capture(&SnapshotRequest {
            repo: dir,
            base: "target",
            head: "HEAD",
            diff_base: None,
            max_blob_bytes: 1 << 20,
        })
        .expect("snapshot")
    }

    fn execution(args: Vec<ProfileArg>) -> Execution {
        let profile = ExecutionProfile {
            image: "alpine:3.20@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc".into(),
            executable: "/bin/sh".into(),
            args,
            timeout_seconds: 60,
            report: Some(CiFormat::SychManifest),
            memory_mb: 256,
            cpu_millis: 500,
            pids: 64,
        };
        Execution {
            enabled: true,
            allowed_profiles: vec!["outcomes".into()],
            profiles: BTreeMap::from([("outcomes".into(), profile)]),
        }
    }

    #[test]
    fn disabled_runs_nothing_and_a_refusal_is_a_gap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snap = snapshot(dir.path());
        let off = Execution::default();
        let (facts, gaps) = run_profiles(&snap, &off);
        assert!(facts.is_empty() && gaps.is_empty());

        let needs_param = execution(vec![ProfileArg::Choice {
            param: "flags".into(),
            one_of: vec!["-x".into()],
        }]);
        let (facts, gaps) = run_profiles(&snap, &needs_param);
        assert!(facts.is_empty());
        assert_eq!(gaps[0].code, "EXECUTION_REFUSED");
    }

    /// Needs a Docker daemon: `cargo test -p sych-adapters -- --ignored execution`.
    #[test]
    #[ignore = "needs docker"]
    fn a_profile_runs_offline_on_the_head_and_its_stdout_is_bound_evidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snap = snapshot(dir.path());
        let script = "grep -q b a.txt || exit 3; \
                      if wget -q -T 3 -O- http://1.1.1.1 >/dev/null 2>&1; then net=1; else net=0; fi; \
                      printf '{\"schema_version\":1,\"commands\":[{\"name\":\"head-read net=%s\",\"exit_code\":0}]}' $net";
        let (facts, gaps) = run_profiles(
            &snap,
            &execution(vec![
                ProfileArg::Literal("-c".into()),
                ProfileArg::Literal(script.into()),
            ]),
        );
        assert!(gaps.is_empty(), "{gaps:?}");
        let f = &facts[0];
        assert_eq!(
            f.tests[0].name, "head-read net=0",
            "the head is mounted, the network is not"
        );
        assert!(f.bound_to(&snap.snapshot));
        assert!(
            f.runner
                .as_deref()
                .is_some_and(|r| r.starts_with("outcomes@alpine"))
        );
    }
}
