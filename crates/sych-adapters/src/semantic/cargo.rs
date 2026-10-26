//! Cargo package boundaries, editions and declared features of a materialized side.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use toml::{Table, Value};

const SKIPPED_DIRS: &[&str] = &["target", "node_modules", ".git"];

/// Every file under `root` whose name `wanted` accepts, as `(relative path, text)`, sorted.
pub fn files(root: &Path, wanted: fn(&str) -> bool) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, wanted: fn(&str) -> bool, out: &mut Vec<(String, String)>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() && !SKIPPED_DIRS.iter().any(|s| name == *s) {
                walk(root, &path, wanted, out);
            } else if name.to_str().is_some_and(wanted)
                && let Ok(text) = fs::read_to_string(&path)
            {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                out.push((rel.display().to_string(), text));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, wanted, &mut out);
    out.sort();
    out
}

/// Every `Cargo.toml` under `root` as `(relative dir, text)`, sorted.
pub fn manifests(root: &Path) -> Vec<(String, String)> {
    files(root, |name| name == "Cargo.toml")
        .into_iter()
        .map(|(path, text)| {
            let dir = path.strip_suffix("Cargo.toml").unwrap_or(&path);
            let dir = dir.trim_end_matches('/');
            (
                if dir.is_empty() {
                    ".".into()
                } else {
                    dir.to_owned()
                },
                text,
            )
        })
        .collect()
}

/// `edition` and one `features:<package>` entry per package; unparseable manifests are named.
pub fn configuration(manifests: &[(String, String)]) -> (BTreeMap<String, String>, Vec<String>) {
    let parsed: Vec<(&str, Option<Table>)> = manifests
        .iter()
        .map(|(dir, text)| (dir.as_str(), text.parse::<Table>().ok()))
        .collect();
    let workspace_edition = parsed
        .iter()
        .find(|(dir, _)| *dir == ".")
        .and_then(|(_, t)| t.as_ref()?.get("workspace")?.get("package")?.get("edition"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut config = BTreeMap::new();
    let mut editions = BTreeSet::new();
    let mut limitations = Vec::new();
    for (dir, table) in parsed {
        let Some(table) = table else {
            limitations.push(format!("{dir}/Cargo.toml does not parse"));
            continue;
        };
        let Some(package) = table.get("package") else {
            continue;
        };
        let name = package.get("name").and_then(Value::as_str).unwrap_or(dir);
        let edition = match package.get("edition") {
            Some(Value::String(e)) => Some(e.clone()),
            Some(Value::Table(t)) if t.get("workspace") == Some(&Value::Boolean(true)) => {
                workspace_edition.clone()
            }
            // Cargo's default when the key is absent.
            None => Some("2015".into()),
            Some(_) => None,
        };
        editions.extend(edition);
        let features: Vec<&str> = table
            .get("features")
            .and_then(Value::as_table)
            .map(|f| f.keys().map(String::as_str).collect())
            .unwrap_or_default();
        if !features.is_empty() {
            config.insert(format!("features:{name}"), features.join(","));
        }
    }
    if !editions.is_empty() {
        config.insert(
            "edition".into(),
            editions.into_iter().collect::<Vec<_>>().join(","),
        );
    }

    (config, limitations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editions_inherit_from_the_workspace_and_features_are_listed() {
        let manifests = [
            (
                ".".to_owned(),
                "[workspace]\nmembers = [\"a\"]\n[workspace.package]\nedition = \"2024\"\n".to_owned(),
            ),
            (
                "a".to_owned(),
                "[package]\nname = \"a\"\nedition.workspace = true\n[features]\ndefault = [\"x\"]\nx = []\n"
                    .to_owned(),
            ),
            ("b".to_owned(), "[package]\nname = \"b\"\nedition = \"2021\"\n".to_owned()),
            ("bad".to_owned(), "[package\n".to_owned()),
        ];
        let (config, limitations) = configuration(&manifests);
        assert_eq!(config["edition"], "2021,2024");
        assert_eq!(config["features:a"], "default,x");
        assert!(!config.contains_key("features:b"));
        assert_eq!(limitations, ["bad/Cargo.toml does not parse"]);
    }
}
