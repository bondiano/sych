//! Static-analysis reports as diagnostics: SARIF 2.1 and GitLab Code Quality JSON. Locations
//! outside the repository (absolute paths) are dropped, never guessed.

use serde_json::Value;
use sych_core::{CiDiagnostic, CiFacts};

use super::{MAX_RECORDS, malformed};
use crate::ci::CiError;

fn relative(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let path = path.strip_prefix("./").unwrap_or(path);
    (!path.is_empty() && !path.starts_with('/') && !path.contains("..")).then(|| path.to_owned())
}

fn push(facts: &mut CiFacts, dropped: &mut usize, d: CiDiagnostic) {
    if facts.diagnostics.len() == MAX_RECORDS {
        *dropped += 1;
    } else {
        facts.diagnostics.push(d);
    }
}

fn finish(facts: &mut CiFacts, dropped: usize, outside: usize) {
    if dropped > 0 {
        facts
            .limitations
            .push(format!("{dropped} diagnostics over the limit were dropped"));
    }

    if outside > 0 {
        facts.limitations.push(format!(
            "{outside} diagnostics outside the repository were dropped"
        ));
    }
}

pub fn sarif(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let v: Value = serde_json::from_str(text).map_err(|e| malformed(facts, &e))?;
    let runs = v["runs"]
        .as_array()
        .ok_or_else(|| malformed(facts, &"no `runs` array"))?;
    let (mut dropped, mut outside) = (0, 0);
    for run in runs {
        let driver = &run["tool"]["driver"];
        if let Some(name) = driver["name"].as_str() {
            let version = driver["version"]
                .as_str()
                .or(driver["semanticVersion"].as_str());
            facts.limitations.push(format!(
                "tool {name} {}",
                version.unwrap_or("of unknown version")
            ));
        }

        for result in run["results"].as_array().into_iter().flatten() {
            let location = &result["locations"][0]["physicalLocation"];
            let Some(path) = location["artifactLocation"]["uri"]
                .as_str()
                .and_then(relative)
            else {
                outside += 1;
                continue;
            };
            let line = location["region"]["startLine"].as_u64().unwrap_or(1);
            push(
                facts,
                &mut dropped,
                CiDiagnostic {
                    code_ref: None,
                    path,
                    line: u32::try_from(line).unwrap_or(u32::MAX),
                    level: result["level"].as_str().unwrap_or("warning").to_owned(),
                    code: result["ruleId"].as_str().map(str::to_owned),
                    message: result["message"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .chars()
                        .take(300)
                        .collect(),
                },
            );
        }
    }
    finish(facts, dropped, outside);
    Ok(())
}

pub fn code_quality(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let v: Value = serde_json::from_str(text).map_err(|e| malformed(facts, &e))?;
    let issues = v
        .as_array()
        .ok_or_else(|| malformed(facts, &"not a JSON array"))?;
    let (mut dropped, mut outside) = (0, 0);
    for issue in issues {
        let location = &issue["location"];
        let Some(path) = location["path"].as_str().and_then(relative) else {
            outside += 1;
            continue;
        };
        let line = location["lines"]["begin"]
            .as_u64()
            .or(location["positions"]["begin"]["line"].as_u64())
            .unwrap_or(1);
        let level = match issue["severity"].as_str() {
            Some("blocker" | "critical" | "major") => "error",
            Some("minor") => "warning",
            _ => "note",
        };
        push(
            facts,
            &mut dropped,
            CiDiagnostic {
                code_ref: None,
                path,
                line: u32::try_from(line).unwrap_or(u32::MAX),
                level: level.into(),
                code: issue["check_name"].as_str().map(str::to_owned),
                message: issue["description"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect(),
            },
        );
    }
    finish(facts, dropped, outside);
    Ok(())
}
