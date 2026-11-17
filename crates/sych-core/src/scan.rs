//! Deterministic checks of what a change adds, with no model: a committed secret
//! (`SEC-SECRET`), text that reads otherwise than it runs (`SEC-HIDDEN-TEXT`) and a manifest line
//! that takes code from outside the package registry (`SEC-SUPPLY-CHAIN`). A hit is a finding
//! anchored on its lines, one per rule and file; the secret itself is never quoted.

use crate::publication::tr;
use crate::{
    CodeRef, ContentKind, DiffManifest, Finding, FindingCategory, FindingId, Lifecycle, RepoPath,
    ReviewLanguage, Severity, SnapshotReader, SourceSide, Support, derived_uuid, glob_match,
    holds_secret, is_secret_path, sha256,
};

pub const SECRET: &str = "SEC-SECRET";
pub const HIDDEN_TEXT: &str = "SEC-HIDDEN-TEXT";
pub const SUPPLY_CHAIN: &str = "SEC-SUPPLY-CHAIN";
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

/// Whether an added line of a dependency manifest takes code from outside the package registry
/// (a git, URL or local-path dependency, another index, a `replace`) or runs it at install time:
/// what a supply-chain attack needs. Comments and lockfiles are left out.
pub fn outside_registry(path: &str, line: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let line = line.trim();
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    if line.starts_with('#') {
        return false;
    }

    match name {
        "package.json" => npm(line),
        "Cargo.toml" => compact.contains("git=\""),
        "go.mod" => line.contains("=>"),
        "pyproject.toml" => {
            compact.contains("git=\"")
                || ["git+", "@ http", "@ file:"]
                    .iter()
                    .any(|p| line.contains(p))
        }
        _ if name.starts_with("requirements")
            && name
                .rsplit_once('.')
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("txt")) =>
        {
            line.contains("git+")
                || line.contains(" @ ")
                || line.starts_with("http://")
                || line.starts_with("https://")
                || [
                    "--index-url",
                    "--extra-index-url",
                    "--trusted-host",
                    "--find-links",
                    "-i ",
                    "-f ",
                ]
                .iter()
                .any(|f| line.starts_with(f))
        }
        _ => false,
    }
}

// Metadata keys whose value is a URL but installs nothing.
const METADATA: &[&str] = &[
    "$schema",
    "author",
    "bugs",
    "email",
    "funding",
    "homepage",
    "registry",
    "repository",
    "url",
];
const SOURCES: &[&str] = &[
    "git+",
    "git:",
    "github:",
    "gitlab:",
    "bitbucket:",
    "file:",
    "link:",
    "http:",
    "https:",
];

/// An install lifecycle script, or a dependency whose spec is not a registry version.
fn npm(line: &str) -> bool {
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    let key = key.trim().trim_matches('"');
    let value = value.trim().trim_start_matches('"');
    matches!(key, "preinstall" | "install" | "postinstall")
        || (!METADATA.contains(&key) && SOURCES.iter().any(|p| value.starts_with(p)))
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

        let (mut secrets, mut hidden, mut supply) = (Vec::new(), Vec::new(), Vec::new());
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

                if outside_registry(path.as_str(), text) {
                    supply.push(line);
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
        for (rule, lines) in [
            (SECRET, secrets),
            (HIDDEN_TEXT, hidden),
            (SUPPLY_CHAIN, supply),
        ] {
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

/// Title, explanation, consequence and action of `rule`; `file` when the whole file is the hit.
fn texts(
    rule: &str,
    file: bool,
    l: ReviewLanguage,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match rule {
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
        HIDDEN_TEXT => (
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
        _ => (
            tr(
                l,
                "Code from outside the package registry",
                "Код не из реестра пакетов",
            ),
            tr(
                l,
                "An added manifest line installs code the registry does not vouch for (a git, URL or local-path dependency, another package index, a `replace`) or runs a script at install time.",
                "Добавленная строка манифеста ставит код, за который реестр не ручается (зависимость из git, по URL или из локального пути, другой индекс пакетов, `replace`), или запускает скрипт при установке.",
            ),
            tr(
                l,
                "What gets installed is whatever that source serves at install time, and a compromised or look-alike source runs its code on every developer and CI machine.",
                "Ставится то, что источник отдаст в момент установки, и скомпрометированный или поддельный источник выполнит свой код на каждой машине разработчика и в CI.",
            ),
            tr(
                l,
                "Prefer a registry release pinned by version and lockfile hash; if this source is intended, pin it to a commit and say why in the change.",
                "Предпочтите релиз из реестра, закреплённый версией и хешем в lockfile; если этот источник нужен намеренно, закрепите его на коммите и объясните это в изменении.",
            ),
        ),
    }
}

fn finding(rule: &str, path: &RepoPath, anchors: Vec<CodeRef>, l: ReviewLanguage) -> Finding {
    let (title, explanation, consequence, action) = texts(rule, anchors.is_empty(), l);
    let key = path.as_str();
    Finding {
        id: FindingId(derived_uuid(&["finding", rule, key])),
        category: FindingCategory::Defect,
        // A signal, not proof: an intended git dependency is common.
        severity: if rule == SUPPLY_CHAIN {
            Severity::Medium
        } else {
            Severity::High
        },
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
    use super::{is_hidden, outside_registry};
    use crate::holds_secret;

    #[test]
    fn sources_outside_the_registry_and_install_scripts_are_flagged_and_metadata_is_not() {
        for (path, line) in [
            ("package.json", r#"    "postinstall": "node setup.js","#),
            (
                "web/package.json",
                r#"    "left-pad": "git+https://example.com/left-pad.git","#,
            ),
            ("package.json", r#"    "util": "file:../util","#),
            (
                "Cargo.toml",
                r#"serde = { git = "https://example.com/serde" }"#,
            ),
            ("go.mod", "replace example.com/a => ../a"),
            (
                "requirements.txt",
                "--extra-index-url https://pypi.example.com/simple",
            ),
            (
                "requirements-dev.txt",
                "-e git+https://example.com/pkg.git#egg=pkg",
            ),
            (
                "pyproject.toml",
                r#"pkg = { git = "https://example.com/pkg" }"#,
            ),
        ] {
            assert!(outside_registry(path, line), "{path}: {line}");
        }
        for (path, line) in [
            ("package.json", r#"  "homepage": "https://example.com","#),
            (
                "package.json",
                r#"    "url": "git+https://example.com/repo.git""#,
            ),
            ("package.json", r#"    "lodash": "^4.17.21","#),
            ("package.json", r#"    "build": "tsc","#),
            (
                "Cargo.toml",
                r#"serde = { version = "1", features = ["derive"] }"#,
            ),
            ("requirements.txt", "# --index-url https://pypi.example.com"),
            ("requirements.txt", "requests==2.32.3"),
            (
                "package-lock.json",
                r#"  "resolved": "https://registry.npmjs.org/a.tgz","#,
            ),
        ] {
            assert!(!outside_registry(path, line), "{path}: {line}");
        }
    }

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
