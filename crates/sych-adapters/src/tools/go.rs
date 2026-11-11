//! Pinned Go modules, built with the pinned `go`.

use std::fs;
use std::path::Path;
use std::process::Output;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::process::Command;

use super::{Artifact, PinnedTool, ToolError};

/// `GOOS`/`GOARCH` of a `<arch>-<os>` target.
fn go_platform(target: &str) -> Option<(&'static str, &'static str)> {
    let (arch, os) = target.split_once('-')?;
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let os = match os {
        "linux" => "linux",
        "macos" => "darwin",
        _ => return None,
    };
    Some((os, arch))
}

/// Checks the module's `h1:` hash against the pin, then `go install`s it for `target` into a
/// staging directory renamed into `dir`. Downloads go through the public proxy and checksum
/// database only; no host Go environment, credentials or `GOFLAGS` are used.
pub(super) async fn go_install(
    go: &Path,
    state: &Path,
    tool: &PinnedTool,
    artifact: &Artifact,
    target: &str,
    dir: &Path,
) -> Result<(), ToolError> {
    let module = tool
        .module
        .as_deref()
        .ok_or_else(|| ToolError::Unsupported(format!("{} names no module", tool.name)))?;
    let (goos, goarch) = go_platform(target)
        .ok_or_else(|| ToolError::Unsupported(format!("no Go platform for {target}")))?;
    let parent = dir.parent().expect("install dir has a parent");
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let gopath = staging.path().join("gopath");
    let bin = go.parent().unwrap_or(go);
    let run = |args: &[&str]| {
        let mut command = Command::new(go);
        command
            .args(args)
            .current_dir(staging.path())
            .env_clear()
            .env("PATH", bin)
            .env("HOME", staging.path())
            .env("GOENV", "off")
            .env("GOTOOLCHAIN", "local")
            .env("GOPROXY", "https://proxy.golang.org")
            .env("GOSUMDB", "sum.golang.org")
            .env("GOFLAGS", "-modcacherw")
            .env("GOPATH", &gopath)
            .env("GOMODCACHE", state.join("mod"))
            .env("GOCACHE", state.join("build"))
            .env("CGO_ENABLED", "0")
            .env("GOOS", goos)
            .env("GOARCH", goarch);
        command
    };
    let failed = |what: &str, out: &Output| {
        ToolError::Download(format!(
            "{what} {module}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    };
    let out = run(&["mod", "download", "-json", module]).output().await?;
    if !out.status.success() {
        return Err(failed("go mod download", &out));
    }

    let info: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| ToolError::Download(format!("go mod download {module}: {e}")))?;
    let actual = info["Sum"]
        .as_str()
        .and_then(|s| s.strip_prefix("h1:"))
        .and_then(|h| STANDARD.decode(h).ok())
        .map(|bytes| {
            bytes
                .iter()
                .fold(String::new(), |hex, b| hex + &format!("{b:02x}"))
        })
        .ok_or_else(|| {
            ToolError::Download(format!(
                "go mod download {module}: no h1: hash in its answer"
            ))
        })?;
    if actual != artifact.sha256 {
        return Err(ToolError::Checksum {
            url: module.to_owned(),
            expected: artifact.sha256.clone(),
            actual,
        });
    }

    // `go install` takes a package of the module at the module's version.
    let install = match (&tool.package, module.split_once('@')) {
        (Some(package), Some((_, version))) => format!("{package}@{version}"),
        _ => module.to_owned(),
    };
    let out = run(&["install", "-trimpath", &install]).output().await?;
    if !out.status.success() {
        return Err(failed("go install", &out));
    }

    // A cross-compiled binary lands under `bin/<goos>_<goarch>/`.
    let built = [
        gopath.join("bin").join(&tool.binary),
        gopath
            .join("bin")
            .join(format!("{goos}_{goarch}"))
            .join(&tool.binary),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or_else(|| ToolError::Download(format!("go install {module} built nothing")))?;
    let installed = staging.path().join("out");
    fs::create_dir(&installed)?;
    fs::rename(built, installed.join(&tool.binary))?;
    match fs::rename(&installed, dir) {
        Err(_) if dir.join(&tool.binary).is_file() => Ok(()),
        other => Ok(other?),
    }
}
