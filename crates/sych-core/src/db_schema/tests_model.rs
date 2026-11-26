use std::collections::BTreeMap;
use std::slice;

use super::rules::{
    ENUM_NOT_ENFORCED, FK_WITHOUT_INDEX, MAX_LENGTH_NOT_ENFORCED, RELATION_WITHOUT_FOREIGN_KEY,
    REQUIRED_FIELD_NULLABLE_COLUMN, TABLE_WITHOUT_PRIMARY_KEY, UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX,
};
use super::tests::{
    bigint_type, col, fk, index, snap, table, table_with_pk, text_type, varchar_type,
};
use super::*;
use crate::config::{
    AppliesTo, PolicyFiles, RepositoryPolicy, Rule, RuleException, RuleKind, RuleSeverity,
    ServerConfig,
};
use crate::{
    BackendStatus, FindingCategory, Producer, RepoPath, ReviewLanguage, SchemaVersion, Severity,
    SourceSnapshotId, Timestamp, sha256,
};

fn model_table(name: &str) -> DbModelTable {
    DbModelTable {
        schema: None,
        name: name.into(),
        source: None,
        columns: Vec::new(),
        primary_key: Vec::new(),
        join_table_for: Vec::new(),
        uniques: Vec::new(),
        relations: Vec::new(),
    }
}

fn model_rules_snapshot() -> DbSchemaSnapshot {
    let mut users = table_with_pk(
        "users",
        vec![
            col("id", bigint_type(), false),
            col("tenant_id", bigint_type(), false),
            col("email", varchar_type(300), true),
            col("status", text_type(), false),
            col("role", text_type(), false),
        ],
        &["id"],
    );
    let mut part_uq = index("uq_email_partial", &["email"], true);
    part_uq.predicate = Some("deleted_at IS NULL".into());
    users.indexes.push(part_uq);
    users.checks.push(DbCheck {
        name: Some("chk_role".into()),
        expression: "((role)::text = ANY ((ARRAY['admin'::character varying, 'member'::character varying])::text[]))".into(),
    });
    users.checks.push(DbCheck {
        name: Some("chk_status".into()),
        expression: "status IN ('active')".into(),
    });

    let mut orders = table_with_pk(
        "orders",
        vec![
            col("id", bigint_type(), false),
            col("tenant_id", bigint_type(), false),
            col("nullable_tenant_id", bigint_type(), true),
            col("user_id", bigint_type(), false),
            col("reviewer_id", bigint_type(), false),
            col("owner_id", bigint_type(), false),
        ],
        &["id"],
    );
    orders.indexes.push(index(
        "idx_orders_tenant_user",
        &["tenant_id", "user_id"],
        false,
    ));
    orders.indexes.push(index(
        "idx_orders_nt_rev",
        &["nullable_tenant_id", "reviewer_id"],
        false,
    ));
    orders.foreign_keys.push(fk(
        "fk_orders_user",
        &["tenant_id", "user_id"],
        Some("public"),
        "users",
        &["tenant_id", "id"],
    ));
    orders.foreign_keys.push(fk(
        "fk_orders_reviewer",
        &["nullable_tenant_id", "reviewer_id"],
        Some("public"),
        "users",
        &["tenant_id", "id"],
    ));

    let mut user_tags = table_with_pk(
        "user_tags",
        vec![
            col("user_id", bigint_type(), false),
            col("tag_id", bigint_type(), false),
        ],
        &["user_id", "tag_id"],
    );
    user_tags.foreign_keys.push(fk(
        "fk_ut_user",
        &["user_id"],
        Some("public"),
        "users",
        &["id"],
    ));
    snap(
        SqlDialect::Postgres,
        "public",
        vec![users, orders, user_tags],
    )
}

