//! The findings architecture review adds to a result without a model: each check's finding that
//! the change introduces or makes worse, and, where the diff base was not checked, those in the
//! files it touches. A smell is an improvement; a required repository rule `ARCH-<CHECK>`
//! (`ARCH-MODULE-CYCLE`, `ARCH-FIELD-LEAK`, …) makes it a rule violation, which an exception of
//! that rule in force over its paths waives as it waives an obligation: the finding is kept,
//! but no longer raises the recommendation. Existing debt and fixed findings stay in the facts.

use std::fmt::Write;

use super::facts::{ArchitectureFacts, ArchitectureFinding};
use super::{Change, Check};
use crate::config::Rule;
use crate::publication::tr;
use crate::{
    BackendStatus, Finding, FindingCategory, FindingId, Lifecycle, ReviewLanguage, Severity,
    Support, Timestamp, derived_uuid, glob_match, sha256,
};

/// What opens the picture at the end of a finding's explanation, after its one paragraph.
pub const PICTURE: &str = "\n\n```text\n";

/// The repository rule that makes `check` a rule violation.
pub fn rule_id(check: Check) -> String {
    format!("ARCH-{}", check.name().to_ascii_uppercase())
}

/// A task gap for each project whose checks did not run or whose diff base was not checked:
/// its silence is not a clean result.
pub fn gaps(facts: &[ArchitectureFacts]) -> Vec<String> {
    let name = |f: &ArchitectureFacts| {
        let dir = if f.project.is_empty() {
            "."
        } else {
            &f.project
        };
        format!("{:?} project {dir}", f.language)
    };
    facts
        .iter()
        .filter_map(|f| {
            if f.status != BackendStatus::Ready {
                Some(format!(
                    "ARCHITECTURE_UNAVAILABLE: {}: {}",
                    name(f),
                    f.limitations.last().map_or("", String::as_str)
                ))
            } else if !f.base_checked {
                Some(format!(
                    "ARCHITECTURE_BASE_UNCHECKED: {}: the diff base was not checked, so no \
                     finding is told new or old",
                    name(f)
                ))
            } else {
                None
            }
        })
        .collect()
}

/// `changed`: whether the change touches a path on head.
pub fn findings(
    facts: &[ArchitectureFacts],
    rules: &[Rule],
    changed: &dyn Fn(&str) -> bool,
    l: ReviewLanguage,
    now: Timestamp,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for project in facts.iter().filter(|f| f.status == BackendStatus::Ready) {
        for found in &project.findings {
            let path = found.code_ref.as_ref().map(|r| r.path.as_str());
            let raised = match found.change {
                Some(change) => matches!(change, Change::New | Change::Worse),
                None => path.is_some_and(changed),
            };
            if raised {
                out.push(finding(project, found, rules, l, now));
            }
        }
    }
    out
}

fn finding(
    project: &ArchitectureFacts,
    found: &ArchitectureFinding,
    rules: &[Rule],
    l: ReviewLanguage,
    now: Timestamp,
) -> Finding {
    let check = found.check;
    let (title, consequence, action) = texts(check, l);
    let mut limitations = Vec::new();
    if found.change.is_none() {
        limitations.push(
            tr(
                l,
                "the diff base was not checked: whether this change introduced it is unknown",
                "базовая версия не проверена: внесло ли это изменение находку, неизвестно",
            )
            .to_owned(),
        );
    }

    limitations.extend(project.partial(check).map(str::to_owned));
    let path = found.code_ref.as_ref().map(|r| r.path.as_str());
    let id = rule_id(check);
    let rule = rules.iter().find(|r| r.id == id && r.required);
    let waiver = rule.and_then(|r| {
        r.exceptions.iter().find(|e| {
            e.expires_at.is_none_or(|at| at > now)
                && path.is_some_and(|p| e.paths.iter().any(|g| glob_match(g, p)))
        })
    });
    let (category, severity) = match (rule, waiver) {
        (Some(rule), None) => (
            FindingCategory::RuleViolation,
            rule.severity_on_violation
                .map_or(Severity::Medium, Severity::from),
        ),
        _ => (FindingCategory::Improvement, Severity::Low),
    };
    if let (Some(rule), Some(e)) = (rule, waiver) {
        limitations.push(format!(
            "{}: waived by {} ({}); the finding is kept",
            rule.id, e.owner, e.reason
        ));
    }

    Finding {
        id: FindingId(derived_uuid(&["finding", "architecture", check.name(), &found.key])),
        category,
        severity,
        support: Support::StaticTrace,
        title: title.into(),
        explanation: explanation(found, l),
        triggering_conditions: Vec::new(),
        consequence: consequence.into(),
        anchors: found.code_ref.iter().cloned().collect(),
        evidence_ids: vec![project.evidence_id()],
        rule_id: rule.map(|r| r.id.clone()),
        obligation_ids: Vec::new(),
        suggested_action: action.into(),
        validation_expectation: tr(
            l,
            "The architecture check no longer reports it, because a boundary of responsibility moved, not because code was spread over more files.",
            "Архитектурная проверка больше её не находит, потому что сдвинулась граница ответственности, а не потому что код разнесён по файлам.",
        )
        .into(),
        // The check and its key: lineage follows the finding while lines move.
        fingerprint: sha256(format!("architecture\0{}\0{}", check.name(), found.key).as_bytes()),
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: project.project.clone(),
        limitations,
        suggested_change: None,
        also: Vec::new(),
    }
}

