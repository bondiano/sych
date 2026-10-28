//! How each language server is chosen and launched for a materialized side.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use sych_core::Language;

use super::{cargo, gomod, tsproject};
use crate::tools::{self, PinnedTool, ToolError};

pub struct Profile {
    pub tool: &'static PinnedTool,
    pub args: &'static [&'static str],
    pub init_options: Value,
    /// Wait for rust-analyzer's `experimental/serverStatus` quiescence before querying.
    pub wait_quiescent: bool,
    pub build_configuration: BTreeMap<String, String>,
    pub limitations: Vec<String>,
    /// Environment for the server beyond the passed-through toolchain variables.
    pub env: Vec<(String, String)>,
}

/// Prefix of the limitation a successful install removes.
const NOT_INSTALLED: &str = "dependencies are not installed";

impl Profile {
    /// Records the dependency install outcome.
    pub fn apply(&mut self, prepared: &super::deps::Prepared) {
        self.build_configuration
            .insert("dependencies".into(), prepared.state.clone());
        if prepared.state != "not installed" {
            self.limitations.retain(|l| !l.starts_with(NOT_INSTALLED));
        }

        self.limitations.extend(prepared.limitation.clone());
        self.env.extend(prepared.env.iter().cloned());
    }
}

/// `Err` is a reason for `UNSUPPORTED`; nothing else is substituted.
pub fn profile(language: Language, root: &Path) -> Result<Profile, ToolError> {
    match language {
        Language::Rust => rust(root),
        Language::Typescript => typescript(root),
        Language::Go => go(root),
        Language::Python => python(root),
    }
}

fn python(root: &Path) -> Result<Profile, ToolError> {
    let config: Vec<&str> = ["pyproject.toml", "ty.toml"]
        .into_iter()
        .filter(|c| root.join(c).is_file())
        .collect();
    let build_configuration = BTreeMap::from([
        ("dependencies".to_owned(), "not installed".to_owned()),
        (
            "config".to_owned(),
            if config.is_empty() {
                "ty defaults".to_owned()
            } else {
                config.join(", ")
            },
        ),
    ]);
    Ok(Profile {
        tool: tools::sole_pinned("ty")?,
        args: &["server"],
        // An untrusted workspace starts no external command (no `uv` workspace discovery);
        // ty offers pull diagnostics to a client without dynamic registration.
        init_options: json!({"untrustedWorkspace": true}),
        wait_quiescent: false,
        build_configuration,
        limitations: vec![format!(
            "{NOT_INSTALLED}: third-party imports stay unresolved, so their types, callers and \
             implementations are missing"
        )],
        env: Vec::new(),
    })
}

fn go(root: &Path) -> Result<Profile, ToolError> {
    if !root.join("go.work").is_file() && gomod::modules(root).is_empty() {
        return Err(ToolError::Unsupported(
            "no go.mod or go.work in the snapshot".into(),
        ));
    }

    let (mut build_configuration, mut limitations) = gomod::configuration(root);
    build_configuration.insert("dependencies".into(), "not installed".into());
    limitations.insert(
        0,
        format!(
            "{NOT_INSTALLED}: packages of modules outside the snapshot stay unresolved and \
             their callers and implementations are missing"
        ),
    );
    Ok(Profile {
        tool: tools::sole_pinned("gopls")?,
        args: &[],
        // gopls offers `textDocument/diagnostic` only when asked: compiler errors, vet and its
        // default staticcheck subset come back per changed file.
        init_options: json!({"pullDiagnostics": true}),
        wait_quiescent: false,
        build_configuration,
        limitations,
        env: Vec::new(),
    })
}

