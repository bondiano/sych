//! Configuration and trusted policy contracts; no external requests.

use std::fs;
use std::path::Path;

use sych_core::config::{
    ConfigError, EffectiveConfig, PolicyChange, PolicyFiles, PolicyInput, RepositoryPolicy,
    ServerConfig, ServerInput, policy_diff,
};
use sych_core::{GitHashAlgorithm, GitObjectId, PolicySource, RepoPath, ReviewLanguage, Timestamp};

mod models;

const ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const SECRET: &str = "sk-or-v1-0123456789abcdef0123456789abcdef";

fn read(path: &str) -> String {
    fs::read_to_string(Path::new(ROOT).join(path)).expect("fixture readable")
}

fn path(p: &str) -> RepoPath {
    p.parse().expect("valid path")
}

fn example_policy() -> PolicyFiles {
    let mut files = PolicyFiles::default();
    for p in [
        ".sych/config.yaml",
        ".sych/rules.yaml",
        ".sych/effects.yaml",
        ".sych/contracts/order-created.md",
    ] {
        files.files.insert(
            path(p),
            read(&format!("examples/personal/{p}")).into_bytes(),
        );
    }
    files
}

fn with(mut files: PolicyFiles, p: &str, text: &str) -> PolicyFiles {
    files.files.insert(path(p), text.as_bytes().to_vec());
    files
}

fn server_errors(fixture: &str) -> Vec<ConfigError> {
    ServerConfig::parse(fixture, &read(fixture)).expect_err("config is rejected")
}

fn repo_errors(files: &PolicyFiles) -> Vec<ConfigError> {
    RepositoryPolicy::parse(files, &ServerConfig::engine_default()).expect_err("policy is rejected")
}

fn effective(files: &PolicyFiles) -> EffectiveConfig {
    let server = read("examples/personal/sych.yaml");
    EffectiveConfig::build(
        &ServerInput {
            origin: "sych.yaml",
            text: Some(&server),
        },
        &PolicyInput {
            origin: "refs/heads/main",
            files,
        },
    )
    .expect("effective config builds")
}

#[test]
fn personal_examples_validate() {
    let config = effective(&example_policy());
    assert_eq!(config.repository.rules.len(), 2);
    assert_eq!(config.repository.contracts.len(), 1);
    assert!(
        !config.server.execution.enabled,
        "execution is off by default"
    );
    assert!(
        config
            .server
            .egress_allowed("https://openrouter.ai/api/v1/chat/completions")
    );
    assert!(
        !config
            .server
            .egress_allowed("https://openrouter.ai.evil.example/api/v1")
    );
    assert!(!config.server.egress_allowed("https://example.com/"));
}

#[test]
fn review_language_is_en_or_ru_and_defaults_to_en() {
    let config = |text: &str| with(PolicyFiles::default(), ".sych/config.yaml", text);
    let parsed = |text: &str| {
        RepositoryPolicy::parse(&config(text), &ServerConfig::engine_default())
            .expect("policy")
            .config
            .review
            .language
    };
    assert_eq!(parsed("schema_version: 1\n"), ReviewLanguage::En);
    assert_eq!(
        parsed("schema_version: 1\nreview:\n  language: ru\n"),
        ReviewLanguage::Ru
    );
    let errors = repo_errors(&config("schema_version: 1\nreview:\n  language: de\n"));
    assert_eq!(errors[0].path, "review.language", "{}", errors[0]);
    assert_eq!(
        effective(&example_policy())
            .repository
            .config
            .review
            .language,
        ReviewLanguage::Ru
    );
}

#[test]
fn unknown_key_names_the_exact_path() {
    let errors = server_errors("fixtures/config/unknown-key.sych.yaml");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path, "models.providers.openrouter.endpont");
    assert!(
        errors[0].message.contains("unknown field `endpont`"),
        "{}",
        errors[0]
    );
}

#[test]
fn secret_values_are_rejected_without_echo() {
    for fixture in [
        "fixtures/config/secret-literal.sych.yaml",
        "fixtures/config/secret-key.sych.yaml",
    ] {
        let errors = server_errors(fixture);
        let shown = errors.iter().map(ToString::to_string).collect::<String>();
        assert!(shown.contains("models.providers.openrouter"), "{shown}");
        assert!(!shown.contains(SECRET), "{shown}");
    }
}

