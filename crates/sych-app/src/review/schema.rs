//! Schema changes of the changed migrations and the code of both sides that names them
//!: the old app's readers and writers on the base, the new app's on the head.
//! The search covers this repository only; consumers outside it stay a named gap.

use sych_adapters::git::LocalSnapshot;
use sych_core::{
    CodeRef, SchemaRef, SnapshotReader, SourceSide, is_migration_path, schema_changes,
};

/// References kept per side and name; more marks the result truncated.
const SEARCH_LIMIT: usize = 20;
/// Lines shown around a hit: a reader needs its function, not the matching line alone.
const WINDOW: u32 = 12;

/// The hit with up to [`WINDOW`] lines on each side, clamped to the file.
fn widen(snap: &LocalSnapshot, r: CodeRef) -> CodeRef {
    let start = r.start_line.saturating_sub(WINDOW).max(1);
    let mut end = r.end_line + WINDOW;
    loop {
        if let Ok(wide) = snap.code_ref(r.side, &r.path, start, end) {
            return wide;
        }

        if end == r.end_line {
            return r;
        }

        end = u32::midpoint(r.end_line, end);
    }
}

pub(super) fn schema_refs(snap: &LocalSnapshot) -> Vec<SchemaRef> {
    let mut out = Vec::new();
    for file in &snap.manifest.files {
        let Some(side) = file
            .side(SourceSide::Head)
            .filter(|s| is_migration_path(s.path.as_str()))
        else {
            continue;
        };
        let read = snap
            .code_ref(
                SourceSide::Head,
                &side.path,
                1,
                side.line_count.unwrap_or(1).max(1),
            )
            .and_then(|r| snap.read_span(&r).map(|s| (r, s.text)));
        let (migration, text) = match read {
            Ok(read) => read,
            Err(err) => {
                tracing::warn!(path = %side.path, error = %err, "migration not read");
                continue;
            }
        };
        for change in schema_changes(&text) {
            let name = change.column.as_deref().unwrap_or(&change.table);
            let mut truncated = false;
            let mut references = Vec::new();
            for s in [SourceSide::DiffBase, SourceSide::Head] {
                let mut hits: Vec<_> = snap
                    .search(s, name, &[], SEARCH_LIMIT + 1)
                    .unwrap_or_default()
                    .into_iter()
                    // Earlier migrations show the schema it changes; only itself is noise.
                    .filter(|r| r.path != migration.path)
                    .collect();
                truncated |= hits.len() > SEARCH_LIMIT;
                hits.truncate(SEARCH_LIMIT);
                references.extend(hits.into_iter().map(|r| widen(snap, r)));
            }
            out.push(SchemaRef {
                change,
                migration: migration.clone(),
                references,
                truncated,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::schema_refs;
    use crate::args::Comparison;
    use crate::{eval, snapshot};

    #[test]
    fn a_dropped_column_is_linked_to_the_code_of_both_sides_that_reads_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = BTreeMap::from([
            (
                "migrations/0001.sql".to_owned(),
                "CREATE TABLE orders (id TEXT, legacy_status TEXT);\n".to_owned(),
            ),
            (
                "orders/read.go".to_owned(),
                "package orders\n\nconst q = \"SELECT legacy_status FROM orders\"\n".to_owned(),
            ),
        ]);
        let head = BTreeMap::from([(
            "migrations/0002.sql".to_owned(),
            Some("ALTER TABLE orders DROP COLUMN legacy_status;\n".to_owned()),
        )]);
        eval::build_inline(dir.path(), &base, &head).expect("repo");
        let snap = snapshot::capture(&Comparison {
            repo: dir.path().to_path_buf(),
            base: "target".into(),
            head: "HEAD".into(),
            diff_base: None,
        })
        .expect("snapshot");
        let refs = schema_refs(&snap);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].change.column.as_deref(), Some("legacy_status"));
        let paths: Vec<&str> = refs[0].references.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "migrations/0001.sql",
                "orders/read.go",
                "migrations/0001.sql",
                "orders/read.go"
            ],
            "base and head readers, and the earlier migration that defines the column"
        );
        let read = &refs[0].references[1];
        assert_eq!(
            (read.start_line, read.end_line),
            (1, 3),
            "a hit is widened to its surroundings, clamped to the file"
        );
    }
}
