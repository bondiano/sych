//! Targeted checks (spec §22.4, roadmap M3.8). Every allowed profile runs on the pinned head,
//! materialized into a fresh workspace, in docker with no network, bounded memory, CPU and
//! processes, and no secrets in its environment or mounts. Its stdout is imported as a CI
//! report bound to the head; a refusal or a failure to run is a gap, never a passed check.
//! With `compare_base` it also runs on the diff base, which only labels head outcomes.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use sych_core::config::{CheckRequest, Execution, ExecutionProfile};
use sych_core::{CiFacts, CiGap, CiManifest, CiTest, SchemaVersion, SourceSide};

use crate::ci;
use crate::git::LocalSnapshot;
use crate::sandbox::{self, Isolation, Job, Mount, SandboxError};

/// Largest single file written into the check's workspace.
const MAX_BLOB_BYTES: u64 = 8 << 20;

/// Facts of the profiles that ran and gaps of those that did not. Without requested `checks`
/// every allowed profile runs with no parameters; with them, only the requested ones run.
pub fn run_profiles(
    snapshot: &LocalSnapshot,
    execution: &Execution,
    checks: &[CheckRequest],
) -> (Vec<CiFacts>, Vec<CiGap>) {
    let (mut facts, mut gaps) = (Vec::new(), Vec::new());
    // A requested check on a disabled server is refused below, so the request leaves a gap.
    if !execution.enabled && checks.is_empty() {
        return (facts, gaps);
    }

    let all: Vec<CheckRequest>;
    let checks = if checks.is_empty() {
        all = execution
            .allowed_profiles
            .iter()
            .map(|p| CheckRequest {
                profile: p.clone(),
                params: BTreeMap::new(),
            })
            .collect();
        &all
    } else {
        checks
    };
    for check in checks {
        match run_one(snapshot, execution, check) {
            Ok(Some(f)) => facts.push(f),
            Ok(None) => {}
            Err(gap) => gaps.push(gap),
        }
    }
    (facts, gaps)
}

/// What one run of a profile on one side left behind.
struct Ran {
    stdout: String,
    exit: Option<String>,
    left_out: usize,
}

fn run_one(
    snapshot: &LocalSnapshot,
    execution: &Execution,
    check: &CheckRequest,
) -> Result<Option<CiFacts>, CiGap> {
    let name = check.profile.as_str();
    let label = check.label();
    let gap = |code: &str, detail: String| CiGap {
        report: format!("execution:{label}"),
        code: code.into(),
        detail,
    };
    let argv = execution
        .command(name, &check.params)
        .map_err(|e| gap("EXECUTION_REFUSED", e.to_string()))?;
    let Some(profile) = execution.profiles.get(name) else {
        return Err(gap("EXECUTION_REFUSED", "profile is not defined".into()));
    };
    let ran = run_side(snapshot, profile, &argv, &label, SourceSide::Head)
        .map_err(|e| gap("EXECUTION_FAILED", e))?;
    let Some(format) = profile.report else {
        return Ok(None);
    };
    let manifest = CiManifest {
        schema_version: SchemaVersion,
        job: format!("execution:{label}"),
        tested_oid: snapshot.snapshot.head.hex().to_owned(),
        toolchain: profile.image.clone(),
        target: "docker".into(),
        features: Vec::new(),
    };
    let import = |text: &str| {
        ci::import_as(
            snapshot,
            &format!("{label}.out"),
            text,
            Some(&manifest),
            Some(format),
            None,
        )
        .map_err(|e| gap("CI_REPORT_MALFORMED", e.to_string()))
    };
    let mut facts = import(&ran.stdout)?;
    facts.runner = Some(format!("{label} in {}", profile.image));
    facts.limitations.extend(ran.exit);
    if ran.left_out > 0 {
        facts.limitations.push(format!(
            "{} paths of the head were not in the check's workspace (secrets, links, \
             submodules, tool configs or size)",
            ran.left_out
        ));
    }

    if profile.compare_base {
        // The base run only labels head outcomes; it is never evidence of its own.
        let base = run_side(snapshot, profile, &argv, &label, SourceSide::DiffBase).and_then(|r| {
            let facts = import(&r.stdout).map_err(|g| g.detail)?;
            Ok((r, facts))
        });
        match base {
            Ok((ran, base)) => {
                // A difference may come from an incomplete base workspace, so that is named too.
                facts
                    .limitations
                    .extend(ran.exit.map(|e| format!("BASE: {e}")));
                if ran.left_out > 0 {
                    facts.limitations.push(format!(
                        "BASE: {} paths of the diff base were not in the check's workspace",
                        ran.left_out
                    ));
                }

                facts.limitations.extend(differs(&base.tests, &facts.tests));
            }
            Err(e) => facts
                .limitations
                .push(format!("BASE_NOT_RUN: {e}; head outcomes are not compared")),
        }
    }

    Ok(Some(facts))
}

