//! Converts [`DbSchemaFacts`] into deterministic `DB-*` [`Finding`] values
//! and task gaps for the Sych review pipeline.

use super::config::{DbSchemaConfig, DbSchemaExclusion};
use super::facts::{DbSchemaChange, DbSchemaFactFinding, DbSchemaFacts};
use super::rules::{self, review_rule_id};
use crate::config::{Rule, RuleException};
use crate::publication::tr;
use crate::{
    BackendStatus, Finding, FindingCategory, FindingId, Lifecycle, ReviewLanguage, Severity,
    Support, Timestamp, derived_uuid, glob_match, sha256,
};

/// Review rule id emitted when a configured schema exclusion matches nothing.
pub const STALE_EXCLUSION_RULE: &str = "DB-STALE-EXCLUSION";

/// Task gaps for schema targets that could not be checked or had unmapped model
/// tables: silence is never mistaken for a clean check.
pub fn gaps(facts: &[DbSchemaFacts]) -> Vec<String> {
    let mut out = Vec::new();
    for doc in facts {
        if doc.status != BackendStatus::Ready {
            out.push(format!(
                "DB_SCHEMA_UNAVAILABLE: {} ({}): {}",
                doc.target,
                doc.dialect,
                doc.limitations
                    .last()
                    .map_or("check failed", String::as_str)
            ));
            continue;
        }

        for warning in &doc.warnings {
            out.push(format!(
                "DB_SCHEMA_UNMAPPED_MODEL: {}: {warning}",
                doc.target
            ));
        }

        for limitation in &doc.limitations {
            out.push(format!("DB_SCHEMA_PARTIAL: {}: {limitation}", doc.target));
        }
    }
    out
}

/// Deterministic `DB-*` findings raised when schema linting is enabled in review.
///
/// When `base_checked` is true, only `NEW` findings are raised (existing debt
/// remains in `db_schema_facts`). When `base_checked` is false (live DB or
/// single HEAD CI snapshot), every unexcluded finding is raised. Active waivers
/// on the matching `DB-*` rule lower the finding to `Improvement` at `Low`,
/// matching Sych's waiver contract.
pub fn findings(
    facts: &[DbSchemaFacts],
    repo_rules: &[Rule],
    waivers: &[(String, RuleException)],
    schema_config: &DbSchemaConfig,
    language: ReviewLanguage,
    now: Timestamp,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for doc in facts.iter().filter(|f| f.status == BackendStatus::Ready) {
        for found in &doc.findings {
            let raised = match found.change {
                Some(DbSchemaChange::New) | None => true,
                Some(DbSchemaChange::Same) => false,
            };
            if raised {
                out.push(build_finding(
                    doc,
                    found,
                    repo_rules,
                    waivers,
                    schema_config,
                    language,
                    now,
                ));
            }
        }
        for stale in &doc.unmatched_exclusions {
            out.push(stale_exclusion_finding(
                doc, stale, repo_rules, waivers, language, now,
            ));
        }
    }
    out
}