fn model_rules_manifest() -> DbModelManifest {
    let mut mt_users = model_table("users");
    mt_users.columns = vec![
        DbModelColumn {
            name: "email".into(),
            required: true,
            unique: true,
            max_length: Some(255),
            enum_values: Vec::new(),
            source: None,
        },
        DbModelColumn {
            name: "status".into(),
            required: true,
            unique: false,
            max_length: None,
            enum_values: vec!["active".into(), "disabled".into()],
            source: None,
        },
        DbModelColumn {
            name: "role".into(),
            required: true,
            unique: false,
            max_length: None,
            enum_values: vec!["admin".into(), "member".into()],
            source: None,
        },
    ];
    let mut mt_orders = model_table("orders");
    mt_orders.relations = ["user_id", "reviewer_id", "owner_id"]
        .into_iter()
        .map(|c| DbModelRelation {
            columns: vec![c.into()],
            references: DbModelTableRef {
                schema: None,
                table: "users".into(),
            },
            ref_columns: vec!["id".into()],
        })
        .collect();
    let mut mt_join = model_table("user_tags");
    mt_join.join_table_for = vec![
        DbModelTableRef {
            schema: None,
            table: "users".into(),
        },
        DbModelTableRef {
            schema: None,
            table: "tags".into(),
        },
    ];
    DbModelManifest {
        schema_version: SchemaVersion,
        extractor: Some("test".into()),
        tables: vec![mt_users, mt_orders, mt_join, model_table("ghost_table")],
        limitations: Vec::new(),
    }
}

#[test]
fn model_rules_cover_required_unique_relations_tenant_fk_m2m_max_length_and_enum() {
    let s = model_rules_snapshot();
    let manifest = model_rules_manifest();
    let report = evaluate(&s, Some(&manifest), &DbSchemaConfig::default());
    let rule_objs: Vec<(&str, String)> = report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.object.format_exclusion()))
        .collect();

    assert!(rule_objs.contains(&(
        REQUIRED_FIELD_NULLABLE_COLUMN,
        "public.users.column:email".into()
    )));
    assert!(rule_objs.contains(&(
        UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX,
        "public.users.column:email".into()
    )));
    assert!(rule_objs.contains(&(MAX_LENGTH_NOT_ENFORCED, "public.users.column:email".into())));
    assert!(rule_objs.contains(&(ENUM_NOT_ENFORCED, "public.users.column:status".into())));
    assert!(!rule_objs.contains(&(ENUM_NOT_ENFORCED, "public.users.column:role".into())));
    assert!(!rule_objs.contains(&(
        RELATION_WITHOUT_FOREIGN_KEY,
        "public.orders.column:user_id".into()
    )));
    assert!(rule_objs.contains(&(
        RELATION_WITHOUT_FOREIGN_KEY,
        "public.orders.column:reviewer_id".into()
    )));
    assert!(rule_objs.contains(&(
        RELATION_WITHOUT_FOREIGN_KEY,
        "public.orders.column:owner_id".into()
    )));
    assert!(rule_objs.contains(&(RELATION_WITHOUT_FOREIGN_KEY, "public.user_tags".into())));
    assert!(report.warnings.iter().any(|w| w.contains("ghost_table")));
}

#[test]
fn sqlite_max_length_requires_check_constraint_not_varchar() {
    let mut t = table_with_pk(
        "notes",
        vec![
            col("id", bigint_type(), false),
            col("title", varchar_type(50), false),
            col("body", varchar_type(200), false),
        ],
        &["id"],
    );
    t.checks.push(DbCheck {
        name: Some("chk_body".into()),
        expression: "length(body) <= 200".into(),
    });
    let s = snap(SqlDialect::Sqlite, "main", vec![t]);
    let manifest = DbModelManifest {
        schema_version: SchemaVersion,
        extractor: Some("test".into()),
        tables: vec![DbModelTable {
            schema: None,
            name: "notes".into(),
            source: None,
            columns: vec![
                DbModelColumn {
                    name: "title".into(),
                    required: true,
                    unique: false,
                    max_length: Some(50),
                    enum_values: Vec::new(),
                    source: None,
                },
                DbModelColumn {
                    name: "body".into(),
                    required: true,
                    unique: false,
                    max_length: Some(200),
                    enum_values: Vec::new(),
                    source: None,
                },
            ],
            primary_key: Vec::new(),
            join_table_for: Vec::new(),
            uniques: Vec::new(),
            relations: Vec::new(),
        }],
        limitations: Vec::new(),
    };
    let report = evaluate(&s, Some(&manifest), &DbSchemaConfig::default());
    let cols: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule == MAX_LENGTH_NOT_ENFORCED)
        .filter_map(|f| f.object.column.as_deref())
        .collect();
    assert_eq!(cols, vec!["title"]);
}

