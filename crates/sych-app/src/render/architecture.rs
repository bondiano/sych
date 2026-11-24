//! Architecture review in a Markdown report: each project's checks, and a finding's picture.

use std::fmt::Write;

use sych_core::architecture::{ArchitectureFacts, PICTURE};

use super::{md, tag};

/// Each project's architecture checks: whether they and the diff base ran, what is partial and
/// why, and every finding by what the change did to it, existing debt and fixed ones too.
pub(super) fn architecture(out: &mut String, facts: &[ArchitectureFacts]) {
    if facts.is_empty() {
        return;
    }

    out.push_str("\n## Architecture\n\n");
    for project in facts {
        let dir = if project.project.is_empty() {
            "."
        } else {
            &project.project
        };
        let _ = writeln!(
            out,
            "- {} project {}: {}, diff base {}, by {} {}",
            md(&format!("{:?}", project.language)),
            md(dir),
            tag(&project.status),
            if project.base_checked {
                "checked"
            } else {
                "not checked"
            },
            md(&project.producer.name),
            md(&project.producer.version)
        );
        for coverage in project.coverage.iter().filter(|c| c.reason.is_some()) {
            let _ = writeln!(
                out,
                "  - {} {}: {}",
                coverage.check.name(),
                tag(&coverage.completeness),
                md(coverage.reason.as_deref().unwrap_or_default())
            );
        }
        for f in &project.findings {
            let change = f.change.map_or_else(|| "UNKNOWN".to_owned(), |c| tag(&c));
            let at = f
                .code_ref
                .as_ref()
                .map(|r| format!(" at {}:{}", md(r.path.as_str()), r.start_line))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "  - {change} {}: {}{at}",
                f.check.name(),
                md(&f.message)
            );
        }
        for f in &project.fixed {
            let _ = writeln!(out, "  - FIXED {}: {}", f.check.name(), md(&f.message));
        }
        for l in &project.limitations {
            let _ = writeln!(out, "  - limitation: {}", md(l));
        }
    }
}

/// A finding's explanation as one list item; an architecture finding's picture, drawn by its
/// check and not by a model, follows it as a code block that no backtick of it can close.
pub(super) fn explanation(out: &mut String, text: &str) {
    let (prose, picture) = text.split_once(PICTURE).map_or((text, None), |(p, rest)| {
        (p, Some(rest.trim_end_matches('`')))
    });
    let _ = writeln!(out, "  - {}", md(prose));
    if let Some(picture) = picture {
        out.push_str("\n    ```text\n");
        for line in picture.lines() {
            let _ = writeln!(out, "    {}", line.replace('`', "'"));
        }
        out.push_str("    ```\n\n");
    }
}

#[cfg(test)]
mod tests {
    use super::explanation;

    #[test]
    fn a_picture_after_the_explanation_is_a_code_block_no_backtick_can_close() {
        let mut out = String::new();
        explanation(
            &mut out,
            "Two `modules` use each other.\n\n```text\n  a``` b\n  c\n```",
        );
        assert_eq!(
            out,
            "  - Two \\`modules\\` use each other.\n\n    ```text\n      a''' b\n      c\n    ```\n\n"
        );
        let mut plain = String::new();
        explanation(&mut plain, "one\nline");
        assert_eq!(plain, "  - one line\n");
    }
}
