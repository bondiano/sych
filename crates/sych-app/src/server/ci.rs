//! Expected CI reports of a server run (spec §15.1–15.3): the trusted selector names the exact
//! workflow, event, job, artifact and path; reports are read by provider ids for the exact head,
//! bound by a checkout manifest, and everything missing becomes a concrete gap.

use std::path::Path;
use std::time::Duration;

use sych_adapters::ci::archive::{self, Limits};
use sych_adapters::ci::import_as;
use sych_adapters::git::{Git, LocalSnapshot, fetch_commits};
use sych_adapters::github::{Artifact, Job, Secret, WorkflowRun};
use sych_core::config::{CiReportSelector, ExpectedReport, ReportFormat};
use sych_core::scm::ScmError;
use sych_core::{CiBindingKind, CiFacts, CiFormat, CiGap, CiManifest, CiOrigin, sha256};

use super::engine::Conn;

/// Artifact zips are read whole into memory; larger ones are a gap, not a partial read.
const MAX_ARTIFACT_BYTES: u64 = 64 << 20;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Gap codes that may still turn into a report before the evidence deadline.
pub const PENDING: &[&str] = &["CI_JOB_MISSING", "CI_JOB_INCOMPLETE"];

#[derive(Default)]
pub struct Collected {
    pub facts: Vec<CiFacts>,
    pub gaps: Vec<CiGap>,
}

impl Collected {
    pub fn pending(&self) -> bool {
        self.gaps.iter().any(|g| PENDING.contains(&g.code.as_str()))
    }
}

fn gap(report: &str, code: &str, detail: impl Into<String>) -> CiGap {
    CiGap {
        report: report.into(),
        code: code.into(),
        detail: detail.into(),
    }
}

/// The parser of a declared format.
const fn format(f: ReportFormat) -> CiFormat {
    match f {
        ReportFormat::CargoJson => CiFormat::CargoJson,
        ReportFormat::Junit => CiFormat::Junit,
        ReportFormat::Sarif => CiFormat::Sarif,
        ReportFormat::GitlabCodeQuality => CiFormat::GitlabCodeQuality,
        ReportFormat::Lcov => CiFormat::Lcov,
        ReportFormat::Cobertura => CiFormat::Cobertura,
        ReportFormat::SychManifest => CiFormat::SychManifest,
        ReportFormat::GoTestJson => CiFormat::GoTestJson,
        ReportFormat::GoCover => CiFormat::GoCover,
        ReportFormat::GoVetJson => CiFormat::GoVetJson,
        ReportFormat::TerraformPlan => CiFormat::TerraformPlan,
        ReportFormat::KubernetesRendered => CiFormat::KubernetesRendered,
    }
}

