//! Validation of `RuleKind::Preference` severity bounds and `review.block_on` entries.

use sych_core::config::{PolicyFiles, RepositoryPolicy, ServerConfig};

use super::{example_policy, repo_errors, with};

#[test]
fn preference_rules_cannot_exceed_low_or_be_named_literally_in_block_on() {
    let literal_block = with(
        example_policy(),
        ".sych/config.yaml",
        "schema_version: 1\nreview:\n  block_on: [\"EFFECT-01\", \"NAMING-01\"]\n",
    );
    let errors = repo_errors(&literal_block);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].file, ".sych/config.yaml");
    assert_eq!(errors[0].path, "review.block_on[1]");
    assert!(
        errors[0]
            .message
            .contains("`NAMING-01` is a preference rule and cannot block a review"),
        "{}",
        errors[0]
    );

    let glob_block = with(
        example_policy(),
        ".sych/config.yaml",
        "schema_version: 1\nreview:\n  block_on: [\"NAMING-*\"]\n",
    );
    RepositoryPolicy::parse(&glob_block, &ServerConfig::engine_default())
        .expect("a glob in block_on remains valid");

    let high_pref = with(
        PolicyFiles::default(),
        ".sych/rules.yaml",
        "schema_version: 1\nrules:\n  - id: PREF-01\n    version: 1\n    title: Naming\n    kind: preference\n    required: false\n    severity_on_violation: high\n    owner: backend\n    statement: Use domain names.\n",
    );
    let sev_errors = repo_errors(&high_pref);
    assert_eq!(sev_errors.len(), 1, "{sev_errors:?}");
    assert_eq!(sev_errors[0].path, "rules[0].severity_on_violation");
    assert!(sev_errors[0].message.contains("at most low"));

    let low_pref = with(
        PolicyFiles::default(),
        ".sych/rules.yaml",
        "schema_version: 1\nrules:\n  - id: PREF-01\n    version: 1\n    title: Naming\n    kind: preference\n    required: false\n    severity_on_violation: low\n    owner: backend\n    statement: Use domain names.\n",
    );
    RepositoryPolicy::parse(&low_pref, &ServerConfig::engine_default())
        .expect("a low preference validates");
}