#[test]
fn unsupported_schema_version_is_rejected() {
    let errors = ServerConfig::parse("sych.yaml", "schema_version: 2\nmode: personal\n")
        .expect_err("rejected");
    assert!(errors[0].message.contains("unsupported schema_version 2"));
}

#[test]
fn yaml_enums_are_snake_case() {
    let errors = ServerConfig::parse("sych.yaml", "schema_version: 1\nmode: PERSONAL\n")
        .expect_err("rejected");
    assert_eq!(errors[0].path, "mode");
}

#[test]
fn dangling_model_references_are_errors() {
    let text = read("examples/personal/sych.yaml").replace("reviewer: cheap_go", "reviewer: nope");
    let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
    assert_eq!(errors[0].path, "models.roles.reviewer");
}

#[test]
fn conflicting_mandatory_rules_are_a_configuration_error() {
    let files = with(
        example_policy(),
        ".sych/rules.yaml",
        &read("fixtures/config/conflicting-rules.yaml"),
    );
    let errors = repo_errors(&files);
    assert!(
        errors
            .iter()
            .any(|e| e.path == "rules[1].id" && e.message.contains("conflicts with rules[0]")),
        "{errors:?}"
    );
}

#[test]
fn policy_paths_stay_inside_the_policy_directory() {
    let files = with(
        PolicyFiles::default(),
        ".sych/config.yaml",
        &read("fixtures/config/wrong-path.config.yaml"),
    );
    assert_eq!(repo_errors(&files)[0].path, "policy.rules_file");

    let escape = with(
        PolicyFiles::default(),
        ".sych/config.yaml",
        "schema_version: 1\npolicy:\n  rules_file: ../../etc/passwd\n",
    );
    let errors = repo_errors(&escape);
    assert_eq!(errors[0].path, "policy.rules_file");
    assert!(errors[0].message.contains("`..` segment"));
}

#[test]
fn unknown_files_missing_contracts_and_symlinks_are_errors() {
    let mut files = with(example_policy(), ".sych/extra.yaml", "x: 1\n");
    files
        .files
        .remove(&path(".sych/contracts/order-created.md"));
    files
        .unreadable
        .insert(path(".sych/deploy.yaml"), "symlink is not followed".into());
    let errors = repo_errors(&files);
    let at = |file: &str, p: &str| errors.iter().any(|e| e.file == file && e.path == p);
    assert!(at(".sych/extra.yaml", "."), "{errors:?}");
    assert!(at(".sych/deploy.yaml", "."), "{errors:?}");
    assert!(
        at(".sych/rules.yaml", "rules[0].contract_refs[0]"),
        "{errors:?}"
    );
    assert!(
        at(".sych/effects.yaml", "effects[0].contract_refs[0]"),
        "{errors:?}"
    );
}

#[test]
fn repository_cannot_exceed_server_ceilings() {
    let files = with(
        example_policy(),
        ".sych/config.yaml",
        &read("fixtures/config/weakened.config.yaml"),
    );
    let errors = repo_errors(&files);
    assert_eq!(errors[0].path, "context.max_tokens_per_request");
    assert!(errors[0].message.contains("server ceiling"));
}

#[test]
fn head_policy_is_diffed_but_not_applied() {
    let trusted_files = example_policy();
    let trusted = effective(&trusted_files);
    let head_files = with(
        with(
            trusted_files.clone(),
            ".sych/config.yaml",
            &read("fixtures/config/weakened.config.yaml"),
        ),
        ".sych/rules.yaml",
        &read("fixtures/config/weakened.rules.yaml"),
    );
    let diff = policy_diff(
        &trusted.repository,
        &trusted_files,
        &head_files,
        &trusted.server,
    );
    let changed: Vec<_> = diff
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.change))
        .collect();
    assert_eq!(
        changed,
        [
            (".sych/config.yaml", PolicyChange::Modified),
            (".sych/rules.yaml", PolicyChange::Modified),
        ]
    );
    // The weakened config fails validation, so the head is reported, never merged.
    assert!(diff.head_errors[0].contains("server ceiling"), "{diff:?}");
    assert_eq!(diff.rules.len(), 2, "rule removals stay visible: {diff:?}");
    assert_eq!(effective(&trusted_files), trusted);

    let rules_only = with(
        trusted_files.clone(),
        ".sych/rules.yaml",
        &read("fixtures/config/weakened.rules.yaml"),
    );
    let diff = policy_diff(
        &trusted.repository,
        &trusted_files,
        &rules_only,
        &trusted.server,
    );
    let removed: Vec<_> = diff
        .rules
        .iter()
        .map(|r| (r.id.as_str(), r.change))
        .collect();
    assert_eq!(
        removed,
        [
            ("EFFECT-01", PolicyChange::Removed),
            ("NAMING-01", PolicyChange::Removed)
        ]
    );
    assert!(
        policy_diff(
            &trusted.repository,
            &trusted_files,
            &trusted_files,
            &trusted.server
        )
        .files
        .is_empty()
    );
}

