//! The directory of pinned indexers easement finds on `PATH`, and the rest of its environment.
//!
//! `PATH` is that directory alone, so no host indexer, `python` or `pip` is reached: an
//! interpreter started in the workspace could import repository code.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use sych_core::Language;

use crate::semantic::{Resolve, deps};
use crate::tools::{self, ToolError, host_target};

/// No file has this path, so cargo fails before any build script or proc-macro compiles:
/// `rust-analyzer scip` always builds them, whatever its configuration says.
pub(super) const NO_RUSTC: &str = "/nonexistent/sych-compiles-no-repository-code";

/// The language of a project file easement reads: a source or a manifest.
pub(super) fn language(path: &str) -> Option<Language> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "Cargo.toml" => Some(Language::Rust),
        "go.mod" => Some(Language::Go),
        "package.json" => Some(Language::Typescript),
        _ => sych_core::source_language(path).map(|(language, _)| language),
    }
}

/// Fills `bin` with the pinned indexers of `languages` and returns the environment easement
/// runs in; `state_dir` holds the module cache `go mod download` fills.
pub(super) fn indexers(
    bin: &Path,
    languages: &BTreeSet<Language>,
    resolve: &Resolve<'_>,
    state_dir: &Path,
) -> Result<Vec<(String, OsString)>, ToolError> {
    fs::create_dir_all(bin)?;
    let pinned = |name: &str| resolve(tools::sole_pinned(name)?, &host_target());
    let link = |target: &Path, name: &str| symlink(target, &bin.join(name));
    let mut vars: Vec<(String, OsString)> = vec![("PATH".into(), bin.into())];
    if languages.contains(&Language::Rust) {
        link(&pinned("rust-analyzer")?, "rust-analyzer")?;
        // The host toolchain, as for the Rust language server; rustup proxies go by name.
        for program in ["cargo", "rustc"] {
            if let Some(found) = on_host_path(program) {
                link(&found, program)?;
            }
        }

        vars.extend(
            [
                ("RUSTC_WRAPPER", NO_RUSTC),
                ("CARGO_NET_OFFLINE", "true"),
                ("RUSTUP_AUTO_INSTALL", "0"),
            ]
            .map(|(k, v)| (k.to_owned(), v.into())),
        );
        let home = |var: &str, rest: &str| {
            env::var_os(var)
                .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(rest).into()))
        };
        vars.extend(home("CARGO_HOME", ".cargo").map(|v| ("CARGO_HOME".to_owned(), v)));
        vars.extend(home("RUSTUP_HOME", ".rustup").map(|v| ("RUSTUP_HOME".to_owned(), v)));
    }

    if languages.contains(&Language::Go) {
        link(&pinned("scip-go")?, "scip-go")?;
        link(&pinned("go")?, "go")?;
        vars.extend(
            deps::go_env(resolve, state_dir)?
                .into_iter()
                .filter(|(k, _)| k != "PATH")
                .map(|(k, v)| (k, v.into())),
        );
    }

    let typescript = languages.contains(&Language::Typescript);
    let python = languages.contains(&Language::Python);
    if typescript || python {
        link(&pinned("node")?, "node")?;
    }

    if typescript {
        link(&pinned("scip-typescript")?, "scip-typescript")?;
    }

    if python {
        let script = pinned("scip-python")?;
        let environment = bin.join("python-environment.json");
        fs::write(&environment, "[]")?;
        let shim = bin.join("scip-python");
        fs::write(&shim, python_shim(&script, &environment))?;
        executable(&shim)?;
    }

    Ok(vars)
}

/// scip-python with an empty `--environment`, which easement does not pass: without it
/// scip-python runs `pip` and `python` from `PATH` to list installed packages.
fn python_shim(script: &Path, environment: &Path) -> String {
    let quote = |p: &Path| serde_json::to_string(&p.display().to_string()).unwrap_or_default();
    let (script, environment) = (quote(script), quote(environment));
    format!(
        "#!/usr/bin/env node\n\
         process.argv.splice(3, 0, \"--environment\", {environment});\n\
         process.getBuiltinModule(\"module\").createRequire({script})({script});\n"
    )
}

fn on_host_path(program: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;
    symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    fs::copy(target, link).map(drop)
}

#[cfg(unix)]
fn executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn executable(_: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_holds_only_the_pinned_indexers_and_cargo_cannot_compile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).expect("tools");
        let resolve = |tool: &'static tools::PinnedTool, _: &str| {
            let path = tools.join(&tool.name);
            fs::write(&path, "").map(|()| path).map_err(ToolError::from)
        };
        let bin = dir.path().join("bin");
        let languages = [Language::Rust, Language::Typescript, Language::Python].into();
        let vars = indexers(&bin, &languages, &resolve, dir.path()).expect("indexers");
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PATH"), Some(bin.clone().into()));
        assert!(!Path::new(NO_RUSTC).exists());
        assert_eq!(get("RUSTC_WRAPPER"), Some(NO_RUSTC.into()));
        assert_eq!(get("CARGO_NET_OFFLINE"), Some("true".into()));
        assert!(get("GOPROXY").is_none(), "no Go in the project");
        for name in ["rust-analyzer", "node", "scip-typescript", "scip-python"] {
            assert!(bin.join(name).exists(), "{name}");
        }
        assert!(!bin.join("scip-go").exists() && !bin.join("python3").exists());
        let shim = fs::read_to_string(bin.join("scip-python")).expect("shim");
        let script =
            serde_json::to_string(&tools.join("scip-python").display().to_string()).expect("json");
        assert!(shim.contains("\"--environment\""), "{shim}");
        assert_eq!(shim.matches(&script).count(), 2, "{shim}");
        assert_eq!(
            fs::read_to_string(bin.join("python-environment.json")).expect("env"),
            "[]"
        );
    }

    #[test]
    fn manifests_count_as_their_language() {
        assert_eq!(language("crates/a/Cargo.toml"), Some(Language::Rust));
        assert_eq!(language("go.mod"), Some(Language::Go));
        assert_eq!(language("web/package.json"), Some(Language::Typescript));
        assert_eq!(language("web/src/a.tsx"), Some(Language::Typescript));
        assert_eq!(language("py/a.py"), Some(Language::Python));
        assert_eq!(language("README.md"), None);
    }
}
