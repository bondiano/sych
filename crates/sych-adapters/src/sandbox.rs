//! Running repository tooling (dependency installers) in a sandbox.
//!
//! Every mount keeps its host path inside the sandbox. bwrap sees the host root read-only
//! with the home directory, `/tmp`, `/root`, `/run/user` and `/run/secrets` replaced by
//! empty tmpfs, so credentials there are invisible; only the listed mounts come back. The
//! environment is cleared to the variables a toolchain needs to find itself.

use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sych_core::config::SandboxBackend;
use thiserror::Error;

/// Passed through so a dynamically linked tool finds its loader (nix-ld) and locale.
const PASSED_ENV: &[&str] = &["PATH", "LANG", "NIX_LD", "NIX_LD_LIBRARY_PATH"];
const HIDDEN: &[&str] = &["/tmp", "/root", "/run/user", "/run/secrets"];
const TAIL_BYTES: u64 = 4096;
const POLL: Duration = Duration::from_millis(200);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Mount {
    pub path: PathBuf,
    pub writable: bool,
}

pub struct Job<'a> {
    pub program: &'a Path,
    pub args: Vec<String>,
    pub cwd: &'a Path,
    pub mounts: Vec<Mount>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

/// Limits for repository code run as a targeted check: no network, bounded memory, CPU and
/// processes. Installers keep the network they need and run without them.
pub struct Isolation {
    pub memory_mb: u32,
    pub cpu_millis: u32,
    pub pids: u32,
}

pub fn name(backend: SandboxBackend) -> &'static str {
    match backend {
        SandboxBackend::Bwrap => "bwrap",
        SandboxBackend::Docker => "docker",
    }
}

fn env_of(job: &Job<'_>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = PASSED_ENV
        .iter()
        .filter_map(|k| env::var(k).ok().map(|v| ((*k).to_owned(), v)))
        .collect();
    out.push(("HOME".into(), "/tmp".into()));
    out.push(("TMPDIR".into(), "/tmp".into()));
    out.extend(job.env.iter().cloned());
    out
}

fn bwrap(job: &Job<'_>) -> Command {
    let mut cmd = Command::new("bwrap");
    cmd.args([
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--share-net",
    ]);
    cmd.args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"]);
    let home = env::var_os("HOME").map(PathBuf::from);
    for hidden in HIDDEN.iter().map(PathBuf::from).chain(home) {
        if hidden.is_dir() {
            cmd.arg("--tmpfs").arg(&hidden);
        }
    }
    for m in &job.mounts {
        cmd.arg(if m.writable { "--bind" } else { "--ro-bind" })
            .arg(&m.path)
            .arg(&m.path);
    }
    cmd.arg("--chdir").arg(job.cwd).arg("--clearenv");
    for (k, v) in env_of(job) {
        cmd.args(["--setenv", &k, &v]);
    }
    cmd.arg("--").arg(job.program).args(&job.args);
    cmd
}

/// A container name unique to this job, so the container can be removed by it.
fn container_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "sych-{}-{nanos}-{}",
        process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn docker(job: &Job<'_>, image: &str, container: &str, isolation: Option<&Isolation>) -> Command {
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "--name", container]);
    let network = if isolation.is_some() {
        "none"
    } else {
        "bridge"
    };
    cmd.args(["--network", network, "--cap-drop", "ALL"]);
    if let Some(i) = isolation {
        cmd.arg(format!("--memory={}m", i.memory_mb))
            .arg(format!(
                "--cpus={}.{:03}",
                i.cpu_millis / 1000,
                i.cpu_millis % 1000
            ))
            .arg(format!("--pids-limit={}", i.pids));
    }

    cmd.args([
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--tmpfs",
        "/tmp:exec",
    ]);
    if let Ok(meta) = fs::metadata(job.cwd) {
        cmd.args(["--user", &format!("{}:{}", meta.uid(), meta.gid())]);
    }

    for m in &job.mounts {
        let mode = if m.writable { "rw" } else { "ro" };
        cmd.arg("-v")
            .arg(format!("{0}:{0}:{mode}", m.path.display()));
    }
    cmd.arg("-w").arg(job.cwd);
    for (k, v) in env_of(job)
        .into_iter()
        .filter(|(k, _)| !k.starts_with("NIX_LD") && k != "PATH")
    {
        cmd.arg("-e").arg(format!("{k}={v}"));
    }
    cmd.arg(image).arg(job.program).args(&job.args);
    cmd
}

