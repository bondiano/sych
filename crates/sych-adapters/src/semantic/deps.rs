//! Dependencies of a materialized side for semantic analysis, installed only in
//! the configured sandbox and never by running repository code: `pnpm install` with scripts
//! and pnpmfile off, `cargo fetch` into a Sych-owned `CARGO_HOME` (no host credentials).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{env, fs, io};

use serde_json::Value;
use sych_core::Language;
use sych_core::config::{OnUnavailable, SandboxBackend, Tooling};
use thiserror::Error;

use super::Resolve;
use crate::sandbox::{self, Job, Mount, SandboxError};
use crate::tools::{self, ToolError};

const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Error)]
pub enum DepsError {
    /// The repository's setup is not one Sych installs, e.g. a yarn lockfile.
    #[error("{0}")]
    Unsupported(String),
    #[error("no sandbox backend can run it: {0}")]
    NoBackend(String),
    #[error(transparent)]
    Tool(#[from] ToolError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error("dependency cache: {0}")]
    Io(#[from] io::Error),
}

/// What the install left behind for the language server.
#[derive(Debug, Default)]
pub struct Prepared {
    /// `installed (<backend>)` or `not installed`.
    pub state: String,
    pub limitation: Option<String>,
    /// Extra environment for the server, e.g. the `CARGO_HOME` that holds the fetched crates.
    pub env: Vec<(String, String)>,
}

fn not_installed(limitation: Option<String>) -> Prepared {
    Prepared {
        state: "not installed".into(),
        limitation,
        env: Vec::new(),
    }
}

/// `Err` only when the policy is `fail`; `degrade` turns every problem into a limitation.
pub fn prepare(
    language: Language,
    root: &Path,
    tooling: &Tooling,
    resolve: &Resolve<'_>,
    state_dir: &Path,
) -> Result<Prepared, DepsError> {
    if !tooling.dependency_install {
        return Ok(not_installed(None));
    }

    let attempt = match language {
        Language::Typescript => pnpm(root, tooling, resolve, state_dir),
        Language::Rust => cargo(root, tooling, state_dir),
        Language::Go => go(root, tooling, resolve, state_dir),
        // A pip or uv install builds sdists through the project's build backend, which runs
        // repository code; not installing is the policy, not an unavailable tool.
        Language::Python => return Ok(not_installed(None)),
    };
    match (attempt, tooling.sandbox.on_unavailable) {
        (Ok(prepared), _) => Ok(prepared),
        (Err(reason), OnUnavailable::Fail) => Err(reason),
        (Err(reason), OnUnavailable::Degrade) => {
            tracing::warn!(%reason, "dependencies not installed; continuing without them");
            Ok(not_installed(Some(format!(
                "dependencies not installed: {reason}"
            ))))
        }
    }
}

/// The first configured backend that is available and can run the job.
fn pick_backend(tooling: &Tooling, docker_ok: bool) -> Result<SandboxBackend, DepsError> {
    let image = tooling.sandbox.docker_image.as_deref();
    let mut tried = Vec::new();
    for &backend in &tooling.sandbox.backends {
        if backend == SandboxBackend::Docker && !docker_ok {
            tried.push("docker (cannot run the host toolchain)".to_owned());
            continue;
        }

        if !sandbox::available(backend, image) {
            tried.push(format!("{} (unavailable)", sandbox::name(backend)));
            continue;
        }

        return Ok(backend);
    }
    Err(DepsError::NoBackend(tried.join(", ")))
}

fn install(tooling: &Tooling, backend: SandboxBackend, job: &Job<'_>) -> Result<(), DepsError> {
    tracing::info!(backend = sandbox::name(backend), program = %job.program.display(), "installing dependencies in the sandbox");
    Ok(sandbox::run(
        backend,
        tooling.sandbox.docker_image.as_deref(),
        job,
    )?)
}

/// The pinned build a backend executes: a container runs Linux whatever the host is.
fn target_for(backend: SandboxBackend) -> String {
    match backend {
        SandboxBackend::Bwrap => tools::host_target(),
        SandboxBackend::Docker => tools::linux_target(),
    }
}

/// Read-only mounts that bring `program` back from under the hidden home: its directory
/// (rustup proxies live in `~/.cargo/bin`) and its symlink target's, never `~/.cargo` itself,
/// which holds credentials and config.
fn program_mounts(program: &Path) -> Vec<Mount> {
    let mut dirs: Vec<PathBuf> = [Some(program.to_path_buf()), fs::canonicalize(program).ok()]
        .into_iter()
        .flatten()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    dirs.dedup();
    dirs.into_iter()
        .map(|path| Mount {
            path,
            writable: false,
        })
        .collect()
}

/// `packageManager: "pnpm@12.14.0+sha512…"` of the root `package.json`.
fn pnpm_version(root: &Path) -> Option<String> {
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(root.join("package.json")).ok()?).ok()?;
    let spec = manifest["packageManager"].as_str()?.strip_prefix("pnpm@")?;
    Some(spec.split('+').next().unwrap_or(spec).to_owned())
}

fn pnpm(
    root: &Path,
    tooling: &Tooling,
    resolve: &Resolve<'_>,
    state_dir: &Path,
) -> Result<Prepared, DepsError> {
    if !root.join("pnpm-lock.yaml").is_file() {
        return Err(DepsError::Unsupported(
            "only pnpm lockfiles are installed so far".into(),
        ));
    }

    let version = pnpm_version(root).ok_or_else(|| {
        DepsError::Unsupported("package.json does not pin pnpm in packageManager".into())
    })?;
    let tool = tools::pinned("pnpm", &version)?;
    let backend = pick_backend(tooling, true)?;
    let binary = resolve(tool, &target_for(backend))?;
    let store = state_dir.join("pnpm-store");
    fs::create_dir_all(&store)?;
    let tools_dir = binary.parent().unwrap_or(&binary).to_path_buf();
    let job = Job {
        program: &binary,
        args: [
            "install",
            "--frozen-lockfile",
            "--ignore-scripts",
            "--ignore-pnpmfile",
            "--config.registry=https://registry.npmjs.org/",
            "--config.manage-package-manager-versions=false",
            "--config.confirm-modules-purge=false",
            "--reporter=append-only",
        ]
        .map(str::to_owned)
        .into_iter()
        .chain([format!("--config.store-dir={}", store.display())])
        .collect(),
        cwd: root,
        mounts: vec![
            Mount {
                path: root.to_path_buf(),
                writable: true,
            },
            Mount {
                path: store,
                writable: true,
            },
            Mount {
                path: tools_dir,
                writable: false,
            },
        ],
        env: vec![("npm_config_userconfig".into(), "/dev/null".into())],
        timeout: INSTALL_TIMEOUT,
    };
    install(tooling, backend, &job)?;
    Ok(Prepared {
        state: format!(
            "installed ({}, pnpm {version}, scripts off)",
            sandbox::name(backend)
        ),
        limitation: None,
        env: Vec::new(),
    })
}

/// Environment of the pinned `go` for gopls: the module cache `go mod download` fills, no
/// proxy (gopls never downloads), no host Go configuration.
pub fn go_env(resolve: &Resolve<'_>, state_dir: &Path) -> Result<Vec<(String, String)>, ToolError> {
    let go = resolve(tools::sole_pinned("go")?, &tools::host_target())?;
    Ok(go_base_env(&go, state_dir)
        .into_iter()
        .chain([("GOPROXY".to_owned(), "off".to_owned())])
        .collect())
}

fn go_base_env(go: &Path, state_dir: &Path) -> Vec<(String, String)> {
    let dir = |name: &str| state_dir.join(name).display().to_string();
    let bin = go.parent().unwrap_or(go).display().to_string();
    [
        ("PATH", bin),
        ("GOENV", "off".into()),
        ("GOTOOLCHAIN", "local".into()),
        ("GOFLAGS", "-modcacherw".into()),
        ("CGO_ENABLED", "0".into()),
        ("GOPATH", dir("go-path")),
        ("GOMODCACHE", dir("go-mod")),
        ("GOCACHE", dir("go-build")),
    ]
    .map(|(k, v)| (k.to_owned(), v))
    .into()
}

/// `go mod download` with the pinned `go` through the public proxy and checksum database:
/// it fetches and verifies modules, it runs no repository code.
fn go(
    root: &Path,
    tooling: &Tooling,
    resolve: &Resolve<'_>,
    state_dir: &Path,
) -> Result<Prepared, DepsError> {
    let backend = pick_backend(tooling, false)?;
    let go = resolve(tools::sole_pinned("go")?, &target_for(backend))?;
    let mut env = go_base_env(&go, state_dir);
    env.extend([
        ("GOPROXY".to_owned(), "https://proxy.golang.org".to_owned()),
        ("GOSUMDB".to_owned(), "sum.golang.org".to_owned()),
    ]);
    let mut mounts = program_mounts(&go);
    // The toolchain's own tree (`GOROOT`) sits two levels above `bin/go`.
    if let Some(goroot) = go.parent().and_then(Path::parent) {
        mounts.push(Mount {
            path: goroot.to_path_buf(),
            writable: false,
        });
    }

    for name in ["go-path", "go-mod", "go-build"] {
        let path = state_dir.join(name);
        fs::create_dir_all(&path)?;
        mounts.push(Mount {
            path,
            writable: true,
        });
    }
    mounts.push(Mount {
        path: root.to_path_buf(),
        writable: true,
    });
    let modules: Vec<PathBuf> = if root.join("go.work").is_file() {
        vec![root.to_path_buf()]
    } else {
        super::gomod::modules(root)
            .iter()
            .filter_map(|m| m.parent().map(Path::to_path_buf))
            .collect()
    };
    for dir in &modules {
        let job = Job {
            program: &go,
            args: vec!["mod".into(), "download".into()],
            cwd: dir,
            mounts: mounts
                .iter()
                .map(|m| Mount {
                    path: m.path.clone(),
                    writable: m.writable,
                })
                .collect(),
            env: env.clone(),
            timeout: INSTALL_TIMEOUT,
        };
        install(tooling, backend, &job)?;
    }
    Ok(Prepared {
        state: format!("downloaded ({}, go mod download)", sandbox::name(backend)),
        limitation: None,
        env: Vec::new(),
    })
}

fn home_path(rest: &str) -> Option<PathBuf> {
    env::var_os("HOME").map(|h| PathBuf::from(h).join(rest))
}

/// Read-only mounts and environment that let `cargo_bin` (often a rustup proxy) run in bwrap.
fn toolchain(cargo_bin: &Path) -> (Vec<Mount>, Vec<(String, String)>) {
    let mut mounts = program_mounts(cargo_bin);
    let mut env = Vec::new();
    let rustup = env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| home_path(".rustup"));
    if let Some(rustup) = rustup.filter(|p| p.is_dir()) {
        env.push(("RUSTUP_HOME".into(), rustup.display().to_string()));
        mounts.push(Mount {
            path: rustup,
            writable: false,
        });
    }

