//! TypeScript project boundaries of a materialized side: tsconfig files with their `extends`,
//! project `references` and `paths` aliases, and the workspace packages.

use std::collections::BTreeMap;
use std::fs;
use std::iter::Peekable;
use std::path::Path;
use std::str::Chars;

use serde_json::Value;

use super::cargo;

fn is_tsconfig(name: &str) -> bool {
    name.starts_with("tsconfig")
        && Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// `tsconfig:<path>`, `package:<dir>` and `workspace` entries; unparseable files and
/// package `extends` that resolve only through installed dependencies are named.
pub fn configuration(root: &Path) -> (BTreeMap<String, String>, Vec<String>) {
    let mut config = BTreeMap::new();
    let mut limitations = Vec::new();
    for (path, text) in cargo::files(root, is_tsconfig) {
        let Ok(tsconfig) = serde_json::from_str::<Value>(&strip_jsonc(&text)) else {
            limitations.push(format!("{path} does not parse"));
            continue;
        };
        let mut parts = Vec::new();
        let extends: Vec<&str> = match &tsconfig["extends"] {
            Value::String(one) => vec![one],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for package in extends.iter().filter(|e| !e.starts_with('.')) {
            limitations.push(format!(
                "{path} extends package {package}: its options are known only with installed dependencies"
            ));
        }
        if !extends.is_empty() {
            parts.push(format!("extends={}", extends.join(",")));
        }

        let references: Vec<&str> = tsconfig["references"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["path"].as_str())
            .collect();
        if !references.is_empty() {
            parts.push(format!("references={}", references.join(",")));
        }

        if let Some(paths) = tsconfig["compilerOptions"]["paths"].as_object() {
            let aliases: Vec<&str> = paths.keys().map(String::as_str).collect();
            parts.push(format!("paths={}", aliases.join(",")));
        }

        if tsconfig["compilerOptions"]["composite"] == true {
            parts.push("composite".into());
        }

        config.insert(format!("tsconfig:{path}"), parts.join("; "));
    }
    for (path, text) in cargo::files(root, |name| name == "package.json") {
        let dir = path.strip_suffix("package.json").unwrap_or(&path);
        let dir = match dir.trim_end_matches('/') {
            "" => ".",
            dir => dir,
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(package) => {
                let name = package["name"].as_str().unwrap_or("(unnamed)");
                config.insert(format!("package:{dir}"), name.to_owned());
            }
            Err(_) => limitations.push(format!("{path} does not parse")),
        }
    }
    if let Ok(text) = fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        let globs: Vec<String> = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
            .ok()
            .and_then(|w| {
                let packages = w.get("packages")?.as_sequence()?;
                Some(
                    packages
                        .iter()
                        .filter_map(|p| p.as_str().map(str::to_owned))
                        .collect(),
                )
            })
            .unwrap_or_default();
        config.insert("workspace".into(), format!("pnpm: {}", globs.join(",")));
    }

    (config, limitations)
}

/// JSON with comments and trailing commas, as tsconfig allows, made strict JSON.
fn strip_jsonc(text: &str) -> String {
    let without_comments = strip_outside_strings(text, |c, rest, out| match (c, rest.peek()) {
        ('/', Some('/')) => while rest.next_if(|&n| n != '\n').is_some() {},
        ('/', Some('*')) => {
            rest.next();
            while let Some(n) = rest.next() {
                if n == '*' && rest.next_if_eq(&'/').is_some() {
                    break;
                }
            }
        }
        _ => out.push(c),
    });
    strip_outside_strings(&without_comments, |c, rest, out| {
        let closes = || rest.clone().find(|n: &char| !n.is_whitespace());
        if c != ',' || !matches!(closes(), Some('}' | ']')) {
            out.push(c);
        }
    })
}

/// Copies string literals verbatim and hands every other character to `other`.
fn strip_outside_strings(
    text: &str,
    other: impl Fn(char, &mut Peekable<Chars<'_>>, &mut String),
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string || c == '"' {
            out.push(c);
            match c {
                '\\' if in_string => out.extend(chars.next()),
                '"' => in_string = !in_string,
                _ => {}
            }
            continue;
        }

        other(c, &mut chars, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let text = "{\n  // a comment\n  \"a\": \"x // not a comment\", /* block */\n  \"b\": [1, 2,],\n}\n";
        let value: Value = serde_json::from_str(&strip_jsonc(text)).expect("strict JSON");
        assert_eq!(value["a"], "x // not a comment");
        assert_eq!(value["b"], serde_json::json!([1, 2]));
    }

    #[test]
    fn references_aliases_packages_and_workspace_are_recorded() {
        let root = tempfile::tempdir().expect("tempdir");
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, text).expect("write");
        };
        write(
            "pnpm-workspace.yaml",
            "packages:\n  - apps/*\n  - packages/*\n",
        );
        write("package.json", r#"{"name": "root"}"#);
        write(
            "tsconfig.base.json",
            r#"{"compilerOptions": {"paths": {"@core/*": ["./packages/core/src/*"]}}}"#,
        );
        write(
            "apps/pwa/tsconfig.json",
            "{\"extends\": \"../../tsconfig.base.json\", // base\n\"references\": [{\"path\": \"../../packages/core\"}],}",
        );
        write("apps/pwa/package.json", r#"{"name": "@app/pwa"}"#);
        write(
            "packages/core/tsconfig.json",
            r#"{"extends": "@tsconfig/strictest", "compilerOptions": {"composite": true}}"#,
        );
        write("packages/core/node_modules/x/tsconfig.json", "not read");
        write("packages/broken/tsconfig.json", "{");
        let (config, limitations) = configuration(root.path());
        assert_eq!(config["workspace"], "pnpm: apps/*,packages/*");
        assert_eq!(config["package:."], "root");
        assert_eq!(config["package:apps/pwa"], "@app/pwa");
        assert_eq!(config["tsconfig:tsconfig.base.json"], "paths=@core/*");
        assert_eq!(
            config["tsconfig:apps/pwa/tsconfig.json"],
            "extends=../../tsconfig.base.json; references=../../packages/core"
        );
        assert_eq!(
            config["tsconfig:packages/core/tsconfig.json"],
            "extends=@tsconfig/strictest; composite"
        );
        assert_eq!(limitations.len(), 2, "{limitations:?}");
        assert!(
            limitations
                .iter()
                .any(|l| l.contains("@tsconfig/strictest"))
        );
        assert!(limitations.contains(&"packages/broken/tsconfig.json does not parse".to_owned()));
    }
}
