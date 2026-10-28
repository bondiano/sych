//! Pinned language-server binaries: Sych downloads exactly the versions and URLs listed in
//! `pinned.json`, verifies their SHA-256 and caches them; it never uses whatever is on `PATH`.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;
use std::time::Duration;
use std::{env, fs};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use reqwest::Client;
use serde::Deserialize;
use sych_core::sha256;
use thiserror::Error;
use tokio::process::Command;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_DOWNLOAD_BYTES: usize = 256 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveKind {
    /// A single gzip-compressed binary.
    Gzip,
    /// A gzip-compressed tarball unpacked whole; `binary` is a path inside it.
    TarGzip,
    /// A Go module built with the pinned `go`: `sha256` is the hex of the module's `h1:` hash,
    /// checked before the build, and the module's `go.sum` pins its dependencies.
    GoModule,
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
    pub url: String,
    pub sha256: String,
    /// Path of the binary in this build's archive when it differs from the tool's, e.g.
    /// `ruff-aarch64-apple-darwin/ruff`.
    #[serde(default)]
    pub binary: Option<String>,
}

impl Artifact {
    pub fn binary<'a>(&'a self, tool: &'a PinnedTool) -> &'a str {
        self.binary.as_deref().unwrap_or(&tool.binary)
    }
}

#[derive(Debug, Deserialize)]
pub struct PinnedTool {
    pub name: String,
    pub version: String,
    pub archive: ArchiveKind,
    pub binary: String,
    /// `path@version` of a [`ArchiveKind::GoModule`].
    #[serde(default)]
    pub module: Option<String>,
    /// Keyed by `<arch>-<os>` as in [`env::consts`].
    pub targets: BTreeMap<String, Artifact>,
}

#[derive(Deserialize)]
struct Manifest {
    tools: Vec<PinnedTool>,
}

#[derive(Debug, Error)]
pub enum ToolError {
    /// No pinned version or platform build; reported as `UNSUPPORTED`, never substituted.
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum {
        url: String,
        expected: String,
        actual: String,
    },
    #[error("tool cache: {0}")]
    Io(#[from] io::Error),
}

fn manifest() -> &'static [PinnedTool] {
    static MANIFEST: OnceLock<Vec<PinnedTool>> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        serde_json::from_str::<Manifest>(include_str!("pinned.json"))
            .expect("pinned.json is valid")
            .tools
    })
}

/// The pinned build of `name` at exactly `version`.
pub fn pinned(name: &str, version: &str) -> Result<&'static PinnedTool, ToolError> {
    manifest()
        .iter()
        .find(|t| t.name == name && t.version == version)
        .ok_or_else(|| {
            let known: Vec<_> = pinned_versions(name).collect();
            ToolError::Unsupported(format!(
                "{name} {version} is not pinned (pinned: {})",
                known.join(", ")
            ))
        })
}

pub fn pinned_versions(name: &str) -> impl Iterator<Item = &'static str> {
    let name = name.to_owned();
    manifest()
        .iter()
        .filter(move |t| t.name == name)
        .map(|t| t.version.as_str())
}

/// The only pinned version of a tool that is not chosen by the reviewed project.
pub fn sole_pinned(name: &str) -> Result<&'static PinnedTool, ToolError> {
    match pinned_versions(name).collect::<Vec<_>>()[..] {
        [version] => pinned(name, version),
        _ => Err(ToolError::Unsupported(format!(
            "{name} needs exactly one pinned version"
        ))),
    }
}

/// `<arch>-<os>` of this host, a key of [`PinnedTool::targets`].
pub fn host_target() -> String {
    format!("{}-{}", env::consts::ARCH, env::consts::OS)
}

/// The Linux build for this CPU, which a container started from this host runs.
pub fn linux_target() -> String {
    format!("{}-linux", env::consts::ARCH)
}

/// Install root: `SYCH_TOOLS_DIR`, else `$XDG_CACHE_HOME/sych/tools`, else `~/.cache/sych/tools`.
pub fn default_cache_root() -> Option<PathBuf> {
    env::var_os("SYCH_TOOLS_DIR")
        .map(PathBuf::from)
        .or_else(|| env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("sych/tools")))
        .or_else(|| env::var_os("HOME").map(|d| PathBuf::from(d).join(".cache/sych/tools")))
}

