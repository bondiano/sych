//! Manifest pins, archive installs and checksum refusal.

use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;
use lzma_rust2::{XzOptions, XzWriter};
use tokio::process::Command;

use super::*;
use crate::ci::archive::tests::zip;

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
        package: None,
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
    for linter in [
        "ruff",
        "oxlint",
        "sqruff",
        "mago",
        "jarl",
        "clj-kondo",
        "actionlint",
        "zizmor",
        "easement",
        "node",
        "scip-go",
        "scip-python",
        "scip-typescript",
    ] {
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
fn zip_install_reads_only_the_binary_entry() {
    let root = tempfile::tempdir().expect("tempdir");
    let bytes = zip(&[
        ("README.md", b"docs", true, 0o100_644),
        ("t", b"#!/bin/sh\n", true, 0o100_755),
    ]);
    let dir = root.path().join("t/1/host");
    install(
        &bytes,
        &tool(ArchiveKind::Zip, "t"),
        &artifact(&bytes),
        &dir,
    )
    .expect("install");
    assert_eq!(fs::read(dir.join("t")).expect("binary"), b"#!/bin/sh\n");
    assert!(
        !dir.join("README.md").exists(),
        "only the binary is written"
    );

    let escaping = zip(&[("../t", b"x", false, 0o100_755)]);
    let other = root.path().join("t/2/host");
    assert!(
        install(
            &escaping,
            &tool(ArchiveKind::Zip, "t"),
            &artifact(&escaping),
            &other
        )
        .is_err()
    );
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

#[test]
fn xz_tarball_install_keeps_member_layout() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(10);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "pkg/bin/t", &b"#!/bin/sh\n"[..])
        .expect("tar");
    let mut xz = XzWriter::new(Vec::new(), XzOptions::with_preset(0)).expect("xz");
    xz.write_all(&tar.into_inner().expect("tar")).expect("xz");
    let bytes = xz.finish().expect("xz");
    let dir = root.path().join("t/1/host");
    install(
        &bytes,
        &tool(ArchiveKind::TarXz, "pkg/bin/t"),
        &artifact(&bytes),
        &dir,
    )
    .expect("install");
    assert_eq!(
        fs::read(dir.join("pkg/bin/t")).expect("binary"),
        b"#!/bin/sh\n"
    );
}

#[test]
fn a_pnpm_lock_tool_is_pinned_by_its_lockfile() {
    let tool = sole_pinned("scip-typescript").expect("pinned");
    assert_eq!(tool.archive, ArchiveKind::PnpmLock);
    let (manifest, lock) = npm::lock(&tool.name).expect("lockfile");
    assert!(manifest.contains(&format!(
        "\"@sourcegraph/scip-typescript\": \"{}\"",
        tool.version
    )));
    for artifact in tool.targets.values() {
        assert_eq!(artifact.sha256, sha256(lock.as_bytes()).hex());
    }
}

/// Installs the easement indexers that are built or installed rather than unpacked:
/// `cargo test -p sych-adapters -- --ignored`.
#[tokio::test]
#[ignore = "needs network"]
async fn pinned_indexers_install_and_report_their_versions() {
    let root = tempfile::tempdir().expect("cache");
    let cache = ToolCache::new(root.path().to_owned());
    let ensure = |name: &'static str| {
        let cache = &cache;
        async move {
            let tool = sole_pinned(name).expect("pinned");
            cache.ensure(tool, &host_target()).await.expect(name)
        }
    };
    let node = ensure("node").await;
    for (indexer, version) in [("scip-typescript", "0.6.1"), ("scip-python", "0.6.8")] {
        let script = ensure(indexer).await;
        let out = Command::new(&node)
            .arg(&script)
            .arg("--version")
            .output()
            .await
            .expect("runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), version);
    }
    let go = ensure("scip-go").await;
    let out = Command::new(&go)
        .arg("--version")
        .output()
        .await
        .expect("runs");
    assert!(String::from_utf8_lossy(&out.stdout).contains("0.2.9"));
}