fn rust(root: &Path) -> Result<Profile, ToolError> {
    if !root.join("Cargo.toml").is_file() {
        return Err(ToolError::Unsupported(
            "no Cargo.toml at the repository root".into(),
        ));
    }

    let toolchain = toolchain(root)?;
    let lock = if root.join("Cargo.lock").is_file() {
        "Cargo.lock"
    } else {
        "absent"
    };
    let (mut build_configuration, mut limitations) = cargo::configuration(&cargo::manifests(root));
    build_configuration.extend(
        [
            ("toolchain", toolchain.as_str()),
            ("lockfile", lock),
            ("enabled_features", "default"),
            ("cfg", "host target, test, debug_assertions"),
            ("build_scripts", "disabled"),
            ("proc_macros", "disabled"),
            ("cargo_network", "offline"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    );
    Ok(Profile {
        tool: tools::sole_pinned("rust-analyzer")?,
        args: &[],
        // Nothing from the reviewed repository is executed: no build scripts, proc-macros,
        // `cargo check` or network fetches.
        init_options: json!({
            "cargo": {
                "buildScripts": {"enable": false},
                "extraEnv": {"CARGO_NET_OFFLINE": "true"}
            },
            "procMacro": {"enable": false},
            "checkOnSave": false,
            "cachePriming": {"enable": false}
        }),
        wait_quiescent: true,
        build_configuration,
        env: Vec::new(),
        limitations: {
            limitations.extend([
                "build scripts and proc-macros are not run: generated and macro-expanded items \
                 are missing, so absent references are not proof of absence"
                    .to_owned(),
                "only default features and the host cfg are analysed: code behind other \
                 features, targets or cfgs is not checked"
                    .to_owned(),
                "diagnostics are rust-analyzer native checks without proc-macros or cargo \
                 check; they are not CI facts and errors may be analysis artefacts"
                    .to_owned(),
            ]);
            limitations
        },
    })
}

/// The toolchain channel the workspace selects. A toolchain `path` would make rustup run a
/// binary from the repository, so it is refused.
pub fn toolchain(root: &Path) -> Result<String, ToolError> {
    let refused = || {
        ToolError::Unsupported(
            "rust-toolchain names a toolchain path; it would run repository binaries".into(),
        )
    };
    if let Ok(text) = fs::read_to_string(root.join("rust-toolchain.toml")) {
        let table: toml::Table = text
            .parse()
            .map_err(|_| ToolError::Unsupported("rust-toolchain.toml does not parse".into()))?;
        let toolchain = table.get("toolchain").and_then(toml::Value::as_table);
        if toolchain.is_some_and(|t| t.contains_key("path")) {
            return Err(refused());
        }

        return Ok(toolchain
            .and_then(|t| t.get("channel"))
            .and_then(toml::Value::as_str)
            .unwrap_or("default")
            .to_owned());
    }

    match fs::read_to_string(root.join("rust-toolchain")) {
        Ok(text) if text.contains("path") || text.contains('/') => Err(refused()),
        Ok(text) => Ok(text
            .lines()
            .next()
            .unwrap_or("default")
            .trim()
            .trim_matches('"')
            .to_owned()),
        Err(_) => Ok("default".into()),
    }
}

fn typescript(root: &Path) -> Result<Profile, ToolError> {
    let (version, lockfile) = typescript_version(root)?;
    let tool =
        tools::pinned("typescript", &version).map_err(|e| match version.split('.').next() {
            Some(major) if major.parse::<u32>().is_ok_and(|m| m < 7) => {
                ToolError::Unsupported(format!(
                    "typescript {version} predates the native LSP (7.x); tsserver is not supported"
                ))
            }
            _ => e,
        })?;
    let (mut build_configuration, project_limitations) = tsproject::configuration(root);
    build_configuration.extend(
        [
            ("typescript", version.as_str()),
            ("lockfile", lockfile),
            ("dependencies", "not installed"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned())),
    );
    Ok(Profile {
        tool,
        args: &["--lsp", "--stdio"],
        init_options: Value::Null,
        wait_quiescent: false,
        build_configuration,
        env: Vec::new(),
        limitations: [format!(
            "{NOT_INSTALLED}: imports of npm and workspace packages stay unresolved and their callers are missing"
        )]
        .into_iter()
        .chain(project_limitations)
        .collect(),
    })
}

/// The single TypeScript version the project's lockfile pins for its own packages.
fn typescript_version(root: &Path) -> Result<(String, &'static str), ToolError> {
    let versions = if let Ok(text) = fs::read_to_string(root.join("pnpm-lock.yaml")) {
        (pnpm_typescript(&text), "pnpm-lock.yaml")
    } else if let Ok(text) = fs::read_to_string(root.join("package-lock.json")) {
        (npm_typescript(&text), "package-lock.json")
    } else {
        return Err(ToolError::Unsupported(
            "no pnpm-lock.yaml or package-lock.json at the repository root".into(),
        ));
    };
    match versions {
        (set, lockfile) if set.len() == 1 => Ok((set.into_iter().next().expect("one"), lockfile)),
        (set, lockfile) if set.is_empty() => Err(ToolError::Unsupported(format!(
            "{lockfile} does not pin typescript"
        ))),
        (set, lockfile) => Err(ToolError::Unsupported(format!(
            "{lockfile} pins several typescript versions: {set:?}"
        ))),
    }
}

/// Direct `typescript` dependency versions of every pnpm importer, peer suffixes dropped.
/// pnpm 11+ writes a multi-document lockfile (package-manager pin first), so all documents
/// are read.
fn pnpm_typescript(text: &str) -> BTreeSet<String> {
    let documents: Vec<serde_yaml_ng::Value> = serde_yaml_ng::Deserializer::from_str(text)
        .map_while(|doc| serde_yaml_ng::Value::deserialize(doc).ok())
        .collect();
    documents
        .iter()
        .flat_map(|lock| match lock.get("importers") {
            Some(serde_yaml_ng::Value::Mapping(map)) => map.values().collect(),
            _ => vec![lock],
        })
        .flat_map(|importer| {
            ["dependencies", "devDependencies", "optionalDependencies"]
                .into_iter()
                .filter_map(|section| importer.get(section)?.get("typescript"))
        })
        .filter_map(|dep| dep.get("version").unwrap_or(dep).as_str())
        .map(|v| v.split('(').next().unwrap_or(v).to_owned())
        .collect()
}

fn npm_typescript(text: &str) -> BTreeSet<String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|lock| {
            lock["packages"]["node_modules/typescript"]["version"]
                .as_str()
                .map(str::to_owned)
        })
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_reads_direct_importer_versions_not_transitive_packages() {
        let lock = "---\nlockfileVersion: '9.0'\nimporters:\n  .:\n    configDependencies: {}\n---\n\
            lockfileVersion: '9.0'\n\
            importers:\n  .:\n    devDependencies:\n      typescript:\n        specifier: 'catalog:'\n        version: 7.0.2\n\
            \x20 apps/pwa:\n    devDependencies:\n      typescript:\n        specifier: 'catalog:'\n        version: 7.0.2(peer@1.0.0)\n\
            packages:\n  typescript@6.0.3:\n    resolution: {integrity: x}\n";
        assert_eq!(pnpm_typescript(lock), BTreeSet::from(["7.0.2".to_owned()]));
    }

    #[test]
    fn npm_reads_installed_typescript() {
        let lock = r#"{"packages": {"node_modules/typescript": {"version": "5.9.3"}}}"#;
        assert_eq!(npm_typescript(lock), BTreeSet::from(["5.9.3".to_owned()]));
    }

    #[test]
    fn python_runs_ty_untrusted_and_names_its_config() {
        let root = tempfile::tempdir().expect("tempdir");
        let p = python(root.path()).expect("profile");
        assert_eq!((p.tool.name.as_str(), p.args), ("ty", &["server"][..]));
        assert_eq!(p.init_options, json!({"untrustedWorkspace": true}));
        assert_eq!(p.build_configuration["config"], "ty defaults");
        assert!(p.limitations[0].starts_with(NOT_INSTALLED));
        fs::write(root.path().join("pyproject.toml"), "[project]\n").expect("write");
        let p = python(root.path()).expect("profile");
        assert_eq!(p.build_configuration["config"], "pyproject.toml");
    }

    #[test]
    fn toolchain_paths_are_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(toolchain(root.path()).expect("default"), "default");
        fs::write(
            root.path().join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.94.1\"\n",
        )
        .expect("write");
        assert_eq!(toolchain(root.path()).expect("channel"), "1.94.1");
        fs::write(
            root.path().join("rust-toolchain.toml"),
            "[toolchain]\npath = \"./evil\"\n",
        )
        .expect("write");
        assert!(matches!(
            toolchain(root.path()),
            Err(ToolError::Unsupported(_))
        ));
    }

    #[test]
    fn old_typescript_is_unsupported_not_substituted() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(
            root.path().join("package-lock.json"),
            r#"{"packages": {"node_modules/typescript": {"version": "5.9.3"}}}"#,
        )
        .expect("write");
        let Err(ToolError::Unsupported(reason)) = typescript(root.path()) else {
            panic!("expected UNSUPPORTED");
        };
        assert!(reason.contains("native LSP"), "{reason}");
    }
}
