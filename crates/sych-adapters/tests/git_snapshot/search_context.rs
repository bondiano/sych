//! Evidence availability, not model accuracy: compare exact hits with the context actually
//! handed to verification, on existing labelled safe/unsafe pairs and pinned Git objects.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

use serde::Deserialize;
use sych_core::analysis::{AskOperation, Context, ContextAsk};
use sych_core::broker::{OpStatus, Ops};
use sych_core::{
    CodeRef, DiffManifest, ReadError, RepoPath, SnapshotReader, SourceSide, SourceSnapshotId,
    SourceSpan, sha256,
};

use super::{capture, commit, git, path, repo, write};

#[derive(Deserialize)]
struct Case {
    base: BTreeMap<String, String>,
    head: BTreeMap<String, String>,
}

const CASES: &[(&str, &str, &str, &str)] = &[
    (
        "go-path-traversal-safe",
        "reports.go",
        "http.ServeFile",
        "if !filepath.IsLocal(name)",
    ),
    (
        "go-path-traversal-unsafe",
        "reports.go",
        "http.ServeFile",
        "name := r.URL.Query()",
    ),
    (
        "rust-path-traversal-safe",
        "src/lib.rs",
        "tokio::fs::read",
        "return Err(StatusCode::BAD_REQUEST)",
    ),
    (
        "rust-path-traversal-unsafe",
        "src/lib.rs",
        "tokio::fs::read",
        ".join(&name)",
    ),
    (
        "react-stale-response-safe",
        "src/OrderDetails.tsx",
        "fetchOrder",
        "return () => controller.abort()",
    ),
    (
        "react-stale-response-unsafe",
        "src/OrderDetails.tsx",
        "fetchOrder",
        "}, [id])",
    ),
    (
        "ts-ssrf-safe",
        "src/routes/avatar.ts",
        "await fetch",
        "!IMAGE_HOSTS.has(url.hostname)",
    ),
    (
        "ts-ssrf-unsafe",
        "src/routes/avatar.ts",
        "await fetch",
        "Buffer.from(await response.arrayBuffer())",
    ),
];

#[test]
fn search_context_exposes_nearby_evidence_in_existing_quality_pairs() {
    let (mut before, mut after, mut before_bytes, mut after_bytes) = (0, 0, 0, 0);
    let started = Instant::now();
    for &(id, file, query, evidence) in CASES {
        let yaml = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../eval/quality/dev/{id}.yaml")),
        )
        .expect("existing quality case");
        let case: Case = serde_yaml_ng::from_str(&yaml).expect("case");
        assert!(
            case.head[file].contains(evidence),
            "{id}: independent source oracle"
        );
        let dir = repo();
        for (name, text) in &case.base {
            write(dir.path(), name, text.as_bytes());
        }
        commit(dir.path(), "base");
        git(dir.path(), &["branch", "base"]);
        for (name, text) in &case.head {
            write(dir.path(), name, text.as_bytes());
        }
        commit(dir.path(), "head");
        let snapshot = capture(dir.path(), "base", "HEAD", None).expect("snapshot");
        // Dirty working-tree content must never enter evidence.
        write(dir.path(), file, b"not the pinned evidence\n");
        let raw = snapshot
            .search(SourceSide::Head, query, &[path(file)], 10)
            .expect("literal search");
        assert_eq!(raw.len(), 1, "{id}: a single selected hit");
        let old = snapshot.read_span(&raw[0]).expect("original hit").text;
        before += usize::from(old.contains(evidence));
        before_bytes += old.len();
        let ops = Ops {
            generated: &[],
            facts: &[],
            evidence: &[],
            reader: &snapshot,
        };
        let ask = ContextAsk {
            question: "what surrounds the operation?".into(),
            operation: AskOperation::SearchSource {
                side: SourceSide::Head,
                query: query.into(),
            },
        };
        let mut context = Context::default();
        let answered = ask.answer(&ops, &mut context, 3);
        assert_eq!(answered.status, Some(OpStatus::CompleteForQuery));
        assert_eq!(answered.refs, 1);
        let reference = context.span("ref-1").expect("shown span");
        assert_eq!(reference.blob_oid, raw[0].blob_oid);
        assert!(reference.end_line - reference.start_line < 17);
        let shown = snapshot.read_span(reference).expect("checked window").text;
        assert!(shown.len() <= 4096);
        assert!(shown.contains(evidence), "{id}: {shown}");
        assert!(!shown.contains("not the pinned evidence"));
        after += usize::from(shown.contains(evidence));
        after_bytes += shown.len();
    }
    println!(
        "evidence availability: {before}/8 -> {after}/8; source bytes: \
         {before_bytes} -> {after_bytes}; elapsed with Git setup: {:?}",
        started.elapsed()
    );
    assert_eq!(before, 0);
    assert_eq!(after, 8);
}

fn answer(snapshot: &impl SnapshotReader, query: &str, generated: &[RepoPath]) -> Context {
    let ops = Ops {
        generated,
        facts: &[],
        evidence: &[],
        reader: snapshot,
    };
    let mut context = Context::default();
    ContextAsk {
        question: "nearby code?".into(),
        operation: AskOperation::SearchSource {
            side: SourceSide::Head,
            query: query.into(),
        },
    }
    .answer(&ops, &mut context, 3);
    context
}

