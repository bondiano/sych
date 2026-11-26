use std::os::unix::fs::{PermissionsExt, symlink};

use sych_adapters::git::{capture_local_head, delete_local_ref};
use sych_core::SnapshotHeadKind::{Staged, Worktree};

use super::{commit, fs, git, repo, write};

#[test]
fn stages_raw_files_without_changing_the_index_or_running_configured_helpers() {
    let repository = repo();
    let dir = repository.path();
    write(dir, ".gitignore", b"ignored.txt\n");
    write(dir, ".gitattributes", b"*.txt filter=lfs\n");
    write(dir, "file.txt", b"base\n");
    write(dir, "delete.txt", b"delete me\n");
    commit(dir, "base");
    write(dir, "file.txt", b"staged\n");
    git(dir, &["add", "file.txt"]);
    write(dir, "file.txt", b"worktree\r\n");
    write(dir, "new name.txt", b"new\n");
    write(dir, "ignored.txt", b"ignored\n");
    fs::remove_file(dir.join("delete.txt")).expect("delete");
    fs::set_permissions(dir.join("file.txt"), fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let hook = b"#!/bin/sh\ntouch hook-ran\nexit 42\n";
    for name in ["pre-commit", "reference-transaction"] {
        let path = dir.join(".git/hooks").join(name);
        fs::write(&path, hook).expect("hook");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make executable");
    }
    git(
        dir,
        &["config", "filter.lfs.process", "touch helper-ran; exit 42"],
    );
    git(
        dir,
        &["config", "filter.lfs.clean", "touch helper-ran; exit 42"],
    );
    git(dir, &["config", "filter.lfs.required", "true"]);
    let index = fs::read(dir.join(".git/index")).expect("index");
    let refs = git(dir, &["show-ref", "--heads"]);
    let head = git(dir, &["rev-parse", "HEAD"]);
    let staged = capture_local_head(dir, Staged).expect("staged");
    let mut worktree = capture_local_head(dir, Worktree).expect("worktree");
    let staged_oid = staged.commit().hex();
    let worktree_oid = worktree.commit().hex();
    assert_ne!(staged_oid, worktree_oid);
    let staged_files = git(dir, &["diff", "--name-only", "HEAD", staged_oid]);
    assert_eq!(staged_files, b"file.txt\n");
    let worktree_files = git(dir, &["diff", "--name-only", "HEAD", worktree_oid]);
    assert_eq!(worktree_files, b"delete.txt\nfile.txt\nnew name.txt\n");
    assert_eq!(
        git(dir, &["show", &format!("{staged_oid}:file.txt")]),
        b"staged\n"
    );
    assert_eq!(
        git(dir, &["show", &format!("{worktree_oid}:file.txt")]),
        b"worktree\r\n"
    );
    assert!(git(dir, &["ls-tree", worktree_oid, "file.txt"]).starts_with(b"100755 "));
    assert_eq!(git(dir, &["rev-parse", &format!("{worktree_oid}^")]), head);
    assert_eq!(fs::read(dir.join(".git/index")).expect("index"), index);
    assert_eq!(git(dir, &["show-ref", "--heads"]), refs);
    assert_eq!(git(dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        fs::read(dir.join("file.txt")).expect("worktree"),
        b"worktree\r\n"
    );
    assert!(!dir.join("hook-ran").exists());
    assert!(!dir.join("helper-ran").exists());

    let staged_pin = staged.commit().clone();
    drop(staged);
    assert_eq!(
        git(
            dir,
            &["for-each-ref", "refs/sych/local", "--format=%(objectname)"]
        ),
        format!("{worktree_oid}\n").as_bytes()
    );
    delete_local_ref(worktree.repository(), &staged_pin).expect("already missing succeeds");
    worktree.keep();
    let durable_pin = worktree.commit().clone();
    drop(worktree);
    git(dir, &["gc", "--prune=now"]);
    assert_eq!(
        git(dir, &["show", &format!("{}:file.txt", durable_pin.hex())]),
        b"worktree\r\n"
    );
}

#[test]
fn worktree_uses_the_index_mode_when_git_ignores_file_permissions() {
    let repository = repo();
    let dir = repository.path();
    write(dir, "tracked.sh", b"old\n");
    commit(dir, "base");
    git(dir, &["update-index", "--chmod=+x", "tracked.sh"]);
    git(dir, &["config", "core.filemode", "false"]);

    write(dir, "tracked.sh", b"edited\n");
    write(dir, "new.sh", b"new\n");
    fs::set_permissions(dir.join("tracked.sh"), fs::Permissions::from_mode(0o644))
        .expect("clear the worktree executable bit");
    fs::set_permissions(dir.join("new.sh"), fs::Permissions::from_mode(0o755))
        .expect("set the worktree executable bit");

    let snapshot = capture_local_head(dir, Worktree).expect("worktree snapshot");
    let oid = snapshot.commit().hex();
    assert!(git(dir, &["ls-tree", oid, "tracked.sh"]).starts_with(b"100755 "));
    assert!(git(dir, &["ls-tree", oid, "new.sh"]).starts_with(b"100644 "));
    assert_eq!(
        git(dir, &["show", &format!("{oid}:tracked.sh")]),
        b"edited\n"
    );
}

#[test]
fn worktree_keeps_a_git_symlink_placeholder_as_a_link() {
    let repository = repo();
    let dir = repository.path();
    symlink("old-target", dir.join("link")).expect("create symlink");
    commit(dir, "base");
    git(dir, &["config", "core.symlinks", "false"]);
    fs::remove_file(dir.join("link")).expect("remove symlink");
    git(dir, &["checkout", "--", "link"]);
    assert!(
        fs::symlink_metadata(dir.join("link"))
            .expect("placeholder")
            .is_file()
    );
    write(dir, "link", b"changed-target");

    let snapshot = capture_local_head(dir, Worktree).expect("worktree snapshot");
    let oid = snapshot.commit().hex();
    assert!(git(dir, &["ls-tree", oid, "link"]).starts_with(b"120000 "));
    assert_eq!(
        git(dir, &["show", &format!("{oid}:link")]),
        b"changed-target"
    );
}

#[test]
fn captures_linked_worktree_paths_and_symlinks_as_links() {
    let repository = repo();
    let dir = repository.path();
    write(dir, "nested/a.txt", b"base\n");
    commit(dir, "base");
    let separate = tempfile::tempdir().expect("different parent");
    let linked = separate.path().join("linked");
    git(
        dir,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().expect("path"),
            "HEAD",
        ],
    );
    fs::remove_dir_all(linked.join("nested")).expect("remove tracked directory");
    let target = separate.path().join("target");
    write(&target, "a.txt", b"outside\n");
    symlink(&target, linked.join("nested")).expect("replacement symlink");
    let captured = capture_local_head(&linked, Worktree).expect("linked worktree");
    let oid = captured.commit().hex();
    assert!(git(&linked, &["ls-tree", oid, "nested"]).starts_with(b"120000 "));
    assert_eq!(
        git(&linked, &["show", &format!("{oid}:nested")]),
        target.as_os_str().as_encoded_bytes()
    );
    assert_eq!(
        fs::read(linked.join("nested/a.txt")).expect("outside unchanged"),
        b"outside\n"
    );
}

#[test]
fn captures_a_submodule_commit_without_reading_its_local_files() {
    let repository = repo();
    let dir = repository.path();
    let child = repo();
    write(child.path(), "note.txt", b"base\n");
    commit(child.path(), "base");
    git(
        dir,
        &[
            "submodule",
            "add",
            "--quiet",
            child.path().to_str().expect("path"),
            "sub",
        ],
    );
    commit(dir, "submodule");
    let sub = dir.join("sub");
    write(&sub, "note.txt", b"next\n");
    commit(&sub, "next");
    let head = git(&sub, &["rev-parse", "HEAD"]);
    write(&sub, "note.txt", b"not committed\n");
    let captured = capture_local_head(dir, Worktree).expect("submodule commit");
    assert_eq!(
        git(
            dir,
            &["rev-parse", &format!("{}:sub", captured.commit().hex())]
        ),
        head
    );
}

#[test]
fn rejects_untracked_nested_repositories_instead_of_silently_omitting_them() {
    let repository = repo();
    let dir = repository.path();
    commit(dir, "base");
    let nested = dir.join("nested");
    fs::create_dir(&nested).expect("directory");
    git(&nested, &["init", "-q", "-b", "main"]);
    write(&nested, "note.txt", b"nested\n");
    commit(&nested, "base");
    let Err(error) = capture_local_head(dir, Worktree) else {
        panic!("nested repositories need explicit staging");
    };
    assert!(
        error
            .to_string()
            .contains("untracked nested Git repository")
    );
}
