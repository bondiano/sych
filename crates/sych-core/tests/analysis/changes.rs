//! The changed files a prompt names: hunks of the shown ones, and the unshown files whose
//! existence the change alters.

use sych_core::analysis::{Answered, AskOperation, Context, ContextAsk};
use sych_core::broker::{OpStatus, Ops};
use sych_core::{ChangeStatus, DiffManifest, Hunk, SourceSide};

use super::{Reader, context, path};

#[test]
fn shown_changed_files_list_their_hunks_and_unshown_added_or_removed_files_are_named() {
    let reader = Reader::new();
    let (mut ctx, _) = context(&reader);
    let mut manifest: DiffManifest = serde_json::from_str(include_str!(
        "../../../../fixtures/publication/diff-manifest.json"
    ))
    .expect("manifest");
    // An unshown modified file is named too, so a model can ask read_diff for it.
    let mut modified = manifest.files[0].clone();
    for side in modified.old.iter_mut().chain(modified.new.iter_mut()) {
        side.path = path("src/other.ts");
    }
    let mut added = modified.clone();
    added.status = ChangeStatus::Added;
    added.old = None;
    added.new.as_mut().expect("head side").path = path("src/pages/new.vue");
    manifest.files.extend([modified, added]);
    let file = &mut manifest.files[0];
    for side in file.old.iter_mut().chain(file.new.iter_mut()) {
        side.path = path("src/order.ts");
    }
    ctx.add_changes(&manifest);
    let block = |prefix: &str| {
        ctx.blocks
            .iter()
            .find(|b| b.label.starts_with(prefix))
            .unwrap_or_else(|| panic!("no block {prefix}"))
            .content
            .as_str()
    };
    assert_eq!(
        block("every hunk"),
        "src/order.ts Modified: 5 lines removed, 7 added; -41,5 +41,7\n"
    );
    let named = block("other files this change");
    assert!(
        named.contains("src/old_name.rs -> src/новое_имя.rs Renamed"),
        "{named}"
    );
    assert!(named.contains("src/gone.rs Deleted"), "{named}");
    assert!(named.contains("src/pages/new.vue Added"), "{named}");
    assert!(named.contains("src/other.ts Modified"), "{named}");
    assert!(!named.contains(" +"), "hunks only for shown files: {named}");
}

/// The fixture manifest with its first file at `src/order.ts`: one hunk turning BASE lines
/// 2-5 (of 6) into HEAD line 2 (of 3), and the generated `src/gen.ts`.
fn order_change() -> DiffManifest {
    let mut manifest: DiffManifest = serde_json::from_str(include_str!(
        "../../../../fixtures/publication/diff-manifest.json"
    ))
    .expect("manifest");
    let mut generated = manifest.files[0].clone();
    let file = &mut manifest.files[0];
    for (side, lines) in [(file.old.as_mut(), 6), (file.new.as_mut(), 3)] {
        let side = side.expect("side");
        side.path = path("src/order.ts");
        side.line_count = Some(lines);
    }
    file.hunks = vec![Hunk {
        old_start: 2,
        old_lines: 4,
        new_start: 2,
        new_lines: 1,
    }];
    for side in generated.old.iter_mut().chain(generated.new.iter_mut()) {
        side.path = path("src/gen.ts");
    }
    generated.generated = true;
    manifest.files.push(generated);
    manifest
}

fn read_diff(ctx: &mut Context, reader: &Reader, p: &str) -> Answered {
    let ops = Ops {
        generated: &[],
        facts: &[],
        evidence: &[],
        reader,
    };
    ContextAsk {
        question: format!("what changed in {p}?"),
        operation: AskOperation::ReadDiff { path: path(p) },
    }
    .answer(&ops, ctx, 3)
}

#[test]
fn read_diff_shows_each_hunk_side_with_its_neighbouring_lines() {
    let reader = Reader::new();
    let mut ctx = Context::default();
    ctx.add_changes(&order_change());
    let answered = read_diff(&mut ctx, &reader, "src/order.ts");
    assert_eq!(answered.status, Some(OpStatus::CompleteForQuery));
    assert_eq!(answered.refs, 2);
    let head = ctx.span("ref-1").expect("head window");
    assert_eq!(
        (head.side, head.start_line, head.end_line),
        (SourceSide::Head, 1, 3)
    );
    let base = ctx.span("ref-2").expect("base window");
    assert_eq!(
        (base.side, base.start_line, base.end_line),
        (SourceSide::DiffBase, 1, 6)
    );
    let block = ctx.blocks.last().expect("answer block");
    assert!(block.content.contains("-2,4 +2,1"), "{}", block.content);
}

#[test]
fn read_diff_never_shows_a_generated_or_unchanged_file_as_a_diff() {
    let reader = Reader::new();
    let mut ctx = Context::default();
    ctx.add_changes(&order_change());
    let generated = read_diff(&mut ctx, &reader, "src/gen.ts");
    assert_eq!(
        (generated.refs, generated.status),
        (0, Some(OpStatus::Partial))
    );
    let unchanged = read_diff(&mut ctx, &reader, "src/order.test.ts");
    assert_eq!(unchanged.refs, 0);
    let block = ctx.blocks.last().expect("answer block");
    assert!(
        block.content.contains("is not changed"),
        "{}",
        block.content
    );
    // Without the change's list there is nothing to say either way.
    let mut bare = Context::default();
    let answered = read_diff(&mut bare, &reader, "src/order.ts");
    assert_eq!(answered.status, Some(OpStatus::Unsupported));
}
