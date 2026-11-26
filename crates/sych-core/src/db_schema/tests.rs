use std::collections::BTreeMap;

use serde_json::json;

use super::rules::{
    DUPLICATE_INDEX, FK_WITHOUT_INDEX, NARROW_KEY, NULLABLE_BOOLEAN, PLAIN_TABLES,
    PREFIX_REDUNDANT_INDEX, REQUIRED_COLUMN, REQUIRED_COLUMN_MISSING,
    REQUIRED_COLUMN_NO_FOREIGN_KEY, REQUIRED_COLUMN_NO_LEADING_INDEX, REQUIRED_COLUMN_NULLABLE,
    REQUIRED_COLUMN_UNIQUE_INDEX, TABLE_WITHOUT_PRIMARY_KEY, lookup,
};
use super::*;
use crate::SchemaVersion;

pub(super) fn col(name: &str, data_type: DbColumnType, nullable: bool) -> DbColumn {
    DbColumn {
        name: name.into(),
        data_type,
        nullable,
        source: None,
    }
}

pub(super) fn bigint_type() -> DbColumnType {
    DbColumnType::Integer {
        raw: "bigint".into(),
        bits: Some(64),
    }
}

fn int32_type() -> DbColumnType {
    DbColumnType::Integer {
        raw: "integer".into(),
        bits: Some(32),
    }
}

fn bool_type() -> DbColumnType {
    DbColumnType::Boolean
}

pub(super) fn varchar_type(len: u32) -> DbColumnType {
    DbColumnType::String {
        raw: format!("character varying({len})"),
        char_max_len: Some(len),
        is_char_type: true,
    }
}

pub(super) fn text_type() -> DbColumnType {
    DbColumnType::String {
        raw: "text".into(),
        char_max_len: None,
        is_char_type: false,
    }
}

pub(super) fn index(name: &str, cols: &[&str], unique: bool) -> DbIndex {
    DbIndex {
        name: name.into(),
        unique,
        method: String::new(),
        predicate: None,
        include: Vec::new(),
        parts: cols
            .iter()
            .map(|c| DbIndexPart {
                column: Some((*c).into()),
                expression: None,
                desc: false,
                opclass: None,
                prefix_length: None,
            })
            .collect(),
        source: None,
    }
}

pub(super) fn fk(
    name: &str,
    cols: &[&str],
    ref_schema: Option<&str>,
    ref_table: &str,
    ref_cols: &[&str],
) -> DbForeignKey {
    DbForeignKey {
        name: name.into(),
        columns: cols.iter().map(|s| (*s).into()).collect(),
        ref_schema: ref_schema.map(str::to_owned),
        ref_table: ref_table.into(),
        ref_columns: ref_cols.iter().map(|s| (*s).into()).collect(),
        source: None,
    }
}

pub(super) fn snap(
    dialect: SqlDialect,
    schema_name: &str,
    tables: Vec<DbTable>,
) -> DbSchemaSnapshot {
    DbSchemaSnapshot {
        schema_version: SchemaVersion,
        dialect,
        source: DbSchemaSource::StaticSql,
        schemas: vec![DbSchema {
            name: schema_name.into(),
            tables,
            views: Vec::new(),
            triggers: Vec::new(),
            functions: Vec::new(),
        }],
        limitations: Vec::new(),
    }
}

pub(super) fn table(name: &str, columns: Vec<DbColumn>) -> DbTable {
    DbTable {
        name: name.into(),
        columns,
        primary_key: None,
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        checks: Vec::new(),
        source: None,
    }
}

pub(super) fn table_with_pk(name: &str, columns: Vec<DbColumn>, pk_cols: &[&str]) -> DbTable {
    let mut t = table(name, columns);
    t.primary_key = Some(index(&format!("{name}_pkey"), pk_cols, true));
    t
}