pub struct ToolCache {
    root: PathBuf,
}

impl ToolCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn install_dir(&self, tool: &PinnedTool, target: &str) -> PathBuf {
        self.root.join(&tool.name).join(&tool.version).join(target)
    }

    /// Path of the cached `target` build; downloads and verifies it first when missing.
    pub async fn ensure(&self, tool: &PinnedTool, target: &str) -> Result<PathBuf, ToolError> {
        let artifact = tool.targets.get(target).ok_or_else(|| {
            ToolError::Unsupported(format!(
                "{} {} has no {target} build",
                tool.name, tool.version
            ))
        })?;
        let dir = self.install_dir(tool, target);
        let binary = dir.join(artifact.binary(tool));
        // The directory appears only by an atomic rename after verification.
        if binary.is_file() {
            tracing::debug!(tool = %tool.name, version = %tool.version, "pinned tool cached");
            return Ok(binary);
        }

        if tool.archive == ArchiveKind::GoModule {
            let go = Box::pin(self.ensure(sole_pinned("go")?, &host_target())).await?;
            tracing::info!(tool = %tool.name, version = %tool.version, "building pinned Go module");
            go_install(&go, &self.root.join("go"), tool, artifact, target, &dir).await?;
            return Ok(binary);
        }

        tracing::info!(tool = %tool.name, version = %tool.version, url = %artifact.url, "downloading pinned tool");
        let bytes = download(&artifact.url).await?;
        install(&bytes, tool, artifact, &dir)?;
        tracing::info!(tool = %tool.name, version = %tool.version, "pinned tool verified and installed");
        Ok(binary)
    }
}

