//! Semantic cache (spec §9.6): facts of one side keyed by snapshot, language, server version
//! and server configuration. The access partition is stored with the entry and checked on
//! every hit, so the same key never serves another repository.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sych_core::{
    BackendStatus, EdgeSet, Language, QueryStatus, SemanticFacts, SourceSide, SourceSnapshotId,
    canonical_sha256,
};

use sych_core::config::Tooling;

use super::profiles::Profile;

#[derive(Serialize, Deserialize)]
struct Entry {
    partition: String,
    facts: SemanticFacts,
}

/// The snapshot id already pins both trees and the diff, hence the queried files.
pub fn key(
    snapshot: &SourceSnapshotId,
    side: SourceSide,
    language: Language,
    profile: &Profile,
    tooling: &Tooling,
) -> Option<String> {
    canonical_sha256(&json!({
        "engine": env!("CARGO_PKG_VERSION"),
        // Discard facts computed before workspace roots were canonicalized.
        "revision": 1,
        "snapshot": snapshot,
        "side": side,
        "language": language,
        "tool": [&profile.tool.name, &profile.tool.version],
        "args": profile.args,
        "init": profile.init_options,
        "build": profile.build_configuration,
        "install": [tooling.dependency_install, &tooling.sandbox.backends],
    }))
    .ok()
    .map(|d| d.hex().to_owned())
}

fn path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

pub fn load(dir: &Path, key: &str, partition: &str) -> Option<SemanticFacts> {
    let entry: Entry = serde_json::from_slice(&fs::read(path(dir, key)).ok()?).ok()?;
    (entry.partition == partition).then_some(entry.facts)
}

/// Marks a diagnostics request that failed on one file.
pub const DIAGNOSTICS_FAILED: &str = "diagnostics failed";

/// A query that failed this time may answer next time; an unsupported one will not.
fn transiently_failed(facts: &SemanticFacts) -> bool {
    let failed = |edges: &EdgeSet| edges.status == QueryStatus::Failed;
    facts.symbols.iter().any(|s| {
        [
            &s.references,
            &s.callers,
            &s.callees,
            &s.type_definitions,
            &s.implementations,
        ]
        .into_iter()
        .any(failed)
    }) || facts
        .limitations
        .iter()
        .any(|l| l.contains(DIAGNOSTICS_FAILED))
}