#[test]
fn rule_catalogue_is_complete_alphabetical_and_documented() {
    assert_eq!(ALL_RULES.len(), 18);
    let ids: Vec<&str> = ALL_RULES.iter().map(|r| r.id).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ALL_RULES must stay sorted alphabetically");

    for spec in ALL_RULES {
        assert!(!spec.why.is_empty(), "empty why for {}", spec.id);
        assert!(!spec.fix.is_empty(), "empty fix for {}", spec.id);
        assert!(
            !spec.exception.is_empty(),
            "empty exception for {}",
            spec.id
        );
        assert!(spec.review_rule_id().starts_with("DB-"));
        assert!(lookup(spec.id).is_some());
    }
}

#[test]
fn table_without_primary_key_across_all_dialects() {
    for dialect in [SqlDialect::Postgres, SqlDialect::Mysql, SqlDialect::Sqlite] {
        let with_pk = table_with_pk("users", vec![col("id", bigint_type(), false)], &["id"]);
        let no_pk = table("events", vec![col("payload", text_type(), false)]);
        let s = snap(dialect, dialect.default_schema(), vec![with_pk, no_pk]);
        let report = evaluate(&s, None, &DbSchemaConfig::default());
        assert_eq!(report.findings.len(), 1, "dialect={dialect}");
        assert_eq!(report.findings[0].rule, TABLE_WITHOUT_PRIMARY_KEY);
        assert_eq!(report.findings[0].object.table.as_deref(), Some("events"));
    }
}

#[test]
fn fk_without_index_handles_order_partial_expression_prefix_and_pk() {
    let mut orders = table_with_pk(
        "orders",
        vec![
            col("id", bigint_type(), false),
            col("user_id", bigint_type(), false),
            col("org_id", bigint_type(), false),
            col("item_id", bigint_type(), false),
            col("partial_id", bigint_type(), false),
            col("expr_id", bigint_type(), false),
            col("prefix_id", bigint_type(), false),
        ],
        &["id"],
    );
    orders
        .indexes
        .push(index("idx_user_org", &["user_id", "org_id"], false));
    let mut partial = index("idx_partial", &["partial_id"], false);
    partial.predicate = Some("deleted_at IS NULL".into());
    orders.indexes.push(partial);
    orders.indexes.push(DbIndex {
        name: "idx_expr".into(),
        unique: false,
        method: String::new(),
        predicate: None,
        include: Vec::new(),
        parts: vec![DbIndexPart {
            column: None,
            expression: Some("abs(expr_id)".into()),
            desc: false,
            opclass: None,
            prefix_length: None,
        }],
        source: None,
    });
    orders.indexes.push(DbIndex {
        name: "idx_prefix".into(),
        unique: false,
        method: String::new(),
        predicate: None,
        include: Vec::new(),
        parts: vec![DbIndexPart {
            column: Some("prefix_id".into()),
            expression: None,
            desc: false,
            opclass: None,
            prefix_length: Some(4),
        }],
        source: None,
    });
    for (name, cols, ref_tbl, ref_cols) in [
        (
            "fk_ok",
            &["org_id", "user_id"][..],
            "members",
            &["org_id", "user_id"][..],
        ),
        ("fk_pk_covered", &["id"][..], "roots", &["id"][..]),
        ("fk_missing", &["item_id"][..], "items", &["id"][..]),
        ("fk_partial", &["partial_id"][..], "items", &["id"][..]),
        ("fk_expr", &["expr_id"][..], "items", &["id"][..]),
        ("fk_prefix", &["prefix_id"][..], "items", &["id"][..]),
    ] {
        orders
            .foreign_keys
            .push(fk(name, cols, Some("public"), ref_tbl, ref_cols));
    }

    let s = snap(SqlDialect::Postgres, "public", vec![orders]);
    let report = evaluate(&s, None, &DbSchemaConfig::default());
    let fk_names: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule == FK_WITHOUT_INDEX)
        .filter_map(|f| f.object.foreign_key.as_deref())
        .collect();
    assert_eq!(
        fk_names,
        vec!["fk_expr", "fk_missing", "fk_partial", "fk_prefix"]
    );
}

