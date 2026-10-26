//! Shapes `facts` finds in Rust sources.

use super::*;

fn kinds(source: &str) -> Vec<(SyntaxKind, u32)> {
    facts(source, &[])
        .expect("parses")
        .into_iter()
        .map(|r| (r.kind, r.start))
        .collect()
}

#[test]
fn guard_held_across_await_is_found_and_drop_releases_it() {
    let held = "async fn f(m: &std::sync::Mutex<u8>) {\n    let g = m.lock().unwrap();\n    io().await;\n}\n";
    assert!(kinds(held).contains(&(SyntaxKind::GuardAcrossAwait, 3)));
    let dropped = "async fn f(m: &std::sync::Mutex<u8>) {\n    let g = m.lock().unwrap();\n    drop(g);\n    io().await;\n}\n";
    assert!(
        !kinds(dropped)
            .iter()
            .any(|(k, _)| *k == SyntaxKind::GuardAcrossAwait)
    );
    let sync_fn =
        "fn f(m: &std::sync::Mutex<u8>) {\n    let g = m.lock().unwrap();\n    later();\n}\n";
    assert!(
        !kinds(sync_fn)
            .iter()
            .any(|(k, _)| *k == SyntaxKind::GuardAcrossAwait)
    );
}

#[test]
fn discarded_results_detached_tasks_and_blocking_calls() {
    let source = "pub async fn run() {\n    let _ = save();\n    tokio::spawn(work());\n    std::thread::sleep(d);\n    let h = tokio::spawn(work());\n    tokio::select! { _ = a => {} }\n}\n";
    let found = kinds(source);
    for expected in [
        (SyntaxKind::AsyncFn, 1),
        (SyntaxKind::PublicItem, 1),
        (SyntaxKind::DiscardedResult, 2),
        (SyntaxKind::DetachedSpawn, 3),
        (SyntaxKind::BlockingInAsync, 4),
        (SyntaxKind::SelectMacro, 6),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
    assert!(
        !found.contains(&(SyntaxKind::DetachedSpawn, 5)),
        "a kept handle is not detached"
    );
}

#[test]
fn cfg_serde_drop_and_test_modules() {
    let source = "#[derive(Serialize)]\n#[serde(rename_all = \"snake_case\")]\npub struct A { #[cfg(feature = \"x\")] f: u8 }\nimpl Drop for A { fn drop(&mut self) {} }\n#[cfg(test)]\nmod tests {}\n";
    let found = kinds(source);
    for expected in [
        (SyntaxKind::SerdeAttribute, 2),
        (SyntaxKind::CfgGate, 3),
        (SyntaxKind::DropImpl, 4),
        (SyntaxKind::TestModule, 5),
    ] {
        assert!(found.contains(&expected), "{expected:?} in {found:?}");
    }
    let gated = "fn f() {\n    #[cfg(feature = \"m\")]\n    record();\n}\n";
    assert!(kinds(gated).contains(&(SyntaxKind::CfgGate, 2)));
    let weakened = "#[test]\n#[ignore]\n#[allow(clippy::unwrap_used)]\nfn t() {}\n";
    let found = kinds(weakened);
    assert!(
        found.contains(&(SyntaxKind::TestSkipped, 2))
            && found.contains(&(SyntaxKind::LintSuppressed, 3))
    );
    assert!(facts("fn broken( {", &[]).is_err());
}

#[test]
fn code_under_a_cfg_the_analysed_build_skips_is_inactive() {
    let source = "#[cfg(windows)]\npub fn dir() -> PathBuf {\n    known()\n}\n\n#[cfg(not(windows))]\npub fn dir() -> PathBuf {\n    xdg()\n}\n\nfn f() {\n    #[cfg(feature = \"metrics\")]\n    record();\n}\n";
    let inactive = |features: &[String]| -> Vec<(u32, u32)> {
        facts(source, features)
            .expect("parses")
            .into_iter()
            .filter(|r| r.kind == SyntaxKind::InactiveCfg)
            .map(|r| (r.start, r.end))
            .collect()
    };
    assert_eq!(inactive(&[]), [(1, 4), (12, 13)]);
    assert_eq!(
        inactive(&["metrics".into()]),
        [(1, 4)],
        "a default feature is analysed"
    );
}

#[test]
fn a_multi_line_use_is_one_import() {
    let found = facts("use std::{\n    io,\n    fs,\n};\nuse crate::Tx;\n", &[]).expect("parses");
    let imports: Vec<_> = found
        .iter()
        .filter(|r| r.kind == SyntaxKind::Import)
        .map(|r| (r.start, r.end))
        .collect();
    assert_eq!(imports, [(1, 4), (5, 5)]);
}
