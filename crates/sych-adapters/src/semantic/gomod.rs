//! Go build configuration of a materialized side (spec §10.3): modules, `go.work`,
//! replacements, toolchain directives and the platform gopls analyses.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::tools::host_target;

/// Directories walked looking for `go.mod`; deeper modules are not found.
const MAX_DEPTH: usize = 6;
/// Toolchain every module is analysed with; a newer `go` directive is a limitation.
pub const PINNED_GO: &str = "1.27.3";

/// Every `go.mod` under `root`, skipping `vendor`, `testdata` and hidden directories.
pub fn modules(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if depth < MAX_DEPTH
                    && !name.starts_with('.')
                    && !matches!(name, "vendor" | "testdata" | "node_modules")
                {
                    walk(&path, depth + 1, out);
                }
            } else if name == "go.mod" {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, 0, &mut out);
    out
}

/// The directive lines of a `go.mod`/`go.work`: `(keyword, rest)`, blocks flattened.
fn directives(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut block: Option<String> = None;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }

        if let Some(keyword) = &block {
            if line == ")" {
                block = None;
            } else {
                out.push((keyword.clone(), line.to_owned()));
            }

            continue;
        }

        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        if rest == "(" {
            block = Some(keyword.to_owned());
        } else {
            out.push((keyword.to_owned(), rest.to_owned()));
        }
    }
    out
}

/// `1.27.3` > `1.27`: numeric comparison of dotted versions.
fn newer(version: &str, than: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> {
        v.split(['.', 'r', 'b'])
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(version) > parts(than)
}

/// Build configuration entries and limitations of the side at `root`.
pub fn configuration(root: &Path) -> (BTreeMap<String, String>, Vec<String>) {
    let mut config = BTreeMap::new();
    let mut limitations = Vec::new();
    let (goos, goarch) = host_target()
        .split_once('-')
        .map(|(arch, os)| {
            let os = if os == "macos" { "darwin" } else { os };
            let arch = match arch {
                "x86_64" => "amd64",
                "aarch64" => "arm64",
                other => other,
            };
            (os.to_owned(), arch.to_owned())
        })
        .unwrap_or_default();
    config.extend(
        [
            (
                "toolchain",
                format!("go {PINNED_GO} (pinned, GOTOOLCHAIN=local)"),
            ),
            ("goos", goos),
            ("goarch", goarch),
            ("build_tags", "none".into()),
            ("cgo", "disabled".into()),
        ]
        .map(|(k, v)| (k.to_owned(), v)),
    );
    if let Ok(text) = fs::read_to_string(root.join("go.work")) {
        let uses: Vec<String> = directives(&text)
            .into_iter()
            .filter(|(k, _)| k == "use")
            .map(|(_, v)| v)
            .collect();
        config.insert("go.work".into(), format!("use {}", uses.join(", ")));
        replacements(&text, "go.work", root, &mut config, &mut limitations);
    }

    if root.join("vendor/modules.txt").is_file() {
        config.insert("vendor".into(), "vendor/modules.txt".into());
    }

    for manifest in modules(root) {
        let rel = manifest
            .strip_prefix(root)
            .unwrap_or(&manifest)
            .display()
            .to_string();
        let Ok(text) = fs::read_to_string(&manifest) else {
            limitations.push(format!("{rel}: not readable"));
            continue;
        };
        let lines = directives(&text);
        let get = |key: &str| lines.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        let module = get("module").unwrap_or_else(|| "?".into());
        let go = get("go").unwrap_or_else(|| "unset".into());
        let requires = lines.iter().filter(|(k, _)| k == "require").count();
        let mut summary = format!("{module}, go {go}, {requires} requirements");
        if let Some(toolchain) = get("toolchain") {
            summary = format!("{summary}, toolchain {toolchain}");
        }

        if go != "unset" && newer(&go, PINNED_GO) {
            limitations.push(format!(
                "{rel}: requires go {go}, newer than the pinned {PINNED_GO}; packages may not load"
            ));
        }

        config.insert(format!("module:{rel}"), summary);
        let dir = manifest.parent().unwrap_or(root);
        replacements(&text, &rel, dir, &mut config, &mut limitations);
    }
    limitations.extend([
        format!(
            "references and call hierarchy cover only the active build configuration \
             ({}/{}, no extra build tags, cgo off): files other GOOS, GOARCH or tags select \
             are not analysed, so missing references are not absence across all builds",
            config["goos"], config["goarch"]
        ),
        "gopls call hierarchy does not follow interface dispatch, reflection or function \
         values: callers through them are missing"
            .into(),
    ]);
    (config, limitations)
}

/// `replace` directives: local paths outside the snapshot cannot resolve.
fn replacements(
    text: &str,
    file: &str,
    dir: &Path,
    config: &mut BTreeMap<String, String>,
    limitations: &mut Vec<String>,
) {
    for (_, rule) in directives(text).into_iter().filter(|(k, _)| k == "replace") {
        let Some((from, to)) = rule.split_once("=>") else {
            continue;
        };
        let (from, to) = (from.trim(), to.trim());
        config.insert(format!("replace:{file}:{from}"), to.to_owned());
        let target = to.split_whitespace().next().unwrap_or(to);
        if target.starts_with("./") || target.starts_with("../") || target.starts_with('/') {
            let resolved = dir.join(target);
            if !resolved.join("go.mod").is_file() {
                limitations.push(format!(
                    "{file}: replacement {from} => {target} is outside the snapshot or has no \
                     go.mod; its packages stay unresolved"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_replacements_and_toolchain_are_recorded() {
        let root = tempfile::tempdir().expect("tempdir");
        let r = root.path();
        fs::create_dir_all(r.join("svc/internal")).expect("dirs");
        fs::create_dir_all(r.join("lib")).expect("dirs");
        fs::write(r.join("go.work"), "go 1.27\n\nuse (\n\t./svc\n\t./lib\n)\n").expect("write");
        fs::write(
            r.join("svc/go.mod"),
            "module example.com/svc\n\ngo 1.28.0\n\nrequire (\n\texample.com/lib v0.0.0\n\tgolang.org/x/sync v0.10.0 // indirect\n)\n\n\
             replace example.com/lib => ../lib\nreplace example.com/gone => ../../gone\n",
        )
        .expect("write");
        fs::write(r.join("lib/go.mod"), "module example.com/lib\n\ngo 1.26\n").expect("write");
        let (config, limitations) = configuration(r);
        assert_eq!(config["go.work"], "use ./svc, ./lib");
        assert_eq!(
            config["module:svc/go.mod"],
            "example.com/svc, go 1.28.0, 2 requirements"
        );
        assert_eq!(config["replace:svc/go.mod:example.com/lib"], "../lib");
        assert!(config.contains_key("goos") && config["cgo"] == "disabled");
        let joined = limitations.join("\n");
        assert!(joined.contains("example.com/gone"), "{joined}");
        assert!(!joined.contains("example.com/lib =>"), "{joined}");
        assert!(joined.contains("requires go 1.28.0"), "{joined}");
        assert!(!joined.contains("lib/go.mod: requires"), "{joined}");
    }
}