#[test]
fn duplicate_and_prefix_redundant_indexes_respect_uniqueness_predicate_and_include() {
    let mut t = table_with_pk(
        "items",
        vec![
            col("id", bigint_type(), false),
            col("a", bigint_type(), false),
            col("b", bigint_type(), false),
            col("c", bigint_type(), false),
        ],
        &["id"],
    );
    // Exact duplicate of a_b.
    t.indexes.push(index("idx_ab_1", &["a", "b"], false));
    t.indexes.push(index("idx_ab_2", &["a", "b"], false));
    // Non-unique (a) is prefix-redundant under (a, b).
    t.indexes.push(index("idx_a_redundant", &["a"], false));
    // Unique (a) is NOT prefix-redundant under non-unique (a, b).
    t.indexes.push(index("uq_a", &["a"], true));
    // Non-unique (a) INCLUDE (c) is NOT redundant under (a, b) without c.
    let mut inc = index("idx_a_inc_c", &["a"], false);
    inc.include.push("c".into());
    t.indexes.push(inc);
    // Partial (a) is NOT redundant under full (a, b).
    let mut part = index("idx_a_partial", &["a"], false);
    part.predicate = Some("b > 0".into());
    t.indexes.push(part);

    let s = snap(SqlDialect::Postgres, "public", vec![t]);
    let report = evaluate(&s, None, &DbSchemaConfig::default());
    let dups: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule == DUPLICATE_INDEX)
        .filter_map(|f| f.object.index.as_deref())
        .collect();
    let prefixes: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule == PREFIX_REDUNDANT_INDEX)
        .filter_map(|f| f.object.index.as_deref())
        .collect();
    assert_eq!(dups, vec!["idx_ab_2"]);
    assert_eq!(prefixes, vec!["idx_a_redundant"]);
}

#[test]
fn nullable_boolean_and_narrow_key_rules() {
    let t = table_with_pk(
        "accounts",
        vec![
            col("id", int32_type(), false),
            col("active", bool_type(), true),
            col("verified", bool_type(), false),
        ],
        &["id"],
    );
    let s = snap(SqlDialect::Postgres, "public", vec![t]);
    let report = evaluate(&s, None, &DbSchemaConfig::default());
    let rules: Vec<(&str, Option<&str>)> = report
        .findings
        .iter()
        .map(|f| (f.rule.as_str(), f.object.column.as_deref()))
        .collect();
    assert_eq!(
        rules,
        vec![(NARROW_KEY, Some("id")), (NULLABLE_BOOLEAN, Some("active"))]
    );

    // SQLite skips narrow-key and lists it in skipped_rules.
    let sqlite_s = snap(
        SqlDialect::Sqlite,
        "main",
        vec![table_with_pk(
            "accounts",
            vec![col("id", int32_type(), false)],
            &["id"],
        )],
    );
    let sqlite_report = evaluate(&sqlite_s, None, &DbSchemaConfig::default());
    assert_eq!(sqlite_report.findings, Vec::new());
    assert!(sqlite_report.skipped_rules.contains(&NARROW_KEY.to_owned()));
}