#[test]
fn policy_snapshot_identity_follows_content() {
    let commit = GitObjectId::new(GitHashAlgorithm::Sha1, &"a".repeat(40)).expect("oid");
    let source = PolicySource::TrustedRef {
        ref_name: "refs/heads/main".into(),
        commit,
    };
    let at = Timestamp::UNIX_EPOCH;
    let base = effective(&example_policy());
    let again = effective(&example_policy());
    let snap = |c: &EffectiveConfig| c.policy_snapshot(source.clone(), at).expect("snapshot");
    assert_eq!(snap(&base).id, snap(&again).id);

    let reworded = example_policy();
    let text =
        read("examples/personal/.sych/rules.yaml").replace("owner: backend", "owner: orders");
    let changed = effective(&with(reworded, ".sych/rules.yaml", &text));
    assert_ne!(snap(&base).rules_hash, snap(&changed).rules_hash);
    assert_eq!(snap(&base).config_hash, snap(&changed).config_hash);
    assert_ne!(snap(&base).id, snap(&changed).id);
}

#[test]
fn absent_repository_policy_uses_engine_defaults() {
    let config = EffectiveConfig::build(
        &ServerInput {
            origin: "<engine>",
            text: None,
        },
        &PolicyInput {
            origin: "refs/heads/main",
            files: &PolicyFiles::default(),
        },
    )
    .expect("defaults are valid");
    assert_eq!(config.server, ServerConfig::engine_default());
    assert!(config.repository.rules.is_empty());
    assert_eq!(config.provenance.len(), 1);
}

#[test]
fn dependency_install_needs_a_backend_and_a_pinned_docker_image() {
    let config = |sandbox: &str| {
        format!(
            "schema_version: 1\nmode: personal\ntooling:\n  dependency_install: true\n  sandbox:\n{sandbox}"
        )
    };
    let fields = |text: &str| -> Vec<String> {
        ServerConfig::parse("sych.yaml", text)
            .err()
            .unwrap_or_default()
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert!(fields(&config("    backends: [bwrap]\n")).is_empty());
    let unpinned = fields(&config(
        "    backends: [docker]\n    docker_image: debian:bookworm-slim\n",
    ));
    assert!(
        unpinned
            .iter()
            .any(|e| e.contains("tooling.sandbox.docker_image")),
        "{unpinned:?}"
    );
    let none = fields(&config("    backends: []\n"));
    assert!(
        none.iter().any(|e| e.contains("tooling.sandbox.backends")),
        "{none:?}"
    );
    let off =
        ServerConfig::parse("sych.yaml", "schema_version: 1\nmode: personal\n").expect("valid");
    assert!(
        !off.tooling.dependency_install,
        "installs are off unless enabled"
    );
}

#[test]
fn a_new_meaning_needs_a_new_rule_version() {
    let trusted_files = example_policy();
    let trusted = effective(&trusted_files);
    let rules = read("examples/personal/.sych/rules.yaml");
    let diff = |text: String| {
        policy_diff(
            &trusted.repository,
            &trusted_files,
            &with(trusted_files.clone(), ".sych/rules.yaml", &text),
            &trusted.server,
        )
        .head_errors
    };
    let reworded = rules.replace(
        "keeps the required event publication",
        "may skip the event publication",
    );
    assert!(
        diff(reworded.clone())
            .iter()
            .any(|e| e.contains("`EFFECT-01` changes its meaning without a version bump")),
        "{:?}",
        diff(reworded.clone())
    );
    let bumped = reworded.replacen("    version: 1", "    version: 2", 1);
    assert!(diff(bumped).is_empty());
    let retitled = rules.replace(
        "Preserve the required effects of an operation",
        "Required effects of an order",
    );
    assert!(diff(retitled).is_empty(), "a title is not the meaning");
}

#[test]
fn endpoint_userinfo_is_not_a_loopback_host() {
    let personal = read("examples/personal/sych.yaml").replace(
        "\"https://openrouter.ai/api/v1\"",
        "\"http://localhost:8080@outside.invalid/api/v1\"",
    );
    let errors = ServerConfig::parse("sych.yaml", &personal).expect_err("rejected");
    assert_eq!(errors[0].path, "models.providers.openrouter.endpoint");
    let config = effective(&example_policy()).server;
    assert!(
        !config.egress_allowed("https://openrouter.ai@outside.invalid/api/v1/chat/completions")
    );
    assert!(!config.egress_allowed("http://openrouter.ai/api/v1/chat/completions"));
}

#[test]
fn history_retention_defaults_to_90_days_and_is_never_zero() {
    let personal = read("examples/personal/sych.yaml");
    let config = ServerConfig::parse("sych.yaml", &personal).expect("valid");
    assert_eq!(
        (
            config.storage.retention.model_attempts_days,
            config.storage.retention.change_events_days
        ),
        (90, 90)
    );
    let zero = personal.replacen(
        "storage:\n",
        "storage:\n  retention:\n    change_events_days: 0\n",
        1,
    );
    let errors = ServerConfig::parse("sych.yaml", &zero).expect_err("rejected");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path, "storage.retention.change_events_days");
}

