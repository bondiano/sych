//! What a path is for, by name alone; the broker uses it to find related tests (spec §9.2).

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PathClass {
    Production,
    /// Tests, benches, end-to-end suites and stories: code that does not ship.
    Test,
    Generated,
    /// Package manifests, lockfiles and compiler configuration.
    Config,
    /// Build scripts and bundler configuration.
    Build,
}

const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "e2e", "benches", "testing"];
const TEST_MARKERS: &[&str] = &[".test.", ".spec.", ".stories.", ".e2e."];
const GENERATED_DIRS: &[&str] = &["generated", "__generated__"];
const GENERATED_MARKERS: &[&str] = &[".gen.", ".generated.", ".pb."];
const CONFIG_FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "package.json",
    "package-lock.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "yarn.lock",
];

/// `generated` is the snapshot's header-marker flag (`@generated`, `DO NOT EDIT`).
pub fn classify(path: &str, generated: bool) -> PathClass {
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    let in_dir = |set: &[&str]| dirs.split('/').any(|d| set.contains(&d));
    let marked = |set: &[&str]| set.iter().any(|m| name.contains(m));
    if generated || in_dir(GENERATED_DIRS) || marked(GENERATED_MARKERS) {
        PathClass::Generated
    } else if CONFIG_FILES.contains(&name)
        || (name.starts_with("tsconfig")
            && Path::new(name).extension().is_some_and(|e| e == "json"))
        || dirs.split('/').any(|d| d == ".cargo")
    {
        PathClass::Config
    } else if name == "build.rs"
        || name.starts_with("Dockerfile")
        || [".config.ts", ".config.js", ".config.mjs", ".config.cjs"]
            .iter()
            .any(|s| name.ends_with(s))
    {
        PathClass::Build
    } else if in_dir(TEST_DIRS)
        || marked(TEST_MARKERS)
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
    {
        PathClass::Test
    } else {
        PathClass::Production
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_follow_names_and_the_generated_flag() {
        let cases = [
            ("crates/sych-core/src/plan.rs", false, PathClass::Production),
            ("crates/sych-core/tests/config.rs", false, PathClass::Test),
            ("src/store_test.rs", false, PathClass::Test),
            (
                "packages/core/src/a/layer-session.test.ts",
                false,
                PathClass::Test,
            ),
            ("apps/pwa/e2e/food/search.spec.ts", false, PathClass::Test),
            ("apps/mobile/src/App.stories.tsx", false, PathClass::Test),
            ("src/api/client.gen.ts", false, PathClass::Generated),
            ("src/schema.ts", true, PathClass::Generated),
            ("apps/pwa/tsconfig.sw.json", false, PathClass::Config),
            ("Cargo.toml", false, PathClass::Config),
            ("crates/x/build.rs", false, PathClass::Build),
            ("apps/pwa/vite.config.ts", false, PathClass::Build),
            ("src/testimony.rs", false, PathClass::Production),
        ];
        for (path, generated, class) in cases {
            assert_eq!(classify(path, generated), class, "{path}");
        }
    }
}