/// What the tested commit is to the requested head: itself, a merge of a target commit and
/// it, or unproven.
fn binding(mirror: &Path, tested: &str, head: &str) -> (CiBindingKind, Option<String>) {
    if tested == head {
        return (CiBindingKind::SourceHead, None);
    }

    let parents = Git::open(mirror)
        .and_then(|git| git.stdout(["rev-list", "--parents", "-n", "1", tested]))
        .map(|out| {
            String::from_utf8_lossy(&out)
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    match parents.as_slice() {
        [commit, target, source] if commit == tested && source == head => {
            (CiBindingKind::MergeResult, Some(target.clone()))
        }
        _ => (CiBindingKind::Unverified, None),
    }
}

/// One head of one repository whose expected reports are collected.
pub struct Head<'a> {
    pub conn: &'a Conn,
    pub repo: &'a str,
    pub sha: &'a str,
    pub token: &'a Secret,
    pub snap: &'a LocalSnapshot,
    pub mirror: &'a Path,
}

/// The exact producer of one report.
struct Located {
    run: WorkflowRun,
    job: Job,
    artifact: Artifact,
}

impl Head<'_> {
    /// Collects every expected report; never fails the run, it only adds gaps.
    pub async fn collect(&self, expected: &[ExpectedReport]) -> Collected {
        let mut out = Collected::default();
        for report in expected {
            let selector = self
                .conn
                .config
                .ci_reports
                .iter()
                .find(|s| s.report == report.id);
            let Some(selector) = selector else {
                out.gaps.push(gap(
                    &report.id,
                    "CI_NOT_MAPPED",
                    "no trusted producer is configured",
                ));
                continue;
            };
            let read = match self.locate(&report.id, selector).await {
                Ok(located) => self.read(report, selector, located).await,
                Err(g) => Err(g),
            };
            match read {
                Ok((facts, gaps)) => {
                    out.facts.push(facts);
                    out.gaps.extend(gaps);
                }
                Err(g) => out.gaps.push(g),
            }
        }
        out
    }

    /// The selector's run of this head, its job in that attempt and its artifact.
    async fn locate(&self, id: &str, selector: &CiReportSelector) -> Result<Located, CiGap> {
        let (gh, repo) = (&self.conn.gh, self.repo);
        let api = |e: ScmError| gap(id, "CI_UNAVAILABLE", e.to_string());
        let runs = gh.runs_for(repo, self.sha, self.token).await.map_err(api)?;
        let run = runs
            .into_iter()
            .filter(|r| {
                r.path == selector.workflow && r.event == selector.event && r.head_sha == self.sha
            })
            .max_by_key(|r| r.id)
            .ok_or_else(|| {
                gap(
                    id,
                    "CI_JOB_MISSING",
                    format!("no {} run of {}", selector.event, selector.workflow),
                )
            })?;
        let jobs = gh
            .jobs(repo, run.id, run.run_attempt, self.token)
            .await
            .map_err(api)?;
        let job = jobs
            .into_iter()
            .find(|j| j.name == selector.job)
            .ok_or_else(|| {
                gap(
                    id,
                    "CI_JOB_MISSING",
                    format!("run {} has no job `{}`", run.id, selector.job),
                )
            })?;
        match (job.status.as_str(), job.conclusion.as_deref()) {
            ("completed", Some("skipped")) => {
                return Err(gap(id, "CI_JOB_SKIPPED", format!("job {}", job.id)));
            }
            ("completed", Some("cancelled")) => {
                return Err(gap(id, "CI_JOB_CANCELLED", format!("job {}", job.id)));
            }
            ("completed", _) => {}
            (status, _) => {
                return Err(gap(
                    id,
                    "CI_JOB_INCOMPLETE",
                    format!("job {} is {status}", job.id),
                ));
            }
        }

        let artifacts = gh.artifacts(repo, run.id, self.token).await.map_err(api)?;
        let artifact = artifacts
            .into_iter()
            .find(|a| a.name == selector.artifact)
            .ok_or_else(|| {
                gap(
                    id,
                    "CI_ARTIFACT_MISSING",
                    format!("run {} has no `{}`", run.id, selector.artifact),
                )
            })?;
        if artifact.expired {
            return Err(gap(
                id,
                "CI_ARTIFACT_EXPIRED",
                format!("artifact {}", artifact.id),
            ));
        }

        Ok(Located { run, job, artifact })
    }

    /// Downloads the artifact, reads the report and the checkout manifest and binds them.
    async fn read(
        &self,
        report: &ExpectedReport,
        selector: &CiReportSelector,
        Located { run, job, artifact }: Located,
    ) -> Result<(CiFacts, Vec<CiGap>), CiGap> {
        let id = report.id.as_str();
        let declared = format(report.format);
        let hosts = &self.conn.config.artifact_hosts;
        let zip = self
            .conn
            .gh
            .download_artifact(
                self.repo,
                artifact.id,
                self.token,
                hosts,
                MAX_ARTIFACT_BYTES,
            )
            .await
            .map_err(|e| gap(id, "CI_UNAVAILABLE", e.to_string()))?;
        let entry = |path: &str| match archive::read_entry(&zip, path, Limits::default()) {
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .map_err(|_| gap(id, "CI_REPORT_MALFORMED", format!("{path} is not UTF-8"))),
            Ok(None) => Err(gap(
                id,
                "CI_REPORT_MISSING",
                format!("`{path}` is not in artifact {}", artifact.id),
            )),
            Err(e) => Err(gap(id, "CI_ARCHIVE_REJECTED", e.to_string())),
        };
        let text = entry(&selector.path)?;
        // No manifest leaves the checkout unproven; a broken one is not read as one.
        let checkout = match selector.checkout_manifest.as_deref().map(entry) {
            Some(Ok(raw)) => {
                let manifest: CiManifest = serde_json::from_str(&raw).map_err(|e| {
                    gap(id, "CI_REPORT_MALFORMED", format!("checkout manifest: {e}"))
                })?;
                Some((manifest, sha256(raw.as_bytes()).to_string()))
            }
            Some(Err(g)) if g.code == "CI_REPORT_MISSING" => None,
            Some(Err(g)) => return Err(g),
            None => None,
        };
        let tested = checkout.as_ref().map(|(m, _)| {
            m.tested_oid
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        let (kind, target) = self.bind(tested.clone()).await;
        let mut gaps = Vec::new();
        if kind == CiBindingKind::Unverified {
            gaps.push(gap(
                id,
                "CI_BINDING_UNVERIFIED",
                "the tested checkout is not proven to be the head or its merge",
            ));
        }

        let origin = CiOrigin {
            provider_instance: self.conn.config.provider_instance.clone(),
            repository: self.repo.to_owned(),
            workflow: selector.workflow.clone(),
            run_id: run.id,
            run_attempt: run.run_attempt,
            job_id: job.id,
            job_name: job.name,
            event: run.event,
            job_status: job.status,
            job_conclusion: job.conclusion,
            allow_failure: None,
            execution_sha: run.head_sha,
            source_head_sha: self.sha.to_owned(),
            target_tip_sha: target,
            tested_sha: tested,
            binding: kind,
            checkout_manifest_sha256: checkout.as_ref().map(|(_, d)| d.clone()),
            artifact_id: artifact.id,
            artifact_name: artifact.name,
            artifact_sha256: sha256(&zip).to_string(),
            report_path: selector.path.clone(),
            started_at: job.started_at,
            completed_at: job.completed_at,
        };
        let manifest = checkout.map(|(m, _)| m);
        import_as(
            self.snap,
            &selector.path,
            &text,
            manifest.as_ref(),
            Some(declared),
            Some(origin),
        )
        .map(|facts| (facts, gaps))
        .map_err(|e| gap(id, "CI_REPORT_MALFORMED", e.to_string()))
    }

    /// The binding of the tested commit; a merge commit is fetched to read its parents.
    async fn bind(&self, tested: Option<String>) -> (CiBindingKind, Option<String>) {
        let Some(tested) = tested else {
            return (CiBindingKind::Unverified, None);
        };
        let url = format!(
            "{}/{}.git",
            self.conn.config.git_base_url.trim_end_matches('/'),
            self.repo
        );
        let (mirror, token, head) = (
            self.mirror.to_owned(),
            self.token.expose().to_owned(),
            self.sha.to_owned(),
        );
        super::worker::blocking(move || {
            if tested != head {
                let _ = fetch_commits(&mirror, &url, Some(&token), &[&tested], FETCH_TIMEOUT);
            }

            binding(&mirror, &tested, &head)
        })
        .await
        .unwrap_or((CiBindingKind::Unverified, None))
    }
}