/// Head outcomes whose status on the diff base was different, by suite and name.
fn differs(base: &[CiTest], head: &[CiTest]) -> Vec<String> {
    head.iter()
        .filter_map(|h| {
            let b = base
                .iter()
                .find(|b| b.suite == h.suite && b.name == h.name)?;
            (b.status != h.status).then(|| {
                format!(
                    "BASE_DIFFERS: {}/{} is {:?} on the diff base and {:?} on the head",
                    h.suite, h.name, b.status, h.status
                )
            })
        })
        .collect()
}

fn run_side(
    snapshot: &LocalSnapshot,
    profile: &ExecutionProfile,
    argv: &[String],
    name: &str,
    side: SourceSide,
) -> Result<Ran, String> {
    let io = |e: io::Error| e.to_string();
    let dir = tempfile::tempdir().map_err(io)?;
    let root = dir.path().canonicalize().map_err(io)?;
    let workspace = root.join("src");
    fs::create_dir(&workspace).map_err(io)?;
    let written = snapshot
        .materialize(side, &workspace, MAX_BLOB_BYTES)
        .map_err(|e| e.to_string())?;
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
        Err(e) => return Err(e.to_string()),
    };
    Ok(Ran {
        stdout: if profile.report.is_some() {
            fs::read_to_string(&stdout).map_err(io)?
        } else {
            String::new()
        },
        exit,
        left_out: written.skipped.len() + written.tool_configs.len() + written.over_budget,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use sych_core::config::ProfileArg;
    use sych_core::{CiFormat, TestStatus};

    use super::*;
    use crate::git::SnapshotRequest;

    fn snapshot(dir: &Path) -> LocalSnapshot {
        snapshot_of(dir, &[("a.txt", "a\n")], &[("a.txt", "b\n")])
    }

    fn snapshot_of(dir: &Path, base: &[(&str, &str)], head: &[(&str, &str)]) -> LocalSnapshot {
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
        let write = |files: &[(&str, &str)]| {
            for (path, text) in files {
                fs::write(dir.join(path), text).expect("write");
            }
        };
        git(&["init", "-q", "-b", "main"]);
        write(base);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["branch", "target"]);
        write(head);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "head"]);
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
            compare_base: false,
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
        let (facts, gaps) = run_profiles(&snap, &off, &[]);
        assert!(facts.is_empty() && gaps.is_empty());
        let asked = CheckRequest {
            profile: "outcomes".into(),
            params: BTreeMap::new(),
        };
        let (facts, gaps) = run_profiles(&snap, &off, &[asked]);
        assert!(facts.is_empty());
        assert_eq!(
            (gaps[0].code.as_str(), gaps[0].detail.as_str()),
            ("EXECUTION_REFUSED", "execution is disabled on this server"),
            "a requested check is never dropped silently"
        );

        let needs_param = execution(vec![ProfileArg::Choice {
            param: "flags".into(),
            one_of: vec!["-x".into()],
        }]);
        let (facts, gaps) = run_profiles(&snap, &needs_param, &[]);
        assert!(facts.is_empty());
        assert_eq!(gaps[0].code, "EXECUTION_REFUSED");

        // A requested value outside the choices and an unlisted profile are refused too.
        let check = |profile: &str, value: &str| CheckRequest {
            profile: profile.into(),
            params: BTreeMap::from([("flags".into(), value.into())]),
        };
        let (facts, gaps) = run_profiles(
            &snap,
            &needs_param,
            &[check("outcomes", "-x; id"), check("other", "-x")],
        );
        assert!(facts.is_empty());
        let reports: Vec<&str> = gaps.iter().map(|g| g.report.as_str()).collect();
        assert_eq!(
            reports,
            [
                r#"execution:outcomes{"flags":"-x; id"}"#,
                r#"execution:other{"flags":"-x"}"#
            ],
            "a check is named with its parameters"
        );
        let details: Vec<&str> = gaps.iter().map(|g| g.detail.as_str()).collect();
        assert_eq!(
            details,
            [
                "parameter `flags` value `-x; id` is not one of the profile's choices",
                "profile `other` is not allowed",
            ]
        );
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
            &[],
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
                .is_some_and(|r| r.starts_with("outcomes in alpine"))
        );
    }

    #[test]
    fn an_outcome_that_differs_from_the_base_is_named() {
        let t = |name: &str, status| CiTest {
            name: name.into(),
            suite: "assertion".into(),
            file: None,
            status,
            message: None,
        };
        let base = [
            t("retry", TestStatus::Passed),
            t("order", TestStatus::Passed),
        ];
        let head = [
            t("retry", TestStatus::Failed),
            t("order", TestStatus::Passed),
            t("new", TestStatus::Failed),
        ];
        assert_eq!(
            differs(&base, &head),
            ["BASE_DIFFERS: assertion/retry is Passed on the diff base and Failed on the head"]
        );
    }

    /// The retry/outbox trace of eval case go-outbox-goroutine: the base retries a publish after
    /// a crash, the head drops it. Needs docker: `cargo test -p sych-adapters -- --ignored execution`.
    #[test]
    #[ignore = "needs docker"]
    fn a_retry_trace_on_base_and_head_names_the_broken_event_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let check = "events=; crashed=; \
             publish() { if [ -z \"$crashed\" ]; then crashed=1; events=\"$events crash\"; return 1; fi; events=\"$events publish\"; }\n\
             . ./relay.sh\n\
             relay\n\
             json=$(printf '%s\\n' $events | sed 's/.*/\"&\"/' | paste -sd, -)\n\
             printf '{\"schema_version\":1,\"assertions\":[{\"name\":\"publish retried after a crash\",\"expected\":[\"commit\",\"crash\",\"publish\"],\"observed\":[%s]}]}' \"$json\"\n";
        let snap = snapshot_of(
            dir.path(),
            &[
                // A secret is never materialized, on either side.
                (".env", "TOKEN=x\n"),
                ("check.sh", check),
                (
                    "relay.sh",
                    "relay() { events=\"commit\"; publish || publish; }\n",
                ),
            ],
            &[(
                "relay.sh",
                "relay() { events=\"commit\"; publish || true; }\n",
            )],
        );
        // The script is a requested choice, so the parameter path runs end to end.
        let mut e = execution(vec![ProfileArg::Choice {
            param: "script".into(),
            one_of: vec!["check.sh".into()],
        }]);
        let p = e.profiles.get_mut("outcomes").expect("profile");
        p.compare_base = true;
        let check = CheckRequest {
            profile: "outcomes".into(),
            params: BTreeMap::from([("script".into(), "check.sh".into())]),
        };
        let (facts, gaps) = run_profiles(&snap, &e, &[check]);
        assert!(gaps.is_empty(), "{gaps:?}");
        let f = &facts[0];
        assert_eq!(
            f.tests[0].status,
            TestStatus::Failed,
            "{:?}",
            f.tests[0].message
        );
        assert!(
            f.limitations.iter().any(|l| l
                == "BASE_DIFFERS: assertion/publish retried after a crash is Passed on the diff base and Failed on the head"),
            "{:?}",
            f.limitations
        );
        assert!(
            f.limitations
                .iter()
                .any(|l| l == "BASE: 1 paths of the diff base were not in the check's workspace"),
            "{:?}",
            f.limitations
        );
        assert!(f.bound_to(&snap.snapshot));
    }
}
