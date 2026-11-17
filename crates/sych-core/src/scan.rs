//! Deterministic checks of what a change adds, with no model: a committed secret
//! (`SEC-SECRET`) and text that reads otherwise than it runs (`SEC-HIDDEN-TEXT`). A hit is a
//! finding anchored on its lines, one per rule and file; the secret itself is never quoted.

use crate::publication::tr;
use crate::{
    CodeRef, ContentKind, DiffManifest, Finding, FindingCategory, FindingId, Lifecycle, RepoPath,
    ReviewLanguage, Severity, SnapshotReader, SourceSide, Support, derived_uuid, glob_match,
    holds_secret, is_secret_path, sha256,
};

pub const SECRET: &str = "SEC-SECRET";
pub const HIDDEN_TEXT: &str = "SEC-HIDDEN-TEXT";
/// Lines one finding cites; more hits in the file are the same finding.
const MAX_ANCHORS: usize = 5;

/// Bidi overrides and isolates (Trojan Source, CVE-2021-42574) and invisible characters that
/// can hide code. Joiners stay out: scripts and emoji need them.
pub fn is_hidden(c: char) -> bool {
    matches!(
        c,
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}' | '\u{2060}' | '\u{feff}'
    )
}

/// Findings in the HEAD lines `manifest` adds, outside generated files and `fixture_paths`.
pub fn scan(
    manifest: &DiffManifest,
    reader: &dyn SnapshotReader,
    fixture_paths: &[String],
    language: ReviewLanguage,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for file in &manifest.files {
        let Some(new) = &file.new else { continue };
        let path = &new.path;
        if file.generated || fixture_paths.iter().any(|g| glob_match(g, path.as_str())) {
            continue;
        }

        // Its content is never read, so the file itself is the finding.
        if is_secret_path(path.as_str()) {
            out.push(finding(SECRET, path, Vec::new(), language));
            continue;
        }

        if new.content != ContentKind::Text {
            continue;
        }

        let (mut secrets, mut hidden) = (Vec::new(), Vec::new());
        for hunk in file.hunks.iter().filter(|h| h.new_lines > 0) {
            let end = hunk.new_start + hunk.new_lines - 1;
            let Ok(span) = reader
                .code_ref(SourceSide::Head, path, hunk.new_start, end)
                .and_then(|r| reader.read_span(&r))
            else {
                continue;
            };
            for (line, text) in (hunk.new_start..).zip(span.text.lines()) {
                if holds_secret(text) {
                    secrets.push(line);
                }

                // A byte order mark opens a file legitimately.
                let text = if line == 1 {
                    text.trim_start_matches('\u{feff}')
                } else {
                    text
                };
                if text.chars().any(is_hidden) {
                    hidden.push(line);
                }
            }
        }
        for (rule, lines) in [(SECRET, secrets), (HIDDEN_TEXT, hidden)] {
            if lines.is_empty() {
                continue;
            }

            let anchors = lines
                .iter()
                .take(MAX_ANCHORS)
                .filter_map(|&l| reader.code_ref(SourceSide::Head, path, l, l).ok())
                .collect();
            out.push(finding(rule, path, anchors, language));
        }
    }
    out
}