#[test]
fn manifest_parser_accepts_version_alias_and_rejects_invalid_documents() {
    let ok = r#"{"version": 1, "extractor": "custom", "tables": [{"name": "users", "columns": [{"name": "id"}]}]}"#;
    let parsed = DbModelManifest::parse(ok).expect("valid manifest");
    assert_eq!(parsed.tables.len(), 1);

    let bad_ver = r#"{"schema_version": 2, "tables": []}"#;
    assert!(DbModelManifest::parse(bad_ver).is_err());

    let bad_join =
        r#"{"schema_version": 1, "tables": [{"name": "j", "join_table_for": [{"table": "a"}]}]}"#;
    assert!(DbModelManifest::parse(bad_join).is_err());

    let bad_enum = r#"{"schema_version": 1, "tables": [{"name": "u", "columns": [{"name": "s", "enum": ["a", "a"]}]}]}"#;
    assert!(DbModelManifest::parse(bad_enum).is_err());

    let bad_max_len = r#"{"schema_version": 1, "tables": [{"name": "u", "columns": [{"name": "s", "max_length": 0}]}]}"#;
    assert!(DbModelManifest::parse(bad_max_len).is_err());
}

#[test]
fn config_validation_exclusions_and_idempotent_baseline() {
    let invalid = "rules:\n  not-a-rule:\n    enabled: true\nexclusions:\n  - rule: fk-without-index\n    object: bad_object\n    reason: ''\n";
    let errs = DbSchemaConfig::parse_yaml("test.yaml", invalid).expect_err("must reject");
    assert!(errs.len() >= 3, "{errs:?}");

    let t = table("events", vec![col("payload", text_type(), false)]);
    let s = snap(SqlDialect::Postgres, "public", vec![t]);
    let report = evaluate(&s, None, &DbSchemaConfig::default());
    assert_eq!(report.findings.len(), 1);

    let header = "# Keep my comment\nrules:\n  nullable-boolean:\n    enabled: true\n";
    let (updated, added) =
        add_baseline("baseline.yaml", header, report.findings.clone()).expect("baseline");
    assert_eq!(added, 1);
    assert!(updated.contains("# Keep my comment"));
    assert!(updated.contains(BASELINE_REASON));

    let (again, added_again) =
        add_baseline("baseline.yaml", &updated, report.findings).expect("baseline 2");
    assert_eq!(added_again, 0);
    assert_eq!(again, updated);

    let cfg = DbSchemaConfig::parse_yaml("baseline.yaml", &updated).expect("valid baseline");
    let clean = evaluate(&s, None, &cfg);
    assert!(clean.passed(), "{clean:?}");

    let fixed_t = table_with_pk("events", vec![col("id", bigint_type(), false)], &["id"]);
    let fixed_snap = snap(SqlDialect::Postgres, "public", vec![fixed_t]);
    let stale = evaluate(&fixed_snap, None, &cfg);
    assert!(!stale.passed());
    assert_eq!(stale.unmatched_exclusions.len(), 1);
}

