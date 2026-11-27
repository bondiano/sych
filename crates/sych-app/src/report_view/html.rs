//! Self-contained offline HTML report built from [`ReportView`] and the embedded web UI bundle.

use sych_core::ReportView;

use crate::render;

/// Marker inside `template.gen.html` replaced with the `<`-escaped JSON payload.
pub const PLACEHOLDER: &str = "<!--SYCH_REPORT_JSON-->";

/// Committed single-file bundle produced by `just report-template`.
pub const TEMPLATE: &str = include_str!("template.gen.html");

/// Injects `view` into [`TEMPLATE`], writing every `<` as `\u003c` inside
/// `<script type="application/json">` so repository or model text cannot break out of the tag.
pub fn render(view: &ReportView) -> Result<String, serde_json::Error> {
    let raw = render::json(view)?;
    let payload = escape_script_json(raw.trim_end());
    let (before, after) = TEMPLATE
        .split_once(PLACEHOLDER)
        .expect("embedded HTML report template contains the JSON placeholder");
    let mut out = String::with_capacity(TEMPLATE.len() + payload.len());
    out.push_str(before);
    out.push_str(&payload);
    out.push_str(after);
    Ok(out)
}

fn escape_script_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len() + 32);
    for ch in json.chars() {
        match ch {
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_script_json;

    #[test]
    fn script_json_escapes_every_opening_angle_bracket_and_stays_valid_json() {
        let raw = "{\"evil\":\"</script><img src=x onerror=alert(1)><!--\"}";
        let escaped = escape_script_json(raw);
        assert!(!escaped.contains('<'), "{escaped}");
        let parsed: serde_json::Value = serde_json::from_str(&escaped).expect("valid JSON");
        assert_eq!(parsed["evil"], "</script><img src=x onerror=alert(1)><!--");
    }
}
