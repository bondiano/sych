//! `TEST-INTEGRITY` on a real snapshot: the removed lines of a changed test reach the model.

use std::cell::RefCell;
use std::fs;

use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError, Speaker};

mod harness;

use harness::*;

thread_local! {
    /// User messages of the requests a scripted provider saw.
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn recording(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    let user = request.messages.iter().find(|m| m.speaker == Speaker::User);
    SEEN.with(|s| s.borrow_mut().extend(user.map(|m| m.content.clone())));
    supported_and_empty(request)
}

#[tokio::test(flavor = "current_thread")]
async fn test_integrity_sees_a_removed_assertion() {
    let dir = repo(false);
    let d = dir.path();
    let test = d.join("src/order.test.ts");
    fs::write(
        &test,
        "it('totals', () => {\n  expect(total([])).toBe(0);\n  expect(total()).toBe(0);\n});\n",
    )
    .expect("test");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "test"]);
    fs::write(
        &test,
        "it('totals', () => {\n  expect(total([])).toBe(0);\n});\n",
    )
    .expect("test");
    git(d, &["commit", "-q", "-am", "drop a check"]);
    // The base is the commit with both checks; HEAD~1 of the harness.
    run_on(dir, recording, Budgets::default(), 14_000, true).await;
    let seen = SEEN.with(|s| s.borrow().join("\n"));
    assert!(
        seen.contains("src/order.test.ts: 1 lines removed\n-   expect(total()).toBe(0);"),
        "{seen}"
    );
}
