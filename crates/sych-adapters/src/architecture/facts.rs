//! The facts of architecture review: the checks on each side of each project the change
//! touches, told apart against the diff base, with how complete each check is.

use serde_json::json;
use sych_core::architecture::{
    ArchitectureFacts, ArchitectureFinding, Check, CheckCoverage, FixedFinding, Limits, check,
    compare, draw,
};
use sych_core::config::ArchitectureSettings;
use sych_core::{
    BackendStatus, Completeness, Language, Producer, SchemaVersion, SourceSide, canonical_sha256,
};

use super::{Analysis, Built, collect};
use crate::git::LocalSnapshot;
use crate::semantic::Options;
use crate::tools;

const CHECKS: [Check; 6] = [
    Check::ModuleCycle,
    Check::Tangle,
    Check::FieldLeak,
    Check::ShallowModule,
    Check::WideDirectory,
    Check::LongFile,
];

/// One document per project the change touches, but those in labelled `fixture_paths`;
/// nothing when the trusted policy turns architecture review off.
pub fn facts(
    snapshot: &LocalSnapshot,
    options: &Options<'_>,
    settings: &ArchitectureSettings,
    fixture_paths: &[String],
) -> Vec<ArchitectureFacts> {
    if !settings.enabled {
        return Vec::new();
    }

    collect(snapshot, options, fixture_paths)
        .into_iter()
        .map(|analysis| project(snapshot, analysis, &settings.limits))
        .collect()
}

fn project(snapshot: &LocalSnapshot, analysis: Analysis, limits: &Limits) -> ArchitectureFacts {
    let mut facts = ArchitectureFacts {
        schema_version: SchemaVersion,
        source_snapshot_id: snapshot.snapshot.id.clone(),
        producer: producer(analysis.language, limits),
        language: analysis.language,
        project: analysis.dir.clone(),
        status: BackendStatus::Ready,
        base_checked: false,
        limits: *limits,
        coverage: Vec::new(),
        findings: Vec::new(),
        fixed: Vec::new(),
        limitations: analysis.limitations,
    };
    let head = match analysis.head {
        Ok(built) => built,
        Err((status, why)) => {
            facts.status = status;
            facts.coverage = CHECKS
                .map(|check| CheckCoverage {
                    check,
                    completeness: Completeness::Unknown,
                    reason: Some(format!("not run: {why}")),
                })
                .into();
            facts.limitations.push(why);
            return facts;
        }
    };
    let base = match analysis.base {
        // The base has no such project: all it has on head is new.
        None => Some(Vec::new()),
        Some(Ok(built)) => Some(check(&built.model, limits)),
        Some(Err((_, why))) => {
            facts.limitations.push(format!(
                "the diff base was not checked ({why}): no finding is told new or old"
            ));
            None
        }
    };
    facts.base_checked = base.is_some();
    facts.coverage = coverage(&head);
    let head_path = |base: &str| head_path(snapshot, base);
    let compared = compare(base.as_deref(), check(&head.model, limits), &head_path);
    facts.findings = compared
        .found
        .into_iter()
        .map(|found| {
            let d = found.diagnostic;
            let code_ref = d.location.as_ref().and_then(|l| {
                let path = l.file.parse().ok()?;
                snapshot
                    .code_ref(SourceSide::Head, &path, l.line, l.line)
                    .ok()
            });
            ArchitectureFinding {
                check: d.check,
                picture: draw(&d.picture),
                key: d.key,
                message: d.message,
                notes: d.notes,
                size: d.size,
                change: found.change,
                base_size: found.base_size,
                code_ref,
            }
        })
        .collect();
    facts.fixed = compared
        .fixed
        .into_iter()
        .map(|d| FixedFinding {
            check: d.check,
            key: d.key,
            message: d.message,
        })
        .collect();
    tracing::info!(
        language = ?facts.language,
        project = %facts.project,
        findings = facts.findings.len(),
        fixed = facts.fixed.len(),
        "architecture facts collected"
    );
    facts
}

/// What each check proves of this model. Layout is read from the files themselves; the
/// semantic checks see what the index resolves, so what it cannot see leaves them partial: a
/// finding stands, the absence of one is not proven.
fn coverage(head: &Built) -> Vec<CheckCoverage> {
    let mut gaps: Vec<String> = head.unknowns.iter().map(|u| (*u).to_owned()).collect();
    if !head.unparsed.is_empty() {
        gaps.push(format!(
            "{} indexed files did not parse: {}",
            head.unparsed.len(),
            head.unparsed
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if head.unresolved > 0 {
        gaps.push(format!(
            "{} of {} member names are not resolved by the index though the project has a \
             field or function so named",
            head.unresolved, head.members
        ));
    }

    let reason = (!gaps.is_empty()).then(|| gaps.join("; "));
    CHECKS
        .map(|check| {
            let layout = matches!(check, Check::WideDirectory | Check::LongFile);
            let reason = if layout { None } else { reason.clone() };
            CheckCoverage {
                check,
                completeness: if reason.is_some() {
                    Completeness::Partial
                } else {
                    Completeness::CompleteForQuery
                },
                reason,
            }
        })
        .into()
}

/// Where a diff-base path is on head: a renamed file keeps its findings.
fn head_path(snapshot: &LocalSnapshot, base: &str) -> String {
    snapshot
        .manifest
        .files
        .iter()
        .find(|f| f.old.as_ref().is_some_and(|s| s.path.as_str() == base))
        .and_then(|f| f.new.as_ref())
        .map_or_else(|| base.to_owned(), |s| s.path.as_str().to_owned())
}

fn producer(language: Language, limits: &Limits) -> Producer {
    let tool = super::indexer(language);
    Producer {
        name: tool.into(),
        version: tools::sole_pinned(tool).map_or_else(|_| "unpinned".into(), |t| t.version.clone()),
        config_hash: canonical_sha256(&json!(limits)).ok(),
    }
}