    (mounts, env)
}

/// `cargo fetch` of the workspace and of the toolchain's standard library, so rust-analyzer
/// resolves both offline.
fn cargo(root: &Path, tooling: &Tooling, state_dir: &Path) -> Result<Prepared, DepsError> {
    super::profiles::toolchain(root)?;
    let cargo_home = state_dir.join("cargo-home");
    fs::create_dir_all(&cargo_home)?;
    let cargo_bin =
        which("cargo").ok_or_else(|| DepsError::Unsupported("cargo is not on PATH".into()))?;
    let (mut mounts, mut env) = toolchain(&cargo_bin);
    mounts.extend([
        Mount {
            path: root.to_path_buf(),
            writable: true,
        },
        Mount {
            path: cargo_home.clone(),
            writable: true,
        },
    ]);
    env.push(("CARGO_HOME".to_owned(), cargo_home.display().to_string()));

    let mut manifests = vec![root.join("Cargo.toml")];
    if let Some(std) = sysroot(root)
        .map(|s| s.join("lib/rustlib/src/rust/library/Cargo.toml"))
        .filter(|p| p.is_file())
    {
        manifests.push(std);
    }

    let backend = pick_backend(tooling, false)?;
    for (i, manifest) in manifests.iter().enumerate() {
        let mut job_env = env.clone();
        // The toolchain's own std manifest uses unstable features, as rust-analyzer allows.
        if i > 0 {
            job_env.push(("RUSTC_BOOTSTRAP".into(), "1".into()));
        }

        let job = Job {
            program: &cargo_bin,
            args: vec![
                "fetch".into(),
                "--manifest-path".into(),
                manifest.display().to_string(),
            ],
            cwd: root,
            mounts: mounts
                .iter()
                .map(|m| Mount {
                    path: m.path.clone(),
                    writable: m.writable,
                })
                .collect(),
            env: job_env,
            timeout: INSTALL_TIMEOUT,
        };
        install(tooling, backend, &job)?;
    }
    Ok(Prepared {
        state: format!("fetched ({}, sych CARGO_HOME)", sandbox::name(backend)),
        limitation: None,
        env: env.into_iter().filter(|(k, _)| k == "CARGO_HOME").collect(),
    })
}

