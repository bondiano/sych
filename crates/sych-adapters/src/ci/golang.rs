//! Go reports: `go test -json` (test2json events, with `-race` reports in
//! their output), `go test -coverprofile` and `go vet -json`. Race and coverage show only
//! the paths the tests executed.

use std::collections::BTreeMap;

use serde_json::Value;
use sych_core::{CiCoverage, CiDiagnostic, CiFacts, CiTest, TestStatus};

use super::{CiError, MAX_RECORDS, malformed};

const RACE: &str = "WARNING: DATA RACE";

/// `pkg/file.go`, 42 of a frame line such as `      /src/pkg/file.go:42 +0x1d` or a
/// position `pkg/file.go:42:7`.
fn frame(line: &str) -> Option<(String, u32)> {
    let token = line.split_whitespace().next()?;
    let at = token.find(".go:")? + 3;
    let number = token[at + 1..].split(':').next()?.parse().ok()?;
    Some((token[..at].to_owned(), number))
}

/// Test outcomes per package and test, and one diagnostic per data race with its first frame.
pub fn test_json(report: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut outcomes: BTreeMap<(String, String), (TestStatus, Vec<String>)> = BTreeMap::new();
    let mut race: Option<(String, Option<(String, u32)>)> = None;
    let mut races = Vec::new();
    for line in report.lines().filter(|l| l.trim_start().starts_with('{')) {
        let event: Value = serde_json::from_str(line).map_err(|e| malformed(facts, &e))?;
        let package = event["Package"].as_str().unwrap_or_default().to_owned();
        let test = event["Test"].as_str().unwrap_or_default().to_owned();
        let output = event["Output"].as_str().unwrap_or_default();
        if output.contains(RACE) {
            races.extend(race.take());
            race = Some((format!("{package} {test}").trim().to_owned(), None));
        } else if let Some((_, at @ None)) = &mut race {
            *at = frame(output);
        }

        let status = match event["Action"].as_str() {
            Some("pass") => TestStatus::Passed,
            Some("fail") => TestStatus::Failed,
            Some("skip") => TestStatus::Skipped,
            Some("output") if !test.is_empty() => {
                if let Some((_, lines)) = outcomes.get_mut(&(package, test)) {
                    lines.push(output.trim_end().to_owned());
                }

                continue;
            }
            Some("run") if !test.is_empty() => {
                outcomes.insert((package, test), (TestStatus::Errored, Vec::new()));
                continue;
            }
            _ => continue,
        };
        if !test.is_empty() {
            let entry = outcomes
                .entry((package, test))
                .or_insert((status, Vec::new()));
            entry.0 = status;
        }
    }
    races.extend(race);
    let total = outcomes.len();
    facts.tests = outcomes
        .into_iter()
        .take(MAX_RECORDS)
        .map(|((suite, name), (status, output))| CiTest {
            name,
            suite,
            file: None,
            message: (status == TestStatus::Failed).then(|| {
                output
                    .iter()
                    .rev()
                    .take(5)
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
            status,
        })
        .collect();
    if total > facts.tests.len() {
        facts.limitations.push(format!(
            "{} tests over the limit were dropped",
            total - facts.tests.len()
        ));
    }

    for (test, at) in races.into_iter().take(MAX_RECORDS) {
        let (path, line) = at.unwrap_or_default();
        facts.diagnostics.push(CiDiagnostic {
            code_ref: None,
            path,
            line,
            level: "error".into(),
            code: Some("race".into()),
            message: format!("data race detected in {test}"),
        });
    }
    facts.limitations.push(
        "the race detector and test outcomes cover only executed paths: an unreported race is \
         not absent"
            .into(),
    );
    Ok(())
}

/// `mode: set` then `pkg/file.go:12.3,14.2 2 1`: statements per block and how often it ran.
/// Paths are import paths; `module` (the root `go.mod` module) is stripped to repo paths.
pub fn cover(text: &str, module: Option<&str>, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut files: BTreeMap<String, CiCoverage> = BTreeMap::new();
    let mut foreign = 0;
    for record in text.lines().skip(1).filter(|l| !l.trim().is_empty()) {
        let parse = || -> Option<(&str, u32, u32, u64)> {
            let (file, rest) = record.rsplit_once(':')?;
            let mut parts = rest.split_whitespace();
            let (start, end) = parts.next()?.split_once(',')?;
            let start = start.split('.').next()?.parse().ok()?;
            let end = end.split('.').next()?.parse().ok()?;
            let _statements = parts.next()?;
            Some((file, start, end, parts.next()?.parse().ok()?))
        };
        let Some((file, start, end, count)) = parse() else {
            return Err(malformed(facts, &format!("bad cover record `{record}`")));
        };
        let Some(path) = module
            .and_then(|m| file.strip_prefix(m))
            .map(|p| p.trim_start_matches('/').to_owned())
        else {
            foreign += 1;
            continue;
        };
        let entry = files.entry(path.clone()).or_insert_with(|| CiCoverage {
            path,
            lines_found: 0,
            lines_hit: 0,
            hit: Vec::new(),
        });
        let lines = end.saturating_sub(start) + 1;
        entry.lines_found += lines;
        if count > 0 {
            entry.lines_hit += lines;
            entry.hit.extend(start..=end);
        }
    }
    if foreign > 0 {
        facts.limitations.push(format!(
            "{foreign} cover blocks outside the root module {} were not mapped",
            module.unwrap_or("(no go.mod)")
        ));
    }

    facts.coverage = files
        .into_values()
        .take(MAX_RECORDS)
        .map(|mut f| {
            f.hit.sort_unstable();
            f.hit.dedup();
            f.lines_hit = f.lines_hit.min(f.lines_found);
            f
        })
        .collect();
    Ok(())
}

/// `go vet -json`: `# pkg` headers between `{"pkg": {"analyzer": [{"posn", "message"}]}}`
/// objects. Positions are absolute paths of the CI checkout, matched to the repo by suffix.
pub fn vet_json(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let json: String = text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let stream = serde_json::Deserializer::from_str(&json).into_iter::<Value>();
    for object in stream {
        let object = object.map_err(|e| malformed(facts, &e))?;
        for (analyzer, findings) in object
            .as_object()
            .into_iter()
            .flat_map(|o| o.values())
            .filter_map(Value::as_object)
            .flatten()
        {
            for finding in findings.as_array().into_iter().flatten() {
                if facts.diagnostics.len() == MAX_RECORDS {
                    break;
                }

                let posn = finding["posn"].as_str().unwrap_or_default();
                let (path, line) = frame(posn).unwrap_or_default();
                facts.diagnostics.push(CiDiagnostic {
                    code_ref: None,
                    path,
                    line,
                    level: "warning".into(),
                    code: Some(format!("vet::{analyzer}")),
                    message: finding["message"].as_str().unwrap_or_default().to_owned(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sych_core::{CiFormat, SourceSnapshotId, sha256};

    fn facts(format: CiFormat) -> CiFacts {
        CiFacts::new(SourceSnapshotId(sha256(b"go")), format, "r".into())
    }

    #[test]
    fn test_events_outcomes_and_races() {
        let text = "{\"Action\":\"run\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\"}\n\
            {\"Action\":\"output\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\",\"Output\":\"==================\\n\"}\n\
            {\"Action\":\"output\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\",\"Output\":\"WARNING: DATA RACE\\n\"}\n\
            {\"Action\":\"output\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\",\"Output\":\"Write at 0x00c by goroutine 7:\\n\"}\n\
            {\"Action\":\"output\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\",\"Output\":\"      /ci/src/orders/create.go:42 +0x1d\\n\"}\n\
            {\"Action\":\"fail\",\"Package\":\"ex/orders\",\"Test\":\"TestSave\"}\n\
            {\"Action\":\"skip\",\"Package\":\"ex/orders\",\"Test\":\"TestSlow\"}\n\
            {\"Action\":\"pass\",\"Package\":\"ex/orders\"}\n";
        let mut f = facts(CiFormat::GoTestJson);
        test_json(text, &mut f).expect("parses");
        assert_eq!(f.tests.len(), 2);
        assert_eq!(f.tests[0].status, TestStatus::Failed);
        assert_eq!(f.tests[1].status, TestStatus::Skipped);
        assert_eq!(f.diagnostics[0].path, "/ci/src/orders/create.go");
        assert_eq!(f.diagnostics[0].line, 42);
        assert_eq!(f.diagnostics[0].code.as_deref(), Some("race"));
    }

    #[test]
    fn cover_profile_maps_import_paths_into_the_module() {
        let text = "mode: set\nexample.com/svc/orders/create.go:10.2,12.3 2 1\n\
            example.com/svc/orders/create.go:14.2,15.3 1 0\nother.org/x/y.go:1.1,2.2 1 1\n";
        let mut f = facts(CiFormat::GoCover);
        cover(text, Some("example.com/svc"), &mut f).expect("parses");
        assert_eq!(f.coverage[0].path, "orders/create.go");
        assert_eq!(f.coverage[0].hit, [10, 11, 12]);
        assert_eq!((f.coverage[0].lines_found, f.coverage[0].lines_hit), (5, 3));
        assert!(f.limitations[0].contains("1 cover blocks"));
    }

    #[test]
    fn vet_findings_per_analyzer() {
        let text = "# example.com/svc/orders\n{\n\t\"example.com/svc/orders\": {\n\t\t\"lostcancel\": [\n\t\t\t{\"posn\": \"/ci/orders/create.go:9:2\", \"message\": \"the cancel function is not used\"}\n\t\t]\n\t}\n}\n";
        let mut f = facts(CiFormat::GoVetJson);
        vet_json(text, &mut f).expect("parses");
        assert_eq!(f.diagnostics[0].code.as_deref(), Some("vet::lostcancel"));
        assert_eq!(f.diagnostics[0].line, 9);
    }
}