#[test]
fn plain_tables_is_default_off_and_supports_exclusions() {
    let mut s = snap(SqlDialect::Postgres, "public", Vec::new());
    s.schemas[0].views.push(DbView {
        name: "active_users".into(),
        extension_owned: false,
    });
    s.schemas[0].views.push(DbView {
        name: "pg_stat_statements".into(),
        extension_owned: true,
    });
    s.schemas[0].functions.push(DbFunction {
        name: "audit_fn".into(),
        extension_owned: false,
    });
    s.schemas[0].functions.push(DbFunction {
        name: "internal_helper".into(),
        extension_owned: false,
    });
    s.schemas[0].triggers.push(DbTrigger {
        table: "users".into(),
        name: "tr_users_audit".into(),
        extension_owned: false,
    });
    s.schemas[0].tables.push(table_with_pk(
        "users",
        vec![col("id", bigint_type(), false)],
        &["id"],
    ));

    // Default off.
    assert_eq!(
        evaluate(&s, None, &DbSchemaConfig::default()).findings,
        Vec::new()
    );

    // Enabled with options.
    let mut cfg = DbSchemaConfig::default();
    let mut opts = BTreeMap::new();
    opts.insert("exclude".into(), json!(["audit_fn"]));
    opts.insert("exclude_prefixes".into(), json!(["internal_"]));
    opts.insert("exclude_extensions".into(), json!(true));
    cfg.rules.insert(
        PLAIN_TABLES.into(),
        DbSchemaRuleConfig {
            enabled: Some(true),
            severity: None,
            options: opts,
        },
    );
    let report = evaluate(&s, None, &cfg);
    let objs: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.rule == PLAIN_TABLES)
        .map(|f| f.object.format_exclusion())
        .collect();
    assert_eq!(
        objs,
        vec![
            "public.view:active_users",
            "public.users.trigger:tr_users_audit",
        ]
    );
}

#[test]
fn required_column_emits_all_five_sub_rules_and_respects_toggles() {
    let no_tenant = table_with_pk("logs", vec![col("id", bigint_type(), false)], &["id"]);

    let mut bad = table_with_pk(
        "orders",
        vec![
            col("id", bigint_type(), false),
            col("tenant_id", bigint_type(), true),
            col("code", bigint_type(), false),
        ],
        &["id"],
    );
    bad.indexes.push(index("idx_code", &["code"], false));
    bad.indexes.push(index("uq_code", &["code"], true));

    let excluded = table_with_pk("tenants", vec![col("id", bigint_type(), false)], &["id"]);

    let s = snap(
        SqlDialect::Postgres,
        "public",
        vec![no_tenant, bad, excluded],
    );

    let mut cfg = DbSchemaConfig::default();
    let mut opts = BTreeMap::new();
    opts.insert("column".into(), json!("tenant_id"));
    opts.insert("exclude_tables".into(), json!(["tenants"]));
    cfg.rules.insert(
        REQUIRED_COLUMN.into(),
        DbSchemaRuleConfig {
            enabled: Some(true),
            severity: None,
            options: opts,
        },
    );

    let report = evaluate(&s, None, &cfg);
    let rules: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.rule.starts_with("required-column-"))
        .map(|f| f.rule.as_str())
        .collect();
    assert!(rules.contains(&REQUIRED_COLUMN_MISSING));
    assert!(rules.contains(&REQUIRED_COLUMN_NULLABLE));
    assert!(rules.contains(&REQUIRED_COLUMN_NO_FOREIGN_KEY));
    assert!(rules.contains(&REQUIRED_COLUMN_NO_LEADING_INDEX));
    assert!(rules.contains(&REQUIRED_COLUMN_UNIQUE_INDEX));
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.object.table.as_deref() == Some("tenants"))
    );

    // Disable one child rule.
    cfg.rules.insert(
        REQUIRED_COLUMN_NO_FOREIGN_KEY.into(),
        DbSchemaRuleConfig {
            enabled: Some(false),
            ..DbSchemaRuleConfig::default()
        },
    );
    let report = evaluate(&s, None, &cfg);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.rule == REQUIRED_COLUMN_NO_FOREIGN_KEY)
    );
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.rule == REQUIRED_COLUMN_MISSING)
    );

    // Disable parent rule disables all children.
    cfg.rules.get_mut(REQUIRED_COLUMN).expect("rule").enabled = Some(false);
    let report = evaluate(&s, None, &cfg);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.rule.starts_with("required-column-"))
    );
}
