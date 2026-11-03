//! CSV exports of the list reads: a header line, RFC 4180 quoting and CRLF line ends. A text
//! cell a spreadsheet would run as a formula gets a leading `'`.

use super::views::{AuditEvent, FindingSummary, UsageDay};
use crate::Timestamp;

/// A text cell, quoted when it holds a separator, a quote or a line break.
fn cell(text: &str) -> String {
    // A leading `=`, `+`, `-`, `@`, tab or CR makes a spreadsheet evaluate the cell.
    let text = if text.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{text}")
    } else {
        text.to_owned()
    };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

fn optional(text: Option<&str>) -> String {
    text.map(cell).unwrap_or_default()
}

/// RFC 3339 in UTC; spreadsheets read it as a date.
fn time(at: i64) -> String {
    Timestamp::from_second(at).map_or_else(|_| at.to_string(), |t| t.to_string())
}

fn table(header: &[&str], rows: impl Iterator<Item = Vec<String>>) -> String {
    let mut out = header.join(",");
    out.push_str("\r\n");
    for row in rows {
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

/// `GET /v1/findings?format=csv`.
#[must_use]
pub fn findings_csv(items: &[FindingSummary]) -> String {
    let header = [
        "id",
        "repository",
        "number",
        "change_id",
        "finding_id",
        "state",
        "severity",
        "category",
        "rule_id",
        "title",
        "path",
        "first_run_id",
        "last_run_id",
        "updated_at",
    ];
    table(
        &header,
        items.iter().map(|f| {
            vec![
                f.id.to_string(),
                cell(&f.repository.full_name),
                f.number.map(|n| n.to_string()).unwrap_or_default(),
                f.change_id.to_string(),
                cell(&f.finding_id),
                cell(&f.state),
                optional(f.severity.as_deref()),
                cell(&f.category),
                optional(f.rule_id.as_deref()),
                optional(f.title.as_deref()),
                optional(f.path.as_deref()),
                cell(&f.first_run_id),
                cell(&f.last_run_id),
                time(f.updated_at),
            ]
        }),
    )
}

/// `GET /v1/usage?format=csv`: the days; subscription fees are monthly and stay in the JSON.
#[must_use]
pub fn usage_csv(days: &[UsageDay]) -> String {
    let header = [
        "day",
        "attempts",
        "cost_known_micro_usd",
        "unknown_cost_attempts",
        "input_tokens",
        "output_tokens",
    ];
    table(
        &header,
        days.iter().map(|d| {
            vec![
                cell(&d.day),
                d.attempts.to_string(),
                d.cost_known_micro_usd.to_string(),
                d.unknown_cost_attempts.to_string(),
                d.input_tokens.to_string(),
                d.output_tokens.to_string(),
            ]
        }),
    )
}

/// `GET /v1/audit?format=csv`: metadata as its JSON text.
#[must_use]
pub fn audit_csv(events: &[AuditEvent]) -> String {
    let header = [
        "id",
        "at",
        "actor",
        "action",
        "target",
        "repository_id",
        "metadata",
    ];
    table(
        &header,
        events.iter().map(|e| {
            vec![
                e.id.to_string(),
                time(e.at),
                cell(&e.actor),
                cell(&e.action),
                cell(&e.target),
                e.repository_id.map(|id| id.to_string()).unwrap_or_default(),
                cell(&e.metadata.to_string()),
            ]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_quoted_and_formulas_disarmed() {
        assert_eq!(cell("plain"), "plain");
        assert_eq!(cell("a,b"), "\"a,b\"");
        assert_eq!(cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("two\nlines"), "\"two\nlines\"");
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(cell("+1"), "'+1");
        assert_eq!(cell("-rm"), "'-rm");
        assert_eq!(cell("@SUM"), "'@SUM");
        assert_eq!(cell("\tx"), "'\tx");
    }

    #[test]
    fn audit_rows_keep_numbers_and_json() {
        let events = [AuditEvent {
            id: 7,
            actor: "github:octo".into(),
            action: "WAIVER_CREATED".into(),
            target: "waiver:3".into(),
            repository_id: None,
            metadata: serde_json::json!({"reason": "x, y"}),
            at: 0,
        }];
        assert_eq!(
            audit_csv(&events),
            "id,at,actor,action,target,repository_id,metadata\r\n\
             7,1970-01-01T00:00:00Z,github:octo,WAIVER_CREATED,waiver:3,,\"{\"\"reason\"\":\"\"x, y\"\"}\"\r\n"
        );
    }
}
