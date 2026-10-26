//! `cargo … --message-format=json`: one JSON object per line; compiler messages carry the
//! level, the rustc/clippy code and the primary span.

use serde_json::Value;
use sych_core::{CiDiagnostic, CiFacts};

use super::MAX_RECORDS;

pub fn parse(text: &str, facts: &mut CiFacts) {
    let mut dropped = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            facts
                .limitations
                .push("a line is not JSON and was skipped".into());
            continue;
        };
        match message["reason"].as_str() {
            Some("build-finished") => facts.build_success = message["success"].as_bool(),
            Some("compiler-message") => {
                let m = &message["message"];
                let level = m["level"].as_str().unwrap_or_default();
                if !matches!(level, "error" | "warning") {
                    continue;
                }

                let Some(span) = m["spans"]
                    .as_array()
                    .and_then(|spans| spans.iter().find(|s| s["is_primary"] == true))
                else {
                    continue;
                };
                let path = span["file_name"].as_str().unwrap_or_default();
                // Dependency and toolchain sources are outside the snapshot.
                if path.starts_with('/') || path.contains("/.cargo/") {
                    continue;
                }

                if facts.diagnostics.len() == MAX_RECORDS {
                    dropped += 1;
                    continue;
                }

                facts.diagnostics.push(CiDiagnostic {
                    code_ref: None,
                    path: path.to_owned(),
                    line: span["line_start"]
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .unwrap_or(1),
                    level: level.to_owned(),
                    code: m["code"]["code"].as_str().map(str::to_owned),
                    message: m["message"].as_str().unwrap_or_default().to_owned(),
                });
            }
            _ => {}
        }
    }
    if dropped > 0 {
        facts
            .limitations
            .push(format!("{dropped} diagnostics over the limit were dropped"));
    }
}

#[cfg(test)]
mod tests {
    use sych_core::{CiFormat, SourceSnapshotId, sha256};

    use super::*;

    #[test]
    fn primary_spans_of_errors_and_warnings_are_kept() {
        let report = r#"{"reason":"compiler-artifact","target":{"name":"x"}}
{"reason":"compiler-message","message":{"level":"warning","code":{"code":"clippy::unwrap_used"},"message":"used `unwrap()`","spans":[{"file_name":"src/lib.rs","line_start":7,"line_end":7,"is_primary":true}]}}
{"reason":"compiler-message","message":{"level":"note","code":null,"message":"n","spans":[]}}
{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0308"},"message":"mismatched types","spans":[{"file_name":"/home/u/.cargo/registry/src/x/lib.rs","line_start":1,"line_end":1,"is_primary":true}]}}
{"reason":"build-finished","success":false}
"#;
        let mut facts = CiFacts::new(
            SourceSnapshotId(sha256(b"s")),
            CiFormat::CargoJson,
            "clippy.json".into(),
        );
        parse(report, &mut facts);
        assert_eq!(facts.build_success, Some(false));
        assert_eq!(
            facts.diagnostics.len(),
            1,
            "notes and dependency spans are dropped"
        );
        let d = &facts.diagnostics[0];
        assert_eq!(
            (d.path.as_str(), d.line, d.code.as_deref()),
            ("src/lib.rs", 7, Some("clippy::unwrap_used"))
        );
    }
}
