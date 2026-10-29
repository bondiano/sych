//! The outgoing prompt on a real snapshot, syntax pass and broker: a secret in a changed file's
//! name and content never leaves in a DATA header or body, a context cut by budget says so, and a
//! risky shape the syntax pass found names itself in its span's label.

use std::cell::RefCell;
use std::fs;

use sych_core::config::Budgets;
use sych_core::model::{ModelReply, ModelRequest, ProviderError};

mod harness;

use harness::*;

const CANARY: &str = "ghp_0123456789abcdefABCDEF";

thread_local! {
    /// Every message of the requests a scripted provider saw.
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn recording(request: &ModelRequest) -> Result<ModelReply, ProviderError> {
    SEEN.with(|s| {
        s.borrow_mut()
            .extend(request.messages.iter().map(|m| m.content.clone()));
    });
    supported_and_empty(request)
}

#[tokio::test(flavor = "current_thread")]
async fn a_secret_in_a_changed_path_and_its_code_never_reaches_the_model() {
    let dir = repo(false);
    let d = dir.path();
    fs::write(
        d.join(format!("src/{CANARY}.ts")),
        format!("export const token = \"{CANARY}\";\n"),
    )
    .expect("file");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "add a token"]);
    run_on(dir, recording, Budgets::default(), 14_000, true).await;
    let seen = SEEN.with(|s| s.borrow().join("\n"));
    assert!(!seen.is_empty(), "no request was sent");
    assert!(!seen.contains(CANARY), "{seen}");
    assert!(seen.contains("[secret values redacted]>>>"), "{seen}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_context_cut_by_budget_names_its_gaps_in_the_prompt() {
    let config =
        "schema_version: 1\ncontext:\n  initial_tokens_per_unit: 8\n  max_tokens_per_request: 8\n";
    let (outcome, _) = run_on(
        repo_with(false, &[(".sych/config.yaml", config)]),
        recording,
        Budgets::default(),
        1,
        true,
    )
    .await;
    assert!(
        outcome.record.passes.iter().any(|p| !p.packets.is_empty()),
        "no pass was sent"
    );
    let seen = SEEN.with(|s| s.borrow().join("\n"));
    let gaps = seen
        .split("<<<DATA ")
        .find(|block| block.contains(" known gaps of this context"));
    assert!(gaps.is_some_and(|g| !g.trim().is_empty()), "{seen}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_risky_shape_the_syntax_pass_found_labels_its_span_in_the_prompt() {
    let dir = repo(false);
    let d = dir.path();
    fs::write(
        d.join("src/Poll.tsx"),
        "import { useEffect } from \"react\";\n\nexport function Poll({ tick }) {\n  useEffect(() => {\n    setInterval(tick, 1000);\n  }, [tick]);\n  return null;\n}\n",
    )
    .expect("file");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "poll"]);
    run_on(dir, recording, Budgets::default(), 14_000, true).await;
    let seen = SEEN.with(|s| s.borrow().join("\n"));
    let label = seen
        .lines()
        .find(|l| l.starts_with("<<<DATA ") && l.contains("src/Poll.tsx"))
        .unwrap_or_default();
    // The span is also a diff hunk, so the label leads with `required:`; the reason is kept.
    assert!(
        label.contains("syntax unpaired_timer: useEffect: setInterval without clearInterval"),
        "{seen}"
    );
}