async fn download(url: &str) -> Result<Vec<u8>, ToolError> {
    let failed = |e: reqwest::Error| ToolError::Download(format!("{url}: {e}"));
    // Redirects are followed (release assets live on a CDN); the checksum is the integrity gate.
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .https_only(true)
        .build()
        .map_err(failed)?;
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(failed)?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_DOWNLOAD_BYTES as u64)
    {
        return Err(ToolError::Download(format!("{url}: larger than the limit")));
    }

    let bytes = response.bytes().await.map_err(failed)?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(ToolError::Download(format!("{url}: larger than the limit")));
    }

    Ok(bytes.to_vec())
}

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
async fn go_install(
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
        .unwrap_or_default();
    if actual != artifact.sha256 {
        return Err(ToolError::Checksum {
            url: module.to_owned(),
            expected: artifact.sha256.clone(),
            actual,
        });
    }

    let out = run(&["install", "-trimpath", module]).output().await?;
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

/// Verifies `bytes` against the pinned digest, unpacks them beside `dir` and renames into place.
fn install(
    bytes: &[u8],
    tool: &PinnedTool,
    artifact: &Artifact,
    dir: &Path,
) -> Result<(), ToolError> {
    let actual = sha256(bytes);
    if actual.hex() != artifact.sha256 {
        return Err(ToolError::Checksum {
            url: artifact.url.clone(),
            expected: artifact.sha256.clone(),
            actual: actual.hex().to_owned(),
        });
    }

    let parent = dir.parent().expect("install dir has a parent");
    fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let binary = artifact.binary(tool);
    match tool.archive {
        ArchiveKind::Gzip => {
            let mut contents = Vec::new();
            GzDecoder::new(bytes).read_to_end(&mut contents)?;
            let path = staging.path().join(binary);
            fs::write(&path, contents)?;
            make_executable(&path)?;
        }
        // `unpack` refuses entries escaping the destination.
        ArchiveKind::TarGzip => tar::Archive::new(GzDecoder::new(bytes)).unpack(staging.path())?,
        ArchiveKind::GoModule => {
            return Err(ToolError::Unsupported(format!(
                "{} is built, not unpacked",
                tool.name
            )));
        }
    }
    if !staging.path().join(binary).is_file() {
        return Err(ToolError::Download(format!(
            "{} does not contain {}",
            artifact.url, binary
        )));
    }

    match fs::rename(staging.path(), dir) {
        // A concurrent install of the same verified artifact won.
        Err(_) if dir.join(binary).is_file() => Ok(()),
        other => Ok(other?),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data).expect("gzip");
        encoder.finish().expect("gzip")
    }

    fn tool(archive: ArchiveKind, binary: &str) -> PinnedTool {
        PinnedTool {
            name: "t".into(),
            version: "1".into(),
            archive,
            binary: binary.into(),
            module: None,
            targets: BTreeMap::new(),
        }
    }

    fn artifact(bytes: &[u8]) -> Artifact {
        Artifact {
            url: "https://example.invalid/t".into(),
            sha256: sha256(bytes).hex().to_owned(),
            binary: None,
        }
    }

    #[test]
    fn manifest_pins_every_tool_for_supported_hosts() {
        for tool in manifest() {
            for target in [
                "aarch64-linux",
                "x86_64-linux",
                "aarch64-macos",
                "x86_64-macos",
            ] {
                let artifact = &tool.targets[target];
                assert!(artifact.url.starts_with("https://"), "{}", tool.name);
                assert_eq!(artifact.sha256.len(), 64, "{}", tool.name);
            }
        }
        assert!(sole_pinned("rust-analyzer").is_ok());
        assert!(sole_pinned("ty").is_ok());
        assert!(sole_pinned("go").is_ok());
        for linter in ["ruff", "oxlint", "actionlint", "zizmor"] {
            assert!(sole_pinned(linter).is_ok(), "{linter}");
        }
        assert!(sole_pinned("gopls").is_ok_and(|t| t.module.is_some()));
        assert!(pinned("typescript", "7.0.2").is_ok());
        assert!(matches!(
            pinned("typescript", "5.9.3"),
            Err(ToolError::Unsupported(_))
        ));
    }

    #[test]
    fn gzip_install_writes_executable_binary() {
        let root = tempfile::tempdir().expect("tempdir");
        let bytes = gzip(b"#!/bin/sh\n");
        let dir = root.path().join("t/1/host");
        install(
            &bytes,
            &tool(ArchiveKind::Gzip, "t"),
            &artifact(&bytes),
            &dir,
        )
        .expect("install");
        assert_eq!(fs::read(dir.join("t")).expect("binary"), b"#!/bin/sh\n");
    }

    #[test]
    fn tarball_install_keeps_member_layout() {
        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "package/lib/tsc", &b"ok"[..])
            .expect("append");
        let bytes = gzip(&tar.into_inner().expect("tar"));
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().join("t/1/host");
        let tool = tool(ArchiveKind::TarGzip, "package/lib/tsc");
        install(&bytes, &tool, &artifact(&bytes), &dir).expect("install");
        assert!(dir.join("package/lib/tsc").is_file());
    }

    /// Downloads the pinned Go and builds gopls: `cargo test -p sych-adapters -- --ignored`.
    #[tokio::test]
    #[ignore = "needs network"]
    async fn pinned_gopls_is_built_with_the_pinned_go() {
        let root = tempfile::tempdir().expect("cache");
        let cache = ToolCache::new(root.path().to_owned());
        let gopls = sole_pinned("gopls").expect("pinned");
        let binary = cache.ensure(gopls, &host_target()).await.expect("gopls");
        let out = Command::new(&binary)
            .arg("version")
            .output()
            .await
            .expect("runs");
        assert!(String::from_utf8_lossy(&out.stdout).contains("v0.24.0"));
    }

    #[test]
    fn checksum_mismatch_installs_nothing() {
        let root = tempfile::tempdir().expect("tempdir");
        let bytes = gzip(b"tampered");
        let dir = root.path().join("t/1/host");
        let err = install(
            &bytes,
            &tool(ArchiveKind::Gzip, "t"),
            &artifact(b"original"),
            &dir,
        )
        .expect_err("mismatch");
        assert!(matches!(err, ToolError::Checksum { .. }));
        assert!(!dir.exists());
    }
}
