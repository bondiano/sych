//! Deployment model and transition matrix: what the owner declared, with a
//! source per claim; states are excluded only by a sourced constraint, and an unknown initial
//! version or build → source link is a named gap, never merge-base.

use sych_core::RepoPath;
use sych_core::config::{PolicyFiles, RepositoryPolicy, ServerConfig};
use sych_core::deploy::{Reachability, TransitionMatrix, TransitionState, matrix};

fn policy(deploy: &str) -> RepositoryPolicy {
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/deploy.yaml".parse::<RepoPath>().expect("path"),
        deploy.as_bytes().to_vec(),
    );
    RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).expect("policy")
}

fn excluded(m: &TransitionMatrix, state: TransitionState) -> Option<&str> {
    m.rows
        .iter()
        .find(|r| r.state == state)
        .and_then(|r| match &r.reachability {
            Reachability::Excluded { source } => Some(source.as_str()),
            Reachability::Reachable => None,
        })
}

/// Production runs an image built from a commit other than the merge-base: that commit is
/// the starting point, and nothing is missing about it.
const PINNED: &str = r#"schema_version: 1
environment: staging
owner: payments
initial_versions:
  value:
    app_image: ghcr.io/acme/orders@sha256:aaaa
    source_commit: "1111111111111111111111111111111111111111"
    schema_phase: expanded
    supported_clients: ["web >= 2026.10.01", "billing-consumer v3"]
  source: argocd app orders-staging, 2026-10-20
order: {value: [schema, app, frontend, jobs], source: deploy/README.md}
mixed_versions: {value: true, source: "rolling update, maxSurge 1"}
compatibility_window: {value: 14d, source: frontend release policy}
recovery: {value: roll_forward_repair, source: runbook RB-12}
excluded_states:
  - {value: backfill_retry, source: ""}
"#;

#[test]
fn a_pinned_model_has_no_initial_version_gap() {
    let p = policy(PINNED);
    let model = p.deployment.as_ref().expect("deploy.yaml");
    let m = matrix(Some(model));
    assert_eq!(m.environment.as_deref(), Some("staging"));
    assert!(m.missing_inputs.is_empty(), "{:?}", m.missing_inputs);
    // An exclusion without a source is not a confirmed constraint.
    assert_eq!(excluded(&m, TransitionState::BackfillRetry), None);
    assert!(
        m.rows
            .iter()
            .all(|r| r.reachability == Reachability::Reachable)
    );
}

#[test]
fn no_model_keeps_every_state_reachable_and_names_the_gaps() {
    let m = matrix(None);
    assert_eq!(m.rows.len(), TransitionState::ALL.len());
    assert!(
        m.rows
            .iter()
            .all(|r| r.reachability == Reachability::Reachable)
    );
    for code in [
        "INITIAL_VERSION_UNKNOWN",
        "ORDER_UNKNOWN",
        "COMPATIBILITY_WINDOW_UNKNOWN",
        "RECOVERY_UNKNOWN",
        "EXTERNAL_CONSUMERS_UNKNOWN",
    ] {
        assert!(
            m.missing_inputs.iter().any(|g| g.starts_with(code)),
            "{code}: {:?}",
            m.missing_inputs
        );
    }
}

#[test]
fn an_image_without_its_source_commit_pins_nothing() {
    let p = policy(
        "schema_version: 1\ninitial_versions:\n  value: {app_image: ghcr.io/acme/orders@sha256:aaaa}\n  source: argocd\n",
    );
    let m = matrix(Some(p.deployment.as_ref().expect("deploy.yaml")));
    assert!(
        m.missing_inputs
            .iter()
            .any(|g| g.starts_with("BUILD_SOURCE_UNKNOWN"))
    );
    // No clients listed: an old open frontend is still assessed, and its contract is a gap.
    assert_eq!(excluded(&m, TransitionState::OldFrontendNewBackend), None);
    assert!(
        m.missing_inputs
            .iter()
            .any(|g| g.starts_with("EXTERNAL_CONSUMERS_UNKNOWN"))
    );
}

#[test]
fn only_a_sourced_constraint_excludes_a_state() {
    let p = policy(
        "schema_version: 1\nmixed_versions: {value: false, source: \"Recreate strategy, one replica\"}\nexcluded_states:\n  - {value: old_frontend_new_backend, source: \"server-rendered, no long-lived tabs\"}\n",
    );
    let m = matrix(Some(p.deployment.as_ref().expect("deploy.yaml")));
    assert_eq!(
        excluded(&m, TransitionState::MixedApps),
        Some("Recreate strategy, one replica")
    );
    assert_eq!(
        excluded(&m, TransitionState::OldFrontendNewBackend),
        Some("server-rendered, no long-lived tabs")
    );
    assert_eq!(excluded(&m, TransitionState::RecoveryAfterNewWrites), None);
}

#[test]
fn unknown_deploy_fields_are_rejected() {
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/deploy.yaml".parse::<RepoPath>().expect("path"),
        b"schema_version: 1\nrollout_magic: true\n".to_vec(),
    );
    assert!(RepositoryPolicy::parse(&files, &ServerConfig::engine_default()).is_err());
}