/// The computed message, what the change did to it, the notes and the picture.
fn explanation(found: &ArchitectureFinding, l: ReviewLanguage) -> String {
    let unit = match found.check {
        Check::ModuleCycle => tr(l, "modules", "модулей"),
        Check::Tangle => tr(l, "things", "сущностей"),
        Check::FieldLeak => tr(l, "places", "мест"),
        Check::ShallowModule => tr(l, "functions shown", "показанных функций"),
        Check::WideDirectory => tr(l, "files and directories", "файлов и директорий"),
        Check::LongFile => tr(l, "lines", "строк"),
    };
    let mut out = format!("{}.", found.message);
    match (found.change, found.base_size) {
        (Some(Change::New), _) => out.push_str(tr(
            l,
            " The diff base has nothing like it: this change introduces it.",
            " В базовой версии такого нет: его вносит это изменение.",
        )),
        (Some(Change::Worse), Some(before)) => {
            let _ = write!(
                out,
                "{} {before} → {} {unit}.",
                tr(
                    l,
                    " The diff base has it too; this change makes it worse:",
                    " В базовой версии это уже есть; изменение ухудшает:",
                ),
                found.size
            );
        }
        _ => {}
    }
    if !found.notes.is_empty() {
        let _ = write!(out, " {}", found.notes.join("; "));
        out.push('.');
    }

    if !found.picture.is_empty() {
        out.push_str(PICTURE);
        for line in &found.picture {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("```");
    }

    out
}

/// Title, consequence and action of a check.
fn texts(check: Check, l: ReviewLanguage) -> (&'static str, &'static str, &'static str) {
    match check {
        Check::ModuleCycle => (
            tr(
                l,
                "Modules use each other in a circle",
                "Модули используют друг друга по кругу",
            ),
            tr(
                l,
                "There is no order to read, test or change them in: each is understood only with the others, and the next change to one reaches the rest.",
                "Их нельзя читать, тестировать или менять по порядку: каждый понятен только вместе с остальными, и следующее изменение одного задевает другие.",
            ),
            tr(
                l,
                "Decide which module is the lower layer and move what the other lends it, starting with the cheapest tie listed first, to where it is used; do not only move files around.",
                "Решите, какой модуль нижний слой, и перенесите то, что ему одалживает другой, начиная с самой дешёвой связи (первой в списке), туда, где это используется; не ограничивайтесь переносом файлов.",
            ),
        ),
        Check::Tangle => (
            tr(
                l,
                "Things know each other in a circle",
                "Сущности знают друг о друге по кругу",
            ),
            tr(
                l,
                "None of them can be learned or changed alone: the circle is one unit spread over its modules, or larger than a reader holds in mind.",
                "Ни одну из них нельзя понять или изменить отдельно: круг — это одна единица, разнесённая по модулям, либо слишком большая, чтобы удержать её в голове.",
            ),
            tr(
                l,
                "Find the layer each thing belongs to and let a caller above both use them, so the lower one stops knowing the upper; moving an impl block changes nothing.",
                "Определите слой каждой сущности и пусть их использует вызывающий код уровнем выше, чтобы нижняя перестала знать о верхней; перенос impl-блока ничего не меняет.",
            ),
        ),
        Check::FieldLeak => (
            tr(
                l,
                "A field is worked on outside its type",
                "Поле обрабатывается вне своего типа",
            ),
            tr(
                l,
                "The type no longer guards its state: code elsewhere depends on how it keeps it, so changing the representation breaks callers, and its invariants are not its own.",
                "Тип больше не охраняет своё состояние: внешний код зависит от того, как оно хранится, поэтому смена представления ломает вызывающих, а инварианты перестают быть его собственными.",
            ),
            tr(
                l,
                "Look at what the outside code does with the field and give the type one narrow operation for it; a getter or setter in front of the field does not hide it.",
                "Посмотрите, что внешний код делает с полем, и дайте типу одну узкую операцию для этого; getter или setter перед полем его не скрывает.",
            ),
        ),
        Check::ShallowModule => (
            tr(
                l,
                "A module shows as much as it hides",
                "Модуль показывает столько же, сколько скрывает",
            ),
            tr(
                l,
                "Its interface costs as much to learn as its implementation would: callers gain nothing from the boundary.",
                "Его интерфейс так же дорог в изучении, как и реализация: граница ничего не даёт вызывающему коду.",
            ),
            tr(
                l,
                "Find the patterns in which callers use these functions and offer those as a few deeper operations, hiding the rest behind them; splitting the module into files does not narrow it.",
                "Найдите сценарии, в которых вызывающие используют эти функции, и предложите их как несколько более глубоких операций, скрыв остальное за ними; разбиение модуля на файлы его не сужает.",
            ),
        ),
        Check::WideDirectory => (
            tr(
                l,
                "A directory holds too many files",
                "В директории слишком много файлов",
            ),
            tr(
                l,
                "Past this many names a listing is searched rather than read, and the parts that belong together are not shown together.",
                "При таком числе имён список не читают, а ищут в нём, и связанные части не видны вместе.",
            ),
            tr(
                l,
                "Group the files by what they are about into directories of their own, each with its own narrow entry point.",
                "Сгруппируйте файлы по смыслу в собственные директории, у каждой — свой узкий вход.",
            ),
        ),
        Check::LongFile => (
            tr(l, "A file is too long", "Файл слишком длинный"),
            tr(
                l,
                "A file this long holds parts that want files of their own; a reader finds their way in it by scrolling.",
                "Такой длинный файл содержит части, которым нужны свои файлы; по нему перемещаются прокруткой.",
            ),
            tr(
                l,
                "Move each independent responsibility into a module of its own, behind the interface its callers need.",
                "Вынесите каждую независимую ответственность в отдельный модуль за тем интерфейсом, который нужен вызывающим.",
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