/// Whether `backend` can start a trivial command here.
pub fn available(backend: SandboxBackend, image: Option<&str>) -> bool {
    let mut probe = match backend {
        SandboxBackend::Bwrap => {
            let mut cmd = Command::new("bwrap");
            cmd.args([
                "--ro-bind",
                "/",
                "/",
                "--unshare-all",
                "--",
                "/bin/sh",
                "-c",
                "true",
            ]);
            cmd
        }
        SandboxBackend::Docker => {
            let Some(image) = image else {
                return false;
            };
            let mut cmd = Command::new("docker");
            cmd.args(["image", "inspect", "--format", "{{.Id}}", image]);
            cmd
        }
    };
    let ok = probe
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    // A missing image is pulled once; a failed pull means the backend is unavailable.
    ok || (backend == SandboxBackend::Docker
        && image.is_some_and(|i| {
            Command::new("docker")
                .args(["pull", "-q", i])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }))
}

fn tail(mut file: File) -> String {
    let len = file.seek(SeekFrom::End(0)).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)));
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    text.trim().to_owned()
}

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("the docker backend needs an image")]
    NoImage,
    #[error("cannot start {backend}: {source}")]
    Spawn {
        backend: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{program} exited with {status}: {output}")]
    Exited {
        program: String,
        status: ExitStatus,
        /// Tail of the combined output.
        output: String,
    },
    #[error("{program} timed out after {timeout:?}")]
    TimedOut { program: String, timeout: Duration },
    #[error("sandbox io: {0}")]
    Io(#[from] io::Error),
}

/// Runs the job to completion or until its timeout.
#[tracing::instrument(level = "debug", skip_all, fields(backend = name(backend), program = %job.program.display()))]
pub fn run(
    backend: SandboxBackend,
    image: Option<&str>,
    job: &Job<'_>,
) -> Result<(), SandboxError> {
    let (cmd, container) = match backend {
        SandboxBackend::Bwrap => (bwrap(job), None),
        SandboxBackend::Docker => {
            let container = container_name();
            let image = image.ok_or(SandboxError::NoImage)?;
            (docker(job, image, &container, None), Some(container))
        }
    };
    execute(cmd, container.as_deref(), backend, job, None)
}

/// Runs repository code in docker without network and within `isolation`; its stdout goes
/// to `stdout` whole (a report), stderr is only kept as the failure tail.
#[tracing::instrument(level = "debug", skip_all, fields(program = %job.program.display()))]
pub fn run_isolated(
    image: &str,
    job: &Job<'_>,
    isolation: &Isolation,
    stdout: &Path,
) -> Result<(), SandboxError> {
    let container = container_name();
    let cmd = docker(job, image, &container, Some(isolation));
    execute(
        cmd,
        Some(&container),
        SandboxBackend::Docker,
        job,
        Some(stdout),
    )
}

fn execute(
    mut cmd: Command,
    container: Option<&str>,
    backend: SandboxBackend,
    job: &Job<'_>,
    stdout: Option<&Path>,
) -> Result<(), SandboxError> {
    let log = tempfile::tempfile()?;
    let out = match stdout {
        Some(path) => File::create(path)?,
        None => log.try_clone()?,
    };
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(log.try_clone()?))
        .spawn()
        .map_err(|source| SandboxError::Spawn {
            backend: name(backend),
            source,
        })?;
    let started = Instant::now();
    let program = job.program.display().to_string();
    match wait(&mut child, job.timeout) {
        Ok(Some(status)) if status.success() => {
            tracing::debug!(elapsed = ?started.elapsed(), "sandbox job finished");
            Ok(())
        }
        Ok(Some(status)) => Err(SandboxError::Exited {
            program,
            status,
            output: tail(log),
        }),
        unfinished => {
            let _ = child.kill();
            let _ = child.wait();
            // Killing the `docker run` client leaves its container running.
            if let Some(container) = &container {
                remove_container(container);
            }

            Err(match unfinished {
                Err(e) => SandboxError::Io(e),
                Ok(_) => SandboxError::TimedOut {
                    program,
                    timeout: job.timeout,
                },
            })
        }
    }
}

