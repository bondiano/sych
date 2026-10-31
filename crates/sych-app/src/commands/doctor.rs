//! Availability checks: no deploy, comment or model call unless asked for by
//! `--probe-model` or `--scm-smoke`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::{env, fs};

use anyhow::Context as _;
use sych_core::Timestamp;
use uuid::Uuid;

use sych_adapters::git::Git;
use sych_core::config::{BillingKind, EffectiveConfig, RunMode};
use sych_core::model::PriceCatalog;
use sych_core::redact;

use crate::args::DoctorArguments;
use crate::backend::{self, ResolveError};
use crate::errors::EXIT_PERMISSION;
use crate::server::build::connections;
use crate::server::smoke::smoke;
use crate::storage::block_on;
use crate::{config, probe, storage};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Check {
    Ok,
    Warn,
    Fail,
}

fn line(level: Check, name: &str, detail: &str) -> Check {
    let tag = match level {
        Check::Ok => "ok  ",
        Check::Warn => "warn",
        Check::Fail => "FAIL",
    };
    println!("{tag} {name}: {}", redact(detail));
    level
}

fn host_path(config: &Path, path: &str) -> PathBuf {
    config.parent().unwrap_or(Path::new(".")).join(path)
}

fn artifact_store(config: &Path, effective: &EffectiveConfig) -> Option<Check> {
    let store = effective.server.storage.artifact_store.as_ref()?;
    let path = host_path(config, &store.path);
    let target = if path.exists() {
        path.as_path()
    } else {
        path.parent().unwrap_or(&path)
    };
    Some(match fs::metadata(target) {
        Ok(meta) if meta.is_dir() && !meta.permissions().readonly() => line(
            Check::Ok,
            "artifact store",
            &format!("{} is writable", path.display()),
        ),
        Ok(_) => line(
            Check::Fail,
            "artifact store",
            &format!("{} is not a writable directory", target.display()),
        ),
        Err(err) => line(
            Check::Warn,
            "artifact store",
            &format!("{}: {err}", target.display()),
        ),
    })
}

fn price_catalog(config: &Path, effective: &EffectiveConfig) -> Option<Check> {
    let path = host_path(config, effective.server.models.price_catalog_file.as_ref()?);
    Some(match fs::read_to_string(&path) {
        Ok(text) => match PriceCatalog::parse(&path.display().to_string(), &text) {
            Ok(catalog) => line(
                Check::Ok,
                "price catalog",
                &format!("{} revision {}", path.display(), catalog.revision),
            ),
            Err(err) => line(Check::Fail, "price catalog", &err.to_string()),
        },
        Err(_) => line(
            Check::Warn,
            "price catalog",
            &format!(
                "{} is missing; calls with a hard monetary cap will not start",
                path.display()
            ),
        ),
    })
}

/// Whether each profile could be called now, without calling it.
fn profiles(config: &Path, effective: &EffectiveConfig) -> Vec<Check> {
    let server = &effective.server;
    server
        .models
        .profiles
        .keys()
        .map(|name| {
            let name_line = format!("profile {name}");
            match backend::resolve(config, server, name, RunMode::PersonalLocal) {
                Ok(r)
                    if r.route.billing == BillingKind::SubscriptionQuota
                        && r.fixed_fee.is_none() =>
                {
                    line(
                        Check::Warn,
                        &name_line,
                        "subscription fixed_fee is not set; the gateway refuses its calls",
                    )
                }
                Ok(r) => line(
                    Check::Ok,
                    &name_line,
                    &format!("{} {} resolves", r.route.adapter, r.route.model),
                ),
                Err(ResolveError::Invalid(e) | ResolveError::Credentials(e)) => {
                    line(Check::Warn, &name_line, &e)
                }
            }
        })
        .collect()
}

/// The real SCM smoke; the server image has no repository checkout, so local checks are skipped.
fn scm(effective: &EffectiveConfig, connection: &str, pr: u64) -> anyhow::Result<ExitCode> {
    let conns = connections(&effective.server)?;
    let conn = conns
        .get(connection)
        .with_context(|| format!("no GitHub connection `{connection}`"))?;
    let repo = conn
        .config
        .repositories
        .first()
        .with_context(|| format!("connection `{connection}` lists no repository"))?;
    let nonce = Uuid::now_v7().to_string();
    let report = block_on(smoke(
        &conn.gh,
        repo,
        pr,
        &conn.config.bot_login,
        &nonce,
        Timestamp::now().as_second(),
    ))??;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(if report.found_by_marker && report.deleted {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

pub fn run(args: &DoctorArguments) -> anyhow::Result<ExitCode> {
    let DoctorArguments {
        config,
        repo,
        probe_model,
        scm_smoke,
        pr,
    } = args;
    let effective = config::effective(Some(config), Some(repo))?;
    if let (Some(connection), Some(pr)) = (scm_smoke, pr) {
        return scm(&effective, connection, *pr);
    }

    let mut checks = vec![line(
        Check::Ok,
        "config",
        &format!("valid, config_hash {}", effective.config_hash),
    )];
    checks.push(match Git::open(repo) {
        Ok(git) => line(
            Check::Ok,
            "git",
            &format!("repository {}", git.git_dir().display()),
        ),
        Err(err) => line(Check::Fail, "git", &err.to_string()),
    });
    for (key, name) in effective.server.env_refs() {
        let set = env::var_os(name.as_str()).is_some();
        checks.push(line(
            if set { Check::Ok } else { Check::Warn },
            &format!("env {}", name.as_str()),
            &format!("{} ({key})", if set { "set" } else { "not set" }),
        ));
    }

    checks.extend(artifact_store(config, &effective));
    checks.extend(price_catalog(config, &effective));
    checks.extend(profiles(config, &effective));
    checks.push(match storage::database_status(config, &effective.server) {
        Ok(detail) => line(Check::Ok, "database", &detail),
        Err(detail) => line(Check::Warn, "database", &detail),
    });
    if checks.contains(&Check::Fail) {
        return Ok(ExitCode::from(EXIT_PERMISSION));
    }

    if let Some(profile) = probe_model {
        return probe::probe(config, &effective.server, profile);
    }

    line(
        Check::Ok,
        "models",
        "not probed; no model request is made without --probe-model",
    );
    Ok(ExitCode::SUCCESS)
}
