//! A real SCM smoke (roadmap M2.1/M2.5): the App's own marked comment is created, read back,
//! updated, found again by its marker and deleted. It writes to the pull request, so it runs
//! only on an explicit `doctor --scm-smoke`; the report carries ids and hashes, never tokens.

use serde::Serialize;
use serde_json::Value;
use sych_adapters::github::{Comment, Github, Secret, capabilities, own_comment};
use sych_core::scm::{ProviderCapabilities, ScmError, ScmErrorKind};
use sych_core::sha256;

#[derive(Debug, Serialize)]
pub struct SmokeReport {
    pub repository: String,
    pub pull_request: u64,
    pub installation_id: u64,
    pub permissions: Value,
    pub capabilities: ProviderCapabilities,
    pub missing_capabilities: Vec<&'static str>,
    pub comment_id: u64,
    pub marker: String,
    pub revisions: Vec<Revision>,
    pub found_by_marker: bool,
    pub deleted: bool,
}

/// One write and its readback by id: the body GitHub returns must be the body written.
#[derive(Debug, Serialize)]
pub struct Revision {
    pub revision: u32,
    pub body_sha256: String,
}

fn failed(message: String) -> ScmError {
    ScmError::new(ScmErrorKind::Transient, message)
}

fn body(marker: &str, revision: u32) -> String {
    format!("<!-- {marker} -->\nSych SCM smoke, revision {revision}; deleted right after.")
}

fn readback(revision: u32, written: &str, read: &Comment, bot: &str) -> Result<Revision, ScmError> {
    if read.body != written || !read.owned_by(bot) {
        return Err(failed(format!(
            "revision {revision} read back by `{}` with another body",
            read.author
        )));
    }

    Ok(Revision {
        revision,
        body_sha256: sha256(written.as_bytes()).to_string(),
    })
}

/// Create → read → update → read → find by marker → delete → confirm 404.
pub async fn smoke(
    gh: &Github,
    repo: &str,
    pr: u64,
    bot: &str,
    nonce: &str,
    now: i64,
) -> Result<SmokeReport, ScmError> {
    let installation = gh.installation(repo, now).await?;
    let caps = capabilities(&installation.permissions);
    let token = gh.token(installation.id, repo, now).await?;
    let marker = format!("sych:smoke:{nonce}");
    let created = gh
        .create_comment(repo, pr, &body(&marker, 1), &token)
        .await?;
    let checked = cycle(gh, repo, pr, bot, &marker, created.id, &token).await;
    // A failed check still removes its comment; a failed delete is reported, not hidden.
    let deleted = gh.delete_comment(repo, created.id, &token).await;
    let (revisions, found_by_marker) = checked?;
    deleted?;
    let gone = matches!(
        gh.comment(repo, created.id, &token).await,
        Err(e) if e.kind == ScmErrorKind::NotFound
    );
    Ok(SmokeReport {
        repository: repo.to_owned(),
        pull_request: pr,
        installation_id: installation.id,
        missing_capabilities: caps.missing(false),
        capabilities: caps,
        permissions: installation.permissions,
        comment_id: created.id,
        marker,
        revisions,
        found_by_marker,
        deleted: gone,
    })
}

async fn cycle(
    gh: &Github,
    repo: &str,
    pr: u64,
    bot: &str,
    marker: &str,
    id: u64,
    token: &Secret,
) -> Result<(Vec<Revision>, bool), ScmError> {
    let first = body(marker, 1);
    let mut revisions = vec![readback(
        1,
        &first,
        &gh.comment(repo, id, token).await?,
        bot,
    )?];
    let second = body(marker, 2);
    gh.update_comment(repo, id, &second, token).await?;
    revisions.push(readback(
        2,
        &second,
        &gh.comment(repo, id, token).await?,
        bot,
    )?);
    let (comments, complete) = gh.comments(repo, pr, token).await?;
    let found = own_comment(comments, complete, bot, marker)?;
    Ok((revisions, found.is_some_and(|c| c.id == id)))
}
