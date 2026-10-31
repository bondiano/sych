//! Coverage reports (LCOV, Cobertura XML): which lines ran. Execution only; a covered line
//! says nothing about whether an assertion checked it.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::Event;
use sych_core::{CiCoverage, CiFacts};

use super::junit::attr;
use super::{MAX_DEPTH, MAX_RECORDS, malformed};
use crate::ci::CiError;

/// Executed lines kept per file.
const MAX_LINES: usize = 20_000;

fn add(files: &mut BTreeMap<String, CiCoverage>, path: &str, line: u32, hits: u64) {
    let entry = files.entry(path.to_owned()).or_insert_with(|| CiCoverage {
        path: path.to_owned(),
        lines_found: 0,
        lines_hit: 0,
        hit: Vec::new(),
    });
    entry.lines_found += 1;
    if hits > 0 {
        entry.lines_hit += 1;
        if entry.hit.len() < MAX_LINES {
            entry.hit.push(line);
        }
    }
}

fn store(facts: &mut CiFacts, files: BTreeMap<String, CiCoverage>) {
    let total = files.len();
    facts.coverage = files
        .into_values()
        .filter(|f| !f.path.starts_with('/') && !f.path.contains(".."))
        .take(MAX_RECORDS)
        .map(|mut f| {
            f.hit.sort_unstable();
            f.hit.dedup();
            f
        })
        .collect();
    if total > facts.coverage.len() {
        facts.limitations.push(format!(
            "{} coverage files outside the repository or over the limit were dropped",
            total - facts.coverage.len()
        ));
    }
}

pub fn lcov(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut files = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("SF:") {
            current = Some(path.trim().trim_start_matches("./").to_owned());
        } else if let Some(record) = line.strip_prefix("DA:") {
            let path = current
                .as_deref()
                .ok_or_else(|| malformed(facts, &"DA before SF"))?;
            let mut parts = record.split(',');
            let (Some(Ok(number)), Some(Ok(hits))) = (
                parts.next().map(str::parse::<u32>),
                parts.next().map(|h| h.trim().parse::<u64>()),
            ) else {
                return Err(malformed(facts, &format!("bad DA record `{record}`")));
            };
            add(&mut files, path, number, hits);
        } else if line == "end_of_record" {
            current = None;
        }
    }
    store(facts, files);
    Ok(())
}

pub fn cobertura(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut reader = Reader::from_str(text);
    let mut files = BTreeMap::new();
    let (mut file, mut depth) = (None::<String>, 0usize);
    loop {
        match reader.read_event().map_err(|e| malformed(facts, &e))? {
            Event::Eof => break,
            Event::Start(e) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(malformed(facts, &"XML nested too deep"));
                }

                if e.local_name().as_ref() == "class" {
                    file = attr(&e, "filename");
                }
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Empty(e) if e.local_name().as_ref() == "line" => {
                let (Some(path), Some(Ok(number)), Some(Ok(hits))) = (
                    file.as_deref(),
                    attr(&e, "number").map(|n| n.parse::<u32>()),
                    attr(&e, "hits").map(|h| h.parse::<u64>()),
                ) else {
                    continue;
                };
                add(&mut files, path, number, hits);
            }
            _ => {}
        }
    }
    store(facts, files);
    Ok(())
}