/// Only complete answers are cached; a failed or unsupported backend, or a failed query,
/// is retried next time.
pub fn store(dir: &Path, key: &str, partition: &str, facts: &SemanticFacts) {
    if facts.status != BackendStatus::Ready
        || transiently_failed(facts)
        || fs::create_dir_all(dir).is_err()
    {
        return;
    }

    let entry = Entry {
        partition: partition.to_owned(),
        facts: facts.clone(),
    };
    // A lost cache write only costs a recomputation.
    let _ = tempfile::NamedTempFile::new_in(dir).and_then(|mut file| {
        file.write_all(&serde_json::to_vec(&entry).map_err(io::Error::other)?)?;
        file.persist(path(dir, key)).map_err(|e| e.error)?;
        Ok(())
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sych_core::{
        CodeRef, GitHashAlgorithm, GitObjectId, SchemaVersion, SymbolFacts, classify, sha256,
    };

    use super::*;
    use crate::tools;

    fn profile() -> Profile {
        Profile {
            tool: tools::sole_pinned("rust-analyzer").expect("pinned"),
            args: &[],
            init_options: json!({}),
            wait_quiescent: false,
            build_configuration: BTreeMap::new(),
            limitations: Vec::new(),
            env: Vec::new(),
        }
    }

    fn facts() -> SemanticFacts {
        SemanticFacts {
            schema_version: SchemaVersion,
            source_snapshot_id: SourceSnapshotId(sha256(b"a")),
            side: SourceSide::Head,
            language: Language::Rust,
            status: BackendStatus::Ready,
            producer: None,
            position_encoding: None,
            build_configuration: BTreeMap::new(),
            query_scope: vec!["src/lib.rs".into()],
            symbols: Vec::new(),
            diagnostics: Vec::new(),
            limitations: Vec::new(),
        }
    }

    #[test]
    fn pre_canonical_workspace_facts_are_not_reused() {
        let dir = tempfile::tempdir().expect("dir");
        let cached = facts();
        let snapshot = &cached.source_snapshot_id;
        let profile = profile();
        let tooling = Tooling::default();
        let legacy_key = canonical_sha256(&json!({
            "engine": env!("CARGO_PKG_VERSION"),
            "snapshot": snapshot,
            "side": SourceSide::Head,
            "language": Language::Rust,
            "tool": [&profile.tool.name, &profile.tool.version],
            "args": profile.args,
            "init": profile.init_options,
            "build": profile.build_configuration,
            "install": [tooling.dependency_install, &tooling.sandbox.backends],
        }))
        .expect("legacy key")
        .hex()
        .to_owned();
        store(dir.path(), &legacy_key, "repo", &cached);
        let current_key = key(
            snapshot,
            SourceSide::Head,
            Language::Rust,
            &profile,
            &tooling,
        )
        .expect("current key");
        assert_eq!(
            load(dir.path(), &current_key, "repo"),
            None,
            "facts from pre-canonical-root analysis must be recomputed"
        );
    }

    #[test]
    fn a_hit_is_served_only_to_its_access_partition() {
        let dir = tempfile::tempdir().expect("dir");
        let snapshot = SourceSnapshotId(sha256(b"a"));
        let key = key(
            &snapshot,
            SourceSide::Head,
            Language::Rust,
            &profile(),
            &Tooling::default(),
        )
        .expect("key");
        store(dir.path(), &key, "repo-a", &facts());
        assert_eq!(load(dir.path(), &key, "repo-a"), Some(facts()));
        assert_eq!(
            load(dir.path(), &key, "repo-b"),
            None,
            "access is checked on hit"
        );
    }

    #[test]
    fn a_failed_query_is_not_cached_but_an_unsupported_one_is() {
        let dir = tempfile::tempdir().expect("dir");
        let definition = CodeRef {
            snapshot_id: SourceSnapshotId(sha256(b"a")),
            side: SourceSide::Head,
            path: "src/lib.rs".parse().expect("path"),
            blob_oid: GitObjectId::new(GitHashAlgorithm::Sha1, &"a".repeat(40)).expect("oid"),
            start_line: 1,
            end_line: 1,
            content_sha256: sha256(b"fn f() {}"),
        };
        let with = |references| {
            let mut facts = facts();
            facts.symbols.push(SymbolFacts {
                name: "f".into(),
                kind: "function".into(),
                container: Vec::new(),
                definition: definition.clone(),
                path_class: classify("src/lib.rs", false),
                references,
                readers: EdgeSet::not_applicable(),
                writers: EdgeSet::not_applicable(),
                callers: EdgeSet::unsupported(),
                calling_constructs: Vec::new(),
                callees: EdgeSet::unsupported(),
                type_definitions: EdgeSet::unsupported(),
                implementations: EdgeSet::not_applicable(),
            });
            facts
        };
        store(
            dir.path(),
            "unsupported",
            "repo",
            &with(EdgeSet::unsupported()),
        );
        assert!(load(dir.path(), "unsupported", "repo").is_some());
        store(
            dir.path(),
            "failed",
            "repo",
            &with(EdgeSet::failed("timeout".into())),
        );
        assert_eq!(load(dir.path(), "failed", "repo"), None);
        let mut diagnostics = facts();
        diagnostics
            .limitations
            .push(format!("src/lib.rs: {DIAGNOSTICS_FAILED}: timeout"));
        store(dir.path(), "diagnostics", "repo", &diagnostics);
        assert_eq!(load(dir.path(), "diagnostics", "repo"), None);
    }

    #[test]
    fn a_wider_or_different_search_scope_is_a_different_key() {
        let tooling = Tooling::default();
        let at = |snapshot: &[u8], side, tooling: &Tooling| {
            key(
                &SourceSnapshotId(sha256(snapshot)),
                side,
                Language::Rust,
                &profile(),
                tooling,
            )
        };
        let base = at(b"a", SourceSide::Head, &tooling);
        // The snapshot id pins the queried files, so another diff never reuses an answer.
        assert_ne!(base, at(b"b", SourceSide::Head, &tooling));
        assert_ne!(base, at(b"a", SourceSide::DiffBase, &tooling));
        // Installed dependencies widen what references can find; an empty answer without
        // them is not reused for a run with them.
        let mut installed = tooling.clone();
        installed.dependency_install = true;
        assert_ne!(base, at(b"a", SourceSide::Head, &installed));
    }
}