/// Polls `child` until it exits or `timeout` passes; `None` means it is still running.
fn wait(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }

        if started.elapsed() >= timeout {
            return Ok(None);
        }

        thread::sleep(POLL);
    }
}

/// `docker rm -f`, waited for at most [`CLEANUP_TIMEOUT`].
fn remove_container(container: &str) {
    let removed = Command::new("docker")
        .args(["rm", "-f", container])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut rm| {
            let status = wait(&mut rm, CLEANUP_TIMEOUT)?;
            if status.is_none() {
                let _ = rm.kill();
                let _ = rm.wait();
            }

            Ok(status)
        });
    if !matches!(removed, Ok(Some(status)) if status.success()) {
        tracing::warn!(
            container,
            "cannot remove the container of a stopped sandbox job"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bwrap_hides_home_and_rebinds_only_listed_mounts() {
        let job = Job {
            program: Path::new("/bin/true"),
            args: Vec::new(),
            cwd: Path::new("/work"),
            mounts: vec![
                Mount {
                    path: "/work".into(),
                    writable: true,
                },
                Mount {
                    path: "/tools".into(),
                    writable: false,
                },
            ],
            env: vec![("CARGO_HOME".into(), "/cache/cargo".into())],
            timeout: Duration::from_secs(1),
        };
        let cmd = bwrap(&job);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(joined.contains("--ro-bind / /"), "{joined}");
        assert!(
            joined.contains("--bind /work /work") && joined.contains("--ro-bind /tools /tools")
        );
        assert!(joined.contains("--clearenv") && joined.contains("--setenv HOME /tmp"));
        assert!(joined.contains("--setenv CARGO_HOME /cache/cargo"));
        let tmpfs = args.iter().position(|a| a == "--tmpfs").expect("tmpfs");
        let bind = args.iter().position(|a| a == "--bind").expect("bind");
        assert!(
            tmpfs < bind,
            "mounts come back after the tmpfs that hides them"
        );
        let container = container_name();
        assert_ne!(container, container_name(), "every job owns its container");
        let docker_args = |isolation: Option<&Isolation>| -> Vec<String> {
            docker(&job, "debian@sha256:00", &container, isolation)
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let dargs = docker_args(None);
        assert!(dargs.join(" ").contains("--network bridge"));
        let isolated = docker_args(Some(&Isolation {
            memory_mb: 512,
            cpu_millis: 1500,
            pids: 64,
        }))
        .join(" ");
        assert!(isolated.contains("--network none"), "{isolated}");
        assert!(isolated.contains("--memory=512m --cpus=1.500 --pids-limit=64"));
        assert!(
            dargs
                .join(" ")
                .contains(&format!("--rm --name {container}"))
        );
        assert!(
            dargs.iter().any(|a| a == "/work:/work:rw")
                && dargs.iter().any(|a| a == "/tools:/tools:ro")
        );
        assert!(dargs.iter().any(|a| a == "ALL") && dargs.iter().any(|a| a == "--read-only"));
    }

    /// Needs a Docker daemon and `node:24-alpine`: `cargo test -p sych-adapters -- --ignored`.
    #[test]
    #[ignore = "needs a Docker daemon"]
    fn a_timed_out_container_is_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let job = Job {
            program: Path::new("/bin/sleep"),
            args: vec!["60".into()],
            cwd: dir.path(),
            mounts: vec![Mount {
                path: dir.path().to_owned(),
                writable: true,
            }],
            env: Vec::new(),
            timeout: Duration::from_secs(3),
        };
        let err = run(SandboxBackend::Docker, Some("node:24-alpine"), &job).expect_err("timeout");
        assert!(matches!(err, SandboxError::TimedOut { .. }), "{err}");
        let out = Command::new("docker")
            .args(["ps", "-aq", "--filter"])
            .arg(format!("name=sych-{}-", process::id()))
            .output()
            .expect("docker ps");
        assert!(
            out.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