#[test]
fn windows_are_bounded_at_both_file_edges_and_on_long_lines() {
    let dir = repo();
    write(
        dir.path(),
        "src/edges.ts",
        "first α\nmiddle\nlast 😀".as_bytes(),
    );
    let long = format!("{}\nneedle\n{}\n", "x".repeat(5000), "y".repeat(5000));
    write(dir.path(), "src/long.ts", long.as_bytes());
    let shrunk = format!(
        "{}\na\nshrink_me\nb\n{}\n",
        "x".repeat(5000),
        "y".repeat(5000)
    );
    write(dir.path(), "src/shrink.ts", shrunk.as_bytes());
    for (query, padding) in [("cap_hit1", 4056), ("cap_hit2", 4057)] {
        let text = format!(
            "{}\n{}{query}\n{}",
            "x".repeat(padding),
            "p\n".repeat(7),
            "q\n".repeat(8),
        );
        write(dir.path(), format!("src/{query}.ts"), text.as_bytes());
    }
    commit(dir.path(), "base");
    let snapshot = capture(dir.path(), "HEAD", "HEAD", None).expect("snapshot");
    for query in ["first", "last"] {
        let context = answer(&snapshot, query, &[]);
        let reference = context.span("ref-1").expect("edge span");
        assert_eq!((reference.start_line, reference.end_line), (1, 3));
        assert!(
            snapshot
                .read_span(reference)
                .expect("hash")
                .text
                .contains('😀')
        );
    }
    let context = answer(&snapshot, "needle", &[]);
    let reference = context.span("ref-1").expect("exact fallback");
    assert_eq!((reference.start_line, reference.end_line), (2, 2));
    let context = answer(&snapshot, "shrink_me", &[]);
    let reference = context.span("ref-1").expect("smaller window");
    assert_eq!((reference.start_line, reference.end_line), (2, 4));
    for (query, expected) in [("cap_hit1", (1, 17)), ("cap_hit2", (5, 13))] {
        let context = answer(&snapshot, query, &[]);
        let reference = context.span("ref-1").expect("byte boundary");
        assert_eq!((reference.start_line, reference.end_line), expected);
        let text = snapshot.read_span(reference).expect("hash").text;
        assert!(text.len() <= 4096);
        if query == "cap_hit1" {
            assert_eq!(text.len(), 4096);
        }
    }
}

/// Fail only expanded reads: the exact search hit remains valid.
struct FailedExpansion<'a> {
    reader: &'a dyn SnapshotReader,
    corrupt_hash: bool,
}

impl SnapshotReader for FailedExpansion<'_> {
    fn read_span(&self, reference: &CodeRef) -> Result<SourceSpan, ReadError> {
        self.reader.read_span(reference)
    }

    fn code_ref(
        &self,
        side: SourceSide,
        path: &RepoPath,
        start: u32,
        end: u32,
    ) -> Result<CodeRef, ReadError> {
        if start != end && !self.corrupt_hash {
            return Err(ReadError::Unavailable("expanded span unavailable".into()));
        }

        let mut reference = self.reader.code_ref(side, path, start, end)?;
        if start != end {
            reference.content_sha256 = sha256(b"wrong hash");
        }

        Ok(reference)
    }

    fn search(
        &self,
        side: SourceSide,
        literal: &str,
        paths: &[RepoPath],
        limit: usize,
    ) -> Result<Vec<CodeRef>, ReadError> {
        self.reader.search(side, literal, paths, limit)
    }

    fn diff(&self, snapshot: &SourceSnapshotId) -> Result<DiffManifest, ReadError> {
        self.reader.diff(snapshot)
    }
}

#[test]
fn failed_or_hash_mismatched_expansion_preserves_the_exact_hit() {
    let dir = repo();
    write(dir.path(), "src/api.ts", b"before\nneedle\nafter\n");
    commit(dir.path(), "base");
    let snapshot = capture(dir.path(), "HEAD", "HEAD", None).expect("snapshot");
    for corrupt_hash in [false, true] {
        let context = answer(
            &FailedExpansion {
                reader: &snapshot,
                corrupt_hash,
            },
            "needle",
            &[],
        );
        let reference = context.span("ref-1").expect("exact fallback");
        assert_eq!((reference.start_line, reference.end_line), (2, 2));
        assert_eq!(
            snapshot.read_span(reference).expect("valid hash").text,
            "needle\n"
        );
        assert!(context.limitations.is_empty());
    }
}

#[test]
fn expansion_does_not_bypass_secret_or_generated_path_exclusions() {
    let dir = repo();
    write(dir.path(), ".env", b"needle-secret\n");
    write(dir.path(), "gen/api.ts", b"needle-generated\n");
    write(dir.path(), "src/api.ts", b"needle-visible\nreturn 42;\n");
    commit(dir.path(), "base");
    let snapshot = capture(dir.path(), "HEAD", "HEAD", None).expect("snapshot");
    let context = answer(&snapshot, "needle", &[path("gen/api.ts")]);
    assert_eq!(
        context.span("ref-1").expect("visible").path,
        path("src/api.ts")
    );
    assert!(context.span("ref-2").is_none());
    let shown = context
        .blocks
        .iter()
        .map(|b| b.content.as_str())
        .collect::<String>();
    assert!(!shown.contains("needle-secret"));
    assert!(!shown.contains("needle-generated"));
    assert!(shown.contains("return 42;"));
}
