//! `JUnit` XML (cargo-nextest, vitest, Jest): `testsuite`/`testcase` with `failure`, `error`
//! or `skipped` children.

use quick_xml::events::{BytesStart, Event};
use std::borrow::Cow;

use quick_xml::{Reader, XmlVersion};
use sych_core::{CiFacts, CiTest, TestStatus};

use super::{CiError, MAX_DEPTH, MAX_RECORDS, malformed};

pub(super) fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(Cow::into_owned)
        })
}

pub fn parse(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut reader = Reader::from_str(text);
    let mut suite = String::new();
    let mut current: Option<CiTest> = None;
    let (mut dropped, mut depth) = (0, 0usize);
    let mut finish = |test: CiTest, facts: &mut CiFacts| {
        if facts.tests.len() == MAX_RECORDS {
            dropped += 1;
        } else {
            facts.tests.push(test);
        }
    };
    loop {
        let event = reader.read_event().map_err(|source| CiError::Junit {
            report: facts.report.clone(),
            source,
        })?;
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth > MAX_DEPTH {
            return Err(malformed(facts, &"XML nested too deep"));
        }

        match event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == "testsuite" => {
                suite = attr(&e, "name").unwrap_or_default();
            }
            Event::Start(e) if e.local_name().as_ref() == "testcase" => {
                current = Some(test_case(&e, &suite));
            }
            Event::Empty(e) if e.local_name().as_ref() == "testcase" => {
                finish(test_case(&e, &suite), facts);
            }
            Event::Start(e) | Event::Empty(e) => {
                if let Some(test) = current.as_mut() {
                    let status = match e.local_name().as_ref() {
                        "failure" => Some(TestStatus::Failed),
                        "error" => Some(TestStatus::Errored),
                        "skipped" => Some(TestStatus::Skipped),
                        _ => None,
                    };
                    if let Some(status) = status {
                        test.status = status;
                        test.message = attr(&e, "message").map(|m| m.chars().take(300).collect());
                    }
                }
            }
            Event::End(e) if e.local_name().as_ref() == "testcase" => {
                if let Some(test) = current.take() {
                    finish(test, facts);
                }
            }
            _ => {}
        }
    }
    if dropped > 0 {
        facts
            .limitations
            .push(format!("{dropped} tests over the limit were dropped"));
    }

    Ok(())
}

fn test_case(e: &BytesStart<'_>, suite: &str) -> CiTest {
    CiTest {
        name: attr(e, "name").unwrap_or_default(),
        suite: attr(e, "classname").unwrap_or_else(|| suite.to_owned()),
        file: attr(e, "file"),
        status: TestStatus::Passed,
        message: None,
    }
}

#[cfg(test)]
mod tests {
    use sych_core::{CiFormat, SourceSnapshotId, sha256};

    use super::*;

    #[test]
    fn statuses_come_from_children() {
        let report = r#"<?xml version="1.0"?>
<testsuites><testsuite name="core" tests="4">
  <testcase name="saves" classname="store" file="src/store.test.ts"/>
  <testcase name="publishes" classname="store"><failure message="expected 1 &amp; got 0">trace</failure></testcase>
  <testcase name="later" classname="store"><skipped/></testcase>
  <testcase name="boom"><error message="panic"/></testcase>
</testsuite></testsuites>"#;
        let mut facts = CiFacts::new(
            SourceSnapshotId(sha256(b"s")),
            CiFormat::Junit,
            "junit.xml".into(),
        );
        parse(report, &mut facts).expect("valid");
        let got: Vec<_> = facts
            .tests
            .iter()
            .map(|t| (t.name.as_str(), t.status))
            .collect();
        assert_eq!(
            got,
            [
                ("saves", TestStatus::Passed),
                ("publishes", TestStatus::Failed),
                ("later", TestStatus::Skipped),
                ("boom", TestStatus::Errored),
            ]
        );
        assert_eq!(
            facts.tests[1].message.as_deref(),
            Some("expected 1 & got 0")
        );
        assert_eq!(facts.tests[0].file.as_deref(), Some("src/store.test.ts"));
        assert_eq!(facts.tests[3].suite, "core");
        assert!(parse("<testsuites><testcase", &mut facts).is_err());
    }
}
