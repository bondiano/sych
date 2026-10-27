//! The text of one finding, shared by its inline comment and the summary: a severity badge,
//! the explanation, when it happens and what follows, the suggested action and a prompt a
//! coding agent can act on.

use std::fmt::Write;

use super::tr;
use crate::{Finding, FindingCategory, ReviewLanguage, Severity, SourceSide};

/// The severity with a colour dot; severity names stay English in every language.
pub(crate) const fn badge(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "🔴 Critical",
        Severity::High => "🟠 High",
        Severity::Medium => "🟡 Medium",
        Severity::Low => "🔵 Low",
        Severity::Info => "⚪ Info",
    }
}

pub(crate) const fn category(language: ReviewLanguage, category: FindingCategory) -> &'static str {
    match category {
        FindingCategory::Defect => tr(language, "Bug", "Ошибка"),
        FindingCategory::RuleViolation => tr(language, "Rule violation", "Нарушение правила"),
        FindingCategory::Improvement => tr(language, "Improvement", "Улучшение"),
        FindingCategory::Nit => tr(language, "Nit", "Мелочь"),
    }
}

/// `path:start-end` of the first anchor.
pub(crate) fn lines(finding: &Finding) -> Option<String> {
    finding.anchors.first().map(|r| {
        if r.start_line < r.end_line {
            format!("{}:{}-{}", r.path.as_str(), r.start_line, r.end_line)
        } else {
            format!("{}:{}", r.path.as_str(), r.start_line)
        }
    })
}

/// A fence longer than any backtick run in `text`, so model prose cannot close it.
fn fence(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// The explanation, a when/consequence table, the suggested action and its code; `inline`
/// when the text is the comment on the finding's first anchor.
pub(crate) fn details(out: &mut String, finding: &Finding, language: ReviewLanguage, inline: bool) {
    let l = language;
    let _ = writeln!(out, "{}", finding.explanation);
    let when = finding.triggering_conditions.join("; ");
    let columns: Vec<_> = [
        (tr(l, "When", "Когда"), when.as_str()),
        (
            tr(l, "Consequence", "Последствие"),
            finding.consequence.as_str(),
        ),
    ]
    .into_iter()
    .filter(|(_, text)| !text.is_empty())
    .collect();
    if !columns.is_empty() {
        let row = |f: &dyn Fn(&(&str, &str)) -> String| {
            columns.iter().map(f).collect::<Vec<_>>().join(" | ")
        };
        let _ = writeln!(
            out,
            "\n| {} |\n|{}|\n| {} |",
            row(&|c| c.0.to_owned()),
            vec!["---"; columns.len()].join("|"),
            row(&|c| cell(c.1))
        );
    }

    if !finding.suggested_action.is_empty() {
        let _ = writeln!(
            out,
            "\n**{}:** {}",
            tr(l, "Suggested", "Что сделать"),
            finding.suggested_action
        );
    }

    if let Some(change) = &finding.suggested_change {
        let code = &change.code;
        let fence = fence(&change.replacement);
        // GitHub replaces exactly the commented lines with a `suggestion` block.
        let commented = finding.anchors.first().is_some_and(|a| {
            a.side == SourceSide::Head
                && (&a.path, a.start_line, a.end_line)
                    == (&code.path, code.start_line, code.end_line)
        });
        let (summary, info) = if inline && commented {
            (
                tr(l, "📝 Committable suggestion", "📝 Готовое исправление"),
                "suggestion",
            )
        } else {
            (tr(l, "📝 Suggested code", "📝 Предлагаемый код"), "")
        };
        // An empty block deletes the lines; one empty line would replace them with a blank.
        let lines = if change.replacement.is_empty() {
            String::new()
        } else {
            format!("{}\n", change.replacement)
        };
        let _ = writeln!(
            out,
            "\n<details open><summary>{summary}: <code>{}:{}-{}</code></summary>\n\n\
             {fence}{info}\n{lines}{fence}\n\n</details>",
            code.path.as_str(),
            code.start_line,
            code.end_line
        );
    }
}

/// One instruction for a coding agent: where, what is wrong, the fix and how to check it.
pub(crate) fn task(finding: &Finding, language: ReviewLanguage) -> String {
    let l = language;
    let mut out = match lines(finding) {
        Some(at) => format!("{} `{at}`: ", tr(l, "In", "В")),
        None => String::new(),
    };
    let _ = write!(out, "{}. {}", finding.title, finding.explanation);
    for (label, text) in [
        (tr(l, "Fix", "Исправить"), &finding.suggested_action),
        (tr(l, "Check", "Проверить"), &finding.validation_expectation),
    ] {
        if !text.is_empty() {
            let _ = write!(out, " {label}: {text}");
        }
    }
    out
}

/// A collapsed block with `tasks` for coding agents, asking to verify each one first.
pub(crate) fn agent_prompt(out: &mut String, summary: &str, tasks: &str, language: ReviewLanguage) {
    let text = format!(
        "{}\n\n{tasks}",
        tr(
            language,
            "Verify each finding against the current code; fix only those that still apply and \
             skip the rest with a short reason.",
            "Проверьте каждое замечание по текущему коду; исправьте только актуальные, остальные \
             пропустите с короткой причиной."
        )
    );
    let fence = fence(&text);
    let _ = writeln!(
        out,
        "\n<details><summary>🤖 {summary}</summary>\n\n{fence}text\n{text}\n{fence}\n\n</details>"
    );
}

/// The body of an inline comment in `language`; the marker finds it again.
pub fn inline_body(finding: &Finding, language: ReviewLanguage) -> String {
    let l = language;
    let rule = finding
        .rule_id
        .as_deref()
        .map(|r| format!(" · `{r}`"))
        .unwrap_or_default();
    let mut out = format!(
        "<!-- sych:inline:{} -->\n**{} · {}**{rule}\n\n#### {}\n\n",
        finding.fingerprint,
        badge(finding.severity),
        category(l, finding.category),
        finding.title
    );
    details(&mut out, finding, l, true);
    agent_prompt(
        &mut out,
        tr(l, "Prompt for AI agents", "Промпт для AI-агента"),
        &task(finding, l),
        l,
    );
    let _ = writeln!(
        out,
        "\n<sub>{}</sub>",
        tr(
            l,
            "Advisory finding by Sych; not a merge gate.",
            "Рекомендательное замечание Sych; не блокирует слияние."
        )
    );
    out
}