/// `rustc --print sysroot` for the toolchain the workspace selects; it reads only the
/// toolchain file, never repository code.
fn sysroot(root: &Path) -> Option<PathBuf> {
    let out = Command::new("rustc")
        .arg("--print")
        .arg("sysroot")
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn which(program: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|d| d.join(program))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn package_manager_pin_drops_the_integrity_suffix() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(
            root.path().join("package.json"),
            r#"{"packageManager": "pnpm@12.14.0+sha512.abc"}"#,
        )
        .expect("write");
        assert_eq!(pnpm_version(root.path()).as_deref(), Some("12.14.0"));
        fs::write(
            root.path().join("package.json"),
            r#"{"packageManager": "yarn@4.0.0"}"#,
        )
        .expect("write");
        assert_eq!(pnpm_version(root.path()), None);
    }

    #[test]
    fn docker_gets_the_linux_build_of_pnpm() {
        let linux = target_for(SandboxBackend::Docker);
        assert_eq!(linux, format!("{}-linux", env::consts::ARCH));
        assert_eq!(target_for(SandboxBackend::Bwrap), tools::host_target());
        let pnpm = tools::pinned("pnpm", "12.14.0").expect("pinned");
        assert!(pnpm.targets[&linux].url.contains("exe.linux-"));
    }

    /// Downloads the pinned pnpm and needs `debian:bookworm-slim`:
    /// `cargo test -p sych-adapters -- --ignored`.
    #[tokio::test]
    #[ignore = "needs network and a Docker daemon"]
    async fn the_docker_build_of_pnpm_runs_in_a_linux_container() {
        let cache = tempfile::tempdir().expect("cache");
        let pnpm = tools::pinned("pnpm", "12.14.0").expect("pinned");
        let binary = tools::ToolCache::new(cache.path().to_owned())
            .ensure(pnpm, &target_for(SandboxBackend::Docker))
            .await
            .expect("download");
        let job = Job {
            program: &binary,
            args: vec!["--version".into()],
            cwd: cache.path(),
            mounts: vec![Mount {
                path: cache.path().to_owned(),
                writable: false,
            }],
            env: Vec::new(),
            timeout: Duration::from_secs(60),
        };
        sandbox::run(SandboxBackend::Docker, Some("debian:bookworm-slim"), &job)
            .expect("pnpm --version in the container");
    }

    #[test]
    fn bwrap_runs_cargo_from_a_hidden_home_without_its_credentials() {
        let Some(cargo) = which("cargo") else { return };
        if !sandbox::available(SandboxBackend::Bwrap, None) {
            return;
        }

        // Under `/tmp`, which the sandbox hides like the home directory.
        let home = tempfile::tempdir_in("/tmp").expect("home");
        let bin = home.path().join(".cargo/bin");
        fs::create_dir_all(&bin).expect("bin");
        symlink(&cargo, bin.join("cargo")).expect("proxy");
        let credentials = home.path().join(".cargo/credentials.toml");
        fs::write(&credentials, "token = \"secret\"").expect("credentials");
        let proxy = bin.join("cargo");
        // This repository, whose toolchain file picks the toolchain as a reviewed one would.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let run = |program: &Path, args: Vec<String>| {
            let (mut mounts, env) = toolchain(&proxy);
            mounts.push(Mount {
                path: workspace.clone(),
                writable: false,
            });
            let job = Job {
                program,
                args,
                cwd: &workspace,
                mounts,
                env,
                timeout: Duration::from_secs(60),
            };
            sandbox::run(SandboxBackend::Bwrap, None, &job)
        };
        run(&proxy, vec!["--version".into()]).expect("cargo runs");
        let check = format!("test ! -e {}", credentials.display());
        run(Path::new("/bin/sh"), vec!["-c".into(), check]).expect("credentials stay hidden");
    }

    #[test]
    fn install_off_or_degraded_never_fails_the_review() {
        let root = tempfile::tempdir().expect("tempdir");
        let resolve = |_: &'static tools::PinnedTool,
                       _: &str|
         -> Result<PathBuf, tools::ToolError> { unreachable!("not called") };
        let off = prepare(
            Language::Typescript,
            root.path(),
            &Tooling::default(),
            &resolve,
            root.path(),
        )
        .expect("off");
        assert_eq!(off.state, "not installed");
        let mut tooling = Tooling {
            dependency_install: true,
            ..Tooling::default()
        };
        tooling.sandbox.on_unavailable = OnUnavailable::Degrade;
        let degraded = prepare(
            Language::Typescript,
            root.path(),
            &tooling,
            &resolve,
            root.path(),
        )
        .expect("degrade");
        assert!(
            degraded
                .limitation
                .is_some_and(|l| l.contains("pnpm lockfiles"))
        );
        tooling.sandbox.on_unavailable = OnUnavailable::Fail;
        assert!(
            prepare(
                Language::Typescript,
                root.path(),
                &tooling,
                &resolve,
                root.path()
            )
            .is_err()
        );
    }
}
