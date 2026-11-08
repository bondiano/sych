//! The changed files a prompt names: hunks of the shown ones, and the unshown files whose
//! existence the change alters.

use sych_core::{ChangeStatus, DiffManifest};

use super::{Reader, context, path};

#[test]
fn shown_changed_files_list_their_hunks_and_unshown_added_or_removed_files_are_named() {
    let reader = Reader::new();
    let (mut ctx, _) = context(&reader);
    let mut manifest: DiffManifest = serde_json::from_str(include_str!(
        "../../../../fixtures/publication/diff-manifest.json"
    ))
    .expect("manifest");
    // An unshown modified file: its existence does not change, so it is not named.
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
    assert!(
        !named.contains("other.ts") && !named.contains(" +"),
        "{named}"
    );
}
