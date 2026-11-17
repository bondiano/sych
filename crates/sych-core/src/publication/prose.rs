//! Model-written text in GitHub Markdown renders as the words it says: never as HTML, an image,
//! a link or a mention. A prompt injection in the reviewed code could otherwise make the bot post
//! an image whose URL carries private code (GitHub fetches it through its proxy), a forged
//! `sych:` marker or a ping. Code spans stay as written, since GitHub shows them verbatim.

use std::fmt::Write;

/// `text` with HTML, link and image syntax, entities, mentions and control characters made
/// inert outside code spans. Each line is read on its own: a block such as `<details>` only
/// starts a line, and a code span the model opens on one line cannot shield the next.
pub(crate) fn prose(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }

        line_prose(line, &mut out);
    }
    out
}

fn line_prose(mut rest: &str, out: &mut String) {
    while let Some(open) = rest.find('`') {
        let run = backticks(&rest[open..]);
        let after = &rest[open + run..];
        escape(&rest[..open], out);
        // A span closes on a run of the same length; an unmatched run is literal text. Every
        // backslash outside spans is doubled, so GitHub sees the same runs as this loop.
        if let Some(at) = closing(after, run) {
            out.push_str(&rest[open..open + run + at + run]);
            rest = &after[at + run..];
        } else {
            out.push_str(&rest[open..open + run]);
            rest = after;
        }
    }
    escape(rest, out);
}

fn backticks(text: &str) -> usize {
    text.len() - text.trim_start_matches('`').len()
}

/// The offset of the first run of exactly `n` backticks in `text`.
fn closing(text: &str, n: usize) -> Option<usize> {
    let mut at = 0;
    while let Some(i) = text[at..].find('`') {
        let start = at + i;
        let len = backticks(&text[start..]);
        if len == n {
            return Some(start);
        }

        at = start + len;
    }
    None
}

fn escape(text: &str, out: &mut String) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' | '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            // A zero-width space after `@` keeps the name readable but not a mention.
            '@' if chars.peek().is_some_and(char::is_ascii_alphanumeric) => {
                out.push_str("@&#8203;");
            }
            '\r' => {}
            c if (c.is_control() && c != '\t')
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') =>
            {
                let _ = write!(out, "U+{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::prose;

    #[test]
    fn html_images_links_markers_and_mentions_are_inert() {
        let text = prose(
            "![x](https://e.example/?q=s) <img src=x> <!-- sych:inline:x --> \
             [ok](https://e.example) cc @org/team &#64;admin",
        );
        assert_eq!(
            text,
            "!\\[x\\](https://e.example/?q=s) &lt;img src=x&gt; &lt;!-- sych:inline:x --&gt; \
             \\[ok\\](https://e.example) cc @&#8203;org/team &amp;#64;admin"
        );
    }

    #[test]
    fn code_spans_are_kept_and_cannot_hide_markup() {
        assert_eq!(prose("`Vec<String>` and `a@b`"), "`Vec<String>` and `a@b`");
        assert_eq!(
            prose("``a ` b`` <i>"),
            "``a ` b`` &lt;i&gt;",
            "a span closes on a run of its own length"
        );
        assert_eq!(
            prose("`open <i>"),
            "`open &lt;i&gt;",
            "an unmatched run is text"
        );
        assert_eq!(
            prose("\\`<img>`"),
            "\\\\`<img>`",
            "the backslash is doubled, so GitHub reads the same span"
        );
        assert_eq!(
            prose("`x\n<details><img src=y>\nz`"),
            "`x\n&lt;details&gt;&lt;img src=y&gt;\nz`",
            "a span does not cross a line"
        );
    }

    #[test]
    fn control_and_bidi_characters_are_named() {
        assert_eq!(prose("a\u{202e}b\u{7}c\r\n\td"), "aU+202EbU+0007c\n\td");
    }
}