#[test]
fn redaction_additions_are_validated_and_added_to_the_defaults() {
    let personal = read("examples/personal/sych.yaml");
    let with = |patterns: &str| {
        personal
            .replace("  patterns: []", &format!("  patterns: {patterns}"))
            .replace("  token_prefixes: []", "  token_prefixes: [acme_]")
    };
    let bad = ServerConfig::parse("sych.yaml", &with(r"['ok', '(']")).expect_err("rejected");
    assert_eq!(bad[0].path, "redaction.patterns[1]");
    assert!(
        bad[0].message.starts_with("invalid regular expression"),
        "{bad:?}"
    );

    let config = ServerConfig::parse("sych.yaml", &with(r"['ACME-\d{6}']")).expect("valid");
    let redactor = config.redaction.redactor().expect("valid");
    assert_eq!(
        redactor.redact("ACME-123456 acme_0123456789abcdefXY ghp_0123456789abcdefABCDEF"),
        "[REDACTED] [REDACTED] [REDACTED]"
    );
}

#[test]
fn corporate_config_denies_every_external_route() {
    let corporate = read("examples/corporate/sych.yaml");
    ServerConfig::parse("sych.yaml", &corporate).expect("the VPC-only example validates");
    for (provider, path) in [
        (
            "    public:\n      adapter: openrouter\n      endpoint: \"https://openrouter.ai/api/v1\"\n      api_key_env: K\n      allowed_modes: [corporate]\n",
            "models.providers.public.allowed_modes",
        ),
        (
            "    go:\n      adapter: opencode_go\n      endpoint: \"https://opencode.ai/inference/go/openai/v1\"\n      api_key_env: K\n      allowed_modes: [corporate]\n",
            "models.providers.go.allowed_modes",
        ),
    ] {
        let text = corporate.replace("  profiles:\n", &format!("{provider}  profiles:\n"));
        let errors = ServerConfig::parse("sych.yaml", &text).expect_err("rejected");
        assert!(errors.iter().any(|e| e.path == path), "{errors:?}");
    }
    let personal = read("examples/personal/sych.yaml").replace(
        "connection_ref_env: SYCH_CHATGPT_CONNECTION\n      allowed_modes: [personal_local]",
        "connection_ref_env: SYCH_CHATGPT_CONNECTION\n      allowed_modes: [personal_local, personal_service]",
    );
    let errors = ServerConfig::parse("sych.yaml", &personal).expect_err("rejected");
    assert_eq!(
        errors[0].path,
        "models.providers.gpt_subscription.allowed_modes"
    );
}