fn build_finding(
    doc: &DbSchemaFacts,
    found: &DbSchemaFactFinding,
    repo_rules: &[Rule],
    waivers: &[(String, RuleException)],
    schema_config: &DbSchemaConfig,
    l: ReviewLanguage,
    now: Timestamp,
) -> Finding {
    let rule_id = review_rule_id(&found.rule);
    let default_sev = rules::lookup(&found.rule).map_or(Severity::Medium, |r| r.default_severity);
    let configured_sev = schema_config
        .severity_override(&found.rule)
        .map(Severity::from);
    let repo_rule = repo_rules.iter().find(|r| r.id == rule_id);
    let rule_sev = repo_rule
        .and_then(|r| r.severity_on_violation)
        .map(Severity::from);
    let base_severity = rule_sev.or(configured_sev).unwrap_or(default_sev);

    let path = found.code_ref.as_ref().map(|r| r.path.as_str());
    let waiver = active_waiver(repo_rule, waivers, &rule_id, path, now);

    let mut limitations = Vec::new();
    if !doc.limitations.is_empty() {
        limitations.push(
            tr(
                l,
                "schema snapshot is partial; see db_schema_facts limitations for details",
                "снимок схемы БД неполный; подробности в ограничениях db_schema_facts",
            )
            .to_owned(),
        );
    }

    let (category, severity, rule_link) = match waiver {
        Some(ex) => {
            limitations.push(format!(
                "{rule_id}: waived by {} ({}); the finding is kept",
                ex.owner, ex.reason
            ));
            (FindingCategory::Improvement, Severity::Low, None)
        }
        None => (
            FindingCategory::RuleViolation,
            base_severity,
            Some(rule_id.clone()),
        ),
    };

    let object_key = found.object.format_exclusion();
    let object_display = found.object.display_path();
    let title = format!(
        "{}: {} ({object_display})",
        rule_title(&found.rule, l),
        found.rule
    );
    let mut explanation = format!("{}.", found.message);
    if found.change == Some(DbSchemaChange::New) {
        explanation.push_str(tr(
            l,
            " Introduced by this change compared to the diff base.",
            " Внесено этим изменением по сравнению с базовой версией.",
        ));
    }

    let consequence =
        rules::lookup(&found.rule).map_or_else(|| found.message.clone(), |r| r.why.to_owned());

    Finding {
        id: FindingId(derived_uuid(&[
            "finding",
            "db_schema",
            &found.rule,
            &object_key,
        ])),
        category,
        severity,
        support: Support::StaticTrace,
        title,
        explanation,
        triggering_conditions: vec![format!(
            "database object `{object_display}` ({})",
            doc.dialect
        )],
        consequence,
        anchors: found.code_ref.iter().cloned().collect(),
        evidence_ids: vec![doc.evidence_id()],
        rule_id: rule_link,
        obligation_ids: Vec::new(),
        suggested_action: found.fix.clone(),
        validation_expectation: tr(
            l,
            "The schema linter reports zero violations for this database object.",
            "Линтер схемы БД больше не сообщает о нарушении для этого объекта.",
        )
        .to_owned(),
        fingerprint: sha256(format!("db-schema\0{}\0{object_key}", found.rule).as_bytes()),
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: format!("{} ({})", doc.target, object_display),
        limitations,
        suggested_change: None,
        also: Vec::new(),
    }
}

fn stale_exclusion_finding(
    doc: &DbSchemaFacts,
    stale: &DbSchemaExclusion,
    repo_rules: &[Rule],
    waivers: &[(String, RuleException)],
    l: ReviewLanguage,
    now: Timestamp,
) -> Finding {
    let repo_rule = repo_rules.iter().find(|r| r.id == STALE_EXCLUSION_RULE);
    let waiver = active_waiver(repo_rule, waivers, STALE_EXCLUSION_RULE, None, now);
    let (category, severity, rule_link, limitations) = match waiver {
        Some(ex) => (
            FindingCategory::Improvement,
            Severity::Low,
            None,
            vec![format!(
                "{STALE_EXCLUSION_RULE}: waived by {} ({}); the finding is kept",
                ex.owner, ex.reason
            )],
        ),
        None => (
            FindingCategory::RuleViolation,
            repo_rule
                .and_then(|r| r.severity_on_violation)
                .map_or(Severity::Medium, Severity::from),
            Some(STALE_EXCLUSION_RULE.to_owned()),
            Vec::new(),
        ),
    };
    let key = format!("{}\0{}", stale.rule, stale.object);
    Finding {
        id: FindingId(derived_uuid(&["finding", "db_schema", "stale_exclusion", &key])),
        category,
        severity,
        support: Support::ExplicitRule,
        title: format!(
            "{}: {} on {}",
            tr(
                l,
                "Stale database schema exclusion",
                "Устаревшее исключение линтера схемы БД"
            ),
            stale.rule,
            stale.object
        ),
        explanation: format!(
            "Configured exclusion for rule `{}` on `{}` (reason: {}) matched no finding.",
            stale.rule, stale.object, stale.reason
        ),
        triggering_conditions: Vec::new(),
        consequence: tr(
            l,
            "A stale exclusion left in configuration silently hides future regressions on the same database object.",
            "Оставленное в конфиге устаревшее исключение может молча скрыть будущую регрессию на том же объекте БД.",
        )
        .to_owned(),
        anchors: Vec::new(),
        evidence_ids: vec![doc.evidence_id()],
        rule_id: rule_link,
        obligation_ids: Vec::new(),
        suggested_action: format!(
            "Remove the stale exclusion for `{}` on `{}` from schema linter configuration or update its object name.",
            stale.rule, stale.object
        ),
        validation_expectation: tr(
            l,
            "Every configured schema exclusion matches an active finding.",
            "Каждое настроенное исключение схемы соответствует активной находке.",
        )
        .to_owned(),
        fingerprint: sha256(format!("db-schema\0stale-exclusion\0{key}").as_bytes()),
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: doc.target.clone(),
        limitations,
        suggested_change: None,
        also: Vec::new(),
    }
}