fn finding(rule: &str, path: &RepoPath, anchors: Vec<CodeRef>, l: ReviewLanguage) -> Finding {
    let file = anchors.is_empty();
    let (title, explanation, consequence, action) = match rule {
        SECRET if file => (
            tr(
                l,
                "A credentials file is committed",
                "Закоммичен файл с учётными данными",
            ),
            tr(
                l,
                "The change adds a file of a kind that holds credentials: an env file, a private key, credentials or Terraform state. The review never reads such a file, so whether it holds a live secret is not checked.",
                "Изменение добавляет файл того вида, где хранят учётные данные: env-файл, закрытый ключ, credentials или состояние Terraform. Ревью такие файлы не читает, поэтому живой ли в нём секрет, не проверено.",
            ),
            tr(
                l,
                "Everyone who can read the repository, its forks and clones can read the file, and history keeps it after it is deleted.",
                "Файл может прочитать каждый, у кого есть доступ к репозиторию, его форкам и клонам, и история хранит его после удаления.",
            ),
            tr(
                l,
                "Remove the file from the change and ignore it; if it held a real secret, revoke that secret at its issuer.",
                "Уберите файл из изменения и добавьте его в ignore; если в нём был настоящий секрет, отзовите его у выпустившей стороны.",
            ),
        ),
        SECRET => (
            tr(l, "A secret is committed", "Закоммичен секрет"),
            tr(
                l,
                "An added line holds a private key or a token in a well-known format (GitHub, GitLab, Slack, AWS, Google, OpenAI). The value is not quoted here.",
                "Добавленная строка содержит закрытый ключ или токен известного формата (GitHub, GitLab, Slack, AWS, Google, OpenAI). Само значение здесь не приводится.",
            ),
            tr(
                l,
                "Anyone who can read the repository can use the credential, and history keeps it after the line is removed.",
                "Учётными данными может воспользоваться любой, кто читает репозиторий, и история хранит их после удаления строки.",
            ),
            tr(
                l,
                "Revoke the credential at its issuer, then load it from the environment or a secret store; removing the line does not undo the exposure.",
                "Отзовите учётные данные у выпустившей стороны и загружайте их из окружения или хранилища секретов; удаление строки утечку не отменяет.",
            ),
        ),
        _ => (
            tr(
                l,
                "Text that reads otherwise than it runs",
                "Текст читается иначе, чем исполняется",
            ),
            tr(
                l,
                "An added line holds a bidirectional control or an invisible character. An editor or a review page can show such a line in another order, or hide a part of it, than the compiler reads (Trojan Source, CVE-2021-42574).",
                "Добавленная строка содержит управляющий символ направления текста или невидимый символ. Редактор или страница ревью может показать такую строку в другом порядке или скрыть её часть, не так, как её читает компилятор (Trojan Source, CVE-2021-42574).",
            ),
            tr(
                l,
                "A reviewer approves code that behaves otherwise than it looks.",
                "Ревьюер одобряет код, который ведёт себя не так, как выглядит.",
            ),
            tr(
                l,
                "Remove the characters, or write them as escapes where a string literal needs them.",
                "Уберите эти символы или запишите их escape-последовательностями там, где они нужны строковому литералу.",
            ),
        ),
    };
    let key = path.as_str();
    Finding {
        id: FindingId(derived_uuid(&["finding", rule, key])),
        category: FindingCategory::Defect,
        severity: Severity::High,
        support: Support::StaticTrace,
        title: title.into(),
        explanation: explanation.into(),
        triggering_conditions: Vec::new(),
        consequence: consequence.into(),
        anchors,
        evidence_ids: Vec::new(),
        rule_id: Some(rule.into()),
        obligation_ids: Vec::new(),
        suggested_action: action.into(),
        validation_expectation: tr(
            l,
            "The added lines hold no secret or hidden text.",
            "В добавленных строках нет секретов и скрытого текста.",
        )
        .into(),
        // The rule and the file: lineage follows the finding while lines move.
        fingerprint: sha256(format!("{rule}\0{key}").as_bytes()),
        predecessor_finding_id: None,
        lifecycle: Lifecycle::Open,
        scope: key.into(),
        limitations: Vec::new(),
        suggested_change: None,
        also: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_hidden;
    use crate::holds_secret;

    #[test]
    fn keys_and_known_tokens_are_secrets_and_lookalikes_are_not() {
        // Split, so this file holds no secret for a scanner, this one included.
        for line in [
            concat!("-----BEGIN OPENSSH PRIVATE", " KEY-----"),
            concat!("let t = \"ghp", "_0123456789abcdefABCDEF\";"),
            concat!("AWS_KEY=AK", "IA0123456789ABCDEF"),
        ] {
            assert!(holds_secret(line), "{line}");
        }
        for line in [
            "password = os.environ[\"DB_PASSWORD\"]",
            "token: Option<String>,",
            "sk-short",
        ] {
            assert!(!holds_secret(line), "{line}");
        }
    }

    #[test]
    fn bidi_and_invisible_characters_are_hidden_and_joiners_are_not() {
        assert!("if a \u{202e}} {".chars().any(is_hidden));
        assert!("let a\u{200b}b = 1;".chars().any(is_hidden));
        assert!(!"👨\u{200d}👩".chars().any(is_hidden));
    }
}
