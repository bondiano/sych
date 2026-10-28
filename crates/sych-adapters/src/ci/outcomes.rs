//! The project JSON manifest of command, test and event-assertion outcomes (`sych_manifest`): a
//! small format a workflow or a targeted check can write without a test-report plugin.

use serde::Deserialize;
use sych_core::{CiFacts, CiTest, TestStatus};

use super::{MAX_RECORDS, malformed};
use crate::ci::CiError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    #[serde(default)]
    commands: Vec<Command>,
    #[serde(default)]
    tests: Vec<CiTest>,
    #[serde(default)]
    assertions: Vec<Assertion>,
}

/// The events a check expected and the ones it recorded, in order (failure/retry/cancel traces).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Assertion {
    name: String,
    expected: Vec<String>,
    /// Missing when the harness recorded nothing: not run, never a pass.
    observed: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    name: String,
    exit_code: i32,
}

pub fn parse(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let m: Manifest = serde_json::from_str(text).map_err(|e| malformed(facts, &e))?;
    if m.schema_version != 1 {
        return Err(malformed(
            facts,
            &format!("schema_version {}", m.schema_version),
        ));
    }

    let commands = m.commands.into_iter().map(|c| CiTest {
        status: if c.exit_code == 0 {
            TestStatus::Passed
        } else {
            TestStatus::Failed
        },
        message: (c.exit_code != 0).then(|| format!("exit code {}", c.exit_code)),
        name: c.name,
        suite: "command".into(),
        file: None,
    });
    let assertions = m.assertions.into_iter().map(|a| {
        let (status, message) = match &a.observed {
            None => (TestStatus::Skipped, Some("no events recorded".to_owned())),
            Some(o) if *o == a.expected => (TestStatus::Passed, None),
            Some(o) => (
                TestStatus::Failed,
                Some(format!(
                    "expected events [{}], observed [{}]",
                    a.expected.join(", "),
                    o.join(", ")
                )),
            ),
        };
        CiTest {
            name: a.name,
            suite: "assertion".into(),
            file: None,
            status,
            message,
        }
    });
    let all: Vec<CiTest> = commands.chain(m.tests).chain(assertions).collect();
    if all.len() > MAX_RECORDS {
        facts.limitations.push(format!(
            "{} outcomes over the limit were dropped",
            all.len() - MAX_RECORDS
        ));
    }

    facts.tests = all.into_iter().take(MAX_RECORDS).collect();
    Ok(())
}
