//! Pinned language-server binaries: Sych downloads exactly the versions and URLs listed in
//! `pinned.json` and `indexers.json`, verifies their SHA-256 and caches them; it never uses
//! whatever is on `PATH`.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use std::{env, fs};

use flate2::read::GzDecoder;
use lzma_rust2::XzReader;
use reqwest::Client;
use serde::Deserialize;
use sych_core::sha256;
use thiserror::Error;

use crate::ci::archive::{Limits, read_entry};

mod go;
mod npm;

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_DOWNLOAD_BYTES: usize = 256 << 20;
/// Largest binary inflated out of a zip; native-image builds are around 70 MiB.
const MAX_BINARY_BYTES: u64 = 256 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveKind {
    /// A single gzip-compressed binary.
    Gzip,
    /// A gzip-compressed tarball unpacked whole; `binary` is a path inside it.
    TarGzip,
    /// An xz-compressed tarball unpacked whole, like [`Self::TarGzip`].
    TarXz,
    /// A zip of which only the `binary` entry is read, through the CI artifact reader.
    Zip,
    /// A Go module built with the pinned `go`: `sha256` is the hex of the module's `h1:` hash,
    /// checked before the build, and the module's `go.sum` pins its dependencies.
    GoModule,
    /// npm packages a Sych-owned lockfile pins (`npm/<tool>/pnpm-lock.yaml`), installed by the
    /// pinned pnpm with scripts off: `sha256` is the lockfile's, and the lockfile holds the
    /// integrity of every package.
    PnpmLock,
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
    /// The package of that module to build, when its `main` is not at the module root.
    #[serde(default)]
    pub package: Option<String>,
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
        // Language servers and linters, then easement and the indexers it runs.
        [include_str!("pinned.json"), include_str!("indexers.json")]
            .into_iter()
            .flat_map(|text| {
                serde_json::from_str::<Manifest>(text)
                    .expect("the pinned tool manifests are valid")
                    .tools
            })
            .collect()
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
            go::go_install(&go, &self.root.join("go"), tool, artifact, target, &dir).await?;
            return Ok(binary);
        }

        if tool.archive == ArchiveKind::PnpmLock {
            let pnpm = Box::pin(self.ensure(pinned("pnpm", npm::PNPM)?, &host_target())).await?;
            tracing::info!(tool = %tool.name, version = %tool.version, "installing pinned npm packages");
            npm::pnpm_install(&pnpm, &self.root.join("pnpm-store"), tool, artifact, &dir).await?;
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
        ArchiveKind::TarXz => {
            tar::Archive::new(XzReader::new(bytes, false)).unpack(staging.path())?;
        }
        ArchiveKind::Zip => {
            let limits = Limits {
                max_entries: 1_000,
                max_entry_bytes: MAX_BINARY_BYTES,
            };
            let contents = read_entry(bytes, binary, limits)
                .map_err(|e| ToolError::Download(format!("{}: {e}", artifact.url)))?
                .ok_or_else(|| {
                    ToolError::Download(format!("{} does not contain {binary}", artifact.url))
                })?;
            let path = staging.path().join(binary);
            fs::write(&path, contents)?;
            make_executable(&path)?;
        }
        ArchiveKind::GoModule | ArchiveKind::PnpmLock => {
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
mod tests;