fn sample_facts() -> DbSchemaFacts {
    let f1 = DbSchemaFinding {
        rule: TABLE_WITHOUT_PRIMARY_KEY.into(),
        object: DbSchemaObject {
            schema: "public".into(),
            table: Some("old_debt".into()),
            ..DbSchemaObject::default()
        },
        message: "old".into(),
        fix: "add pk".into(),
        source: None,
    };
    let f2 = DbSchemaFinding {
        rule: FK_WITHOUT_INDEX.into(),
        object: DbSchemaObject {
            schema: "public".into(),
            table: Some("orders".into()),
            foreign_key: Some("fk_orders_user".into()),
            ..DbSchemaObject::default()
        },
        message: "fk missing index".into(),
        fix: "add index".into(),
        source: None,
    };
    let (labeled, fixed) = diff_findings(
        Some(slice::from_ref(&f1)),
        vec![(f1.clone(), None), (f2, None)],
    );
    assert_eq!(fixed, Vec::new());
    assert_eq!(labeled[0].change, Some(DbSchemaChange::Same));
    assert_eq!(labeled[1].change, Some(DbSchemaChange::New));

    DbSchemaFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: SourceSnapshotId(sha256(b"snap")),
        producer: Producer {
            name: "sych-db-schema".into(),
            version: "0.1.0".into(),
            config_hash: None,
        },
        target: "migrations/postgres".into(),
        dialect: SqlDialect::Postgres,
        source: DbSchemaSource::StaticSql,
        model_extractor: None,
        status: BackendStatus::Ready,
        base_checked: true,
        findings: labeled,
        fixed: Vec::new(),
        unmatched_exclusions: vec![DbSchemaExclusion {
            rule: TABLE_WITHOUT_PRIMARY_KEY.into(),
            object: "public.gone".into(),
            reason: "legacy".into(),
        }],
        warnings: vec!["model table `ghost` has no matching table in database".into()],
        skipped_rules: Vec::new(),
        limitations: vec!["skipped procedural statement".into()],
    }
}

#[test]
fn diff_findings_and_review_findings_cover_waivers_localization_and_gaps() {
    let facts = sample_facts();
    let mut cfg = DbSchemaConfig::default();
    cfg.rules.insert(
        FK_WITHOUT_INDEX.into(),
        DbSchemaRuleConfig {
            enabled: Some(true),
            severity: Some(RuleSeverity::Critical),
            options: BTreeMap::new(),
        },
    );

    let now = Timestamp::from_second(1_700_000_000).expect("ts");
    let out_en = findings(
        slice::from_ref(&facts),
        &[],
        &[],
        &cfg,
        ReviewLanguage::En,
        now,
    );
    assert_eq!(out_en.len(), 2);
    assert_eq!(out_en[0].category, FindingCategory::RuleViolation);
    assert_eq!(out_en[0].rule_id.as_deref(), Some("DB-FK-WITHOUT-INDEX"));
    assert_eq!(out_en[0].severity, Severity::Critical);
    assert_eq!(out_en[1].rule_id.as_deref(), Some(STALE_EXCLUSION_RULE));

    let repo_rule = Rule {
        id: "DB-FK-WITHOUT-INDEX".into(),
        version: 1,
        title: "FK index".into(),
        kind: RuleKind::Invariant,
        required: true,
        severity_on_violation: None,
        owner: "team".into(),
        applies_to: AppliesTo::default(),
        statement: "every fk has an index".into(),
        rationale: None,
        contract_refs: Vec::new(),
        acceptance: None,
        exceptions: vec![RuleException {
            paths: vec!["**".into()],
            reason: "accepted".into(),
            owner: "team".into(),
            author: "alice".into(),
            expires_at: None,
            until: None,
        }],
    };
    let waived = findings(
        slice::from_ref(&facts),
        &[repo_rule],
        &[],
        &cfg,
        ReviewLanguage::Ru,
        now,
    );
    assert_eq!(waived[0].category, FindingCategory::Improvement);
    assert_eq!(waived[0].severity, Severity::Low);
    assert!(waived[0].rule_id.is_none());
    assert!(waived[0].title.contains("Внешний ключ"));

    let g = gaps(slice::from_ref(&facts));
    assert!(g.iter().any(|s| s.contains("DB_SCHEMA_PARTIAL")));
    assert!(g.iter().any(|s| s.contains("DB_SCHEMA_UNMAPPED_MODEL")));
}

#[test]
fn policy_serialization_omits_default_schema_lint() {
    let mut files = PolicyFiles::default();
    files.files.insert(
        ".sych/config.yaml".parse::<RepoPath>().expect("path"),
        b"schema_version: 1\n".to_vec(),
    );
    let server = ServerConfig::engine_default();
    let policy = RepositoryPolicy::parse(&files, &server).expect("policy");
    assert!(!policy.config.schema_lint.enabled);
    assert!(policy.schema_lint.is_none());
    let json = serde_json::to_string(&policy.config).expect("json");
    assert!(!json.contains("schema_lint"));
    let server_json = serde_json::to_string(&server).expect("server json");
    assert!(!server_json.contains("schema_lint"));
}