fn active_waiver<'a>(
    rule: Option<&'a Rule>,
    waivers: &'a [(String, RuleException)],
    rule_id: &str,
    path: Option<&str>,
    now: Timestamp,
) -> Option<&'a RuleException> {
    let matches = |e: &&RuleException| {
        e.expires_at.is_none_or(|at| at > now)
            && match path {
                Some(p) => e.paths.iter().any(|g| glob_match(g, p)),
                None => e.paths.iter().any(|g| g == "*" || g == "**"),
            }
    };
    rule.and_then(|r| r.exceptions.iter().find(matches))
        .or_else(|| {
            waivers
                .iter()
                .filter(|(id, _)| id == rule_id)
                .map(|(_, e)| e)
                .find(matches)
        })
}

fn rule_title(rule_id: &str, l: ReviewLanguage) -> &'static str {
    match rule_id {
        rules::TABLE_WITHOUT_PRIMARY_KEY => tr(
            l,
            "Table without primary key",
            "Таблица без первичного ключа",
        ),
        rules::FK_WITHOUT_INDEX => tr(l, "Foreign key without index", "Внешний ключ без индекса"),
        rules::DUPLICATE_INDEX => tr(l, "Duplicate database index", "Дублирующийся индекс БД"),
        rules::PREFIX_REDUNDANT_INDEX => tr(
            l,
            "Prefix-redundant database index",
            "Избыточный префиксный индекс БД",
        ),
        rules::NULLABLE_BOOLEAN => tr(l, "Nullable boolean column", "Nullable boolean-колонка"),
        rules::NARROW_KEY => tr(l, "Narrow integer key column", "Узкий целочисленный ключ"),
        rules::PLAIN_TABLES => tr(
            l,
            "Database object outside application model",
            "Объект БД вне модели приложения",
        ),
        rules::REQUIRED_COLUMN_MISSING => tr(
            l,
            "Missing required column",
            "Отсутствует обязательная колонка",
        ),
        rules::REQUIRED_COLUMN_NULLABLE => tr(
            l,
            "Required column is nullable",
            "Обязательная колонка допускает NULL",
        ),
        rules::REQUIRED_COLUMN_NO_FOREIGN_KEY => tr(
            l,
            "Required column lacks foreign key",
            "У обязательной колонки нет внешнего ключа",
        ),
        rules::REQUIRED_COLUMN_NO_LEADING_INDEX => tr(
            l,
            "Required column lacks leading index",
            "У обязательной колонки нет ведущего индекса",
        ),
        rules::REQUIRED_COLUMN_UNIQUE_INDEX => tr(
            l,
            "Unique index does not lead with required column",
            "Уникальный индекс не начинается с обязательной колонки",
        ),
        rules::REQUIRED_FIELD_NULLABLE_COLUMN => tr(
            l,
            "Required model field maps to nullable column",
            "Обязательное поле модели привязано к nullable-колонке",
        ),
        rules::UNIQUE_FIELD_WITHOUT_UNIQUE_INDEX => tr(
            l,
            "Unique model field lacks unique index",
            "Уникальное поле модели не защищено уникальным индексом",
        ),
        rules::RELATION_WITHOUT_FOREIGN_KEY => tr(
            l,
            "Model relation lacks foreign key",
            "Связь модели не обеспечена внешним ключом",
        ),
        rules::MAX_LENGTH_NOT_ENFORCED => tr(
            l,
            "Model max_length not enforced in database",
            "Ограничение max_length модели не проверяется в БД",
        ),
        rules::ENUM_NOT_ENFORCED => tr(
            l,
            "Model enum not enforced in database",
            "Набор enum модели не проверяется в БД",
        ),
        _ => tr(
            l,
            "Database schema rule violation",
            "Нарушение правила схемы БД",
        ),
    }
}
