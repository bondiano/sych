//! `/metrics` in the Prometheus text format (spec §26.4), only for allowed client addresses.
//! Labels are bounded enums (states, coverage, judgements, reasons), never paths or ids.

use std::fmt::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use sych_storage::Metrics;

use super::App;

/// Webhook outcomes since start; the reasons are the fixed refusal codes.
#[derive(Debug, Default)]
pub struct Counters {
    pub stored: AtomicU64,
    pub duplicate: AtomicU64,
    pub rejected: [AtomicU64; REASONS.len()],
}

pub const REASONS: [&str; 6] = [
    "BAD_SIGNATURE",
    "BODY_TOO_LARGE",
    "CONTENT_TYPE",
    "INVALID_DELIVERY",
    "NOT_FOUND",
    "STORAGE_UNAVAILABLE",
];

impl Counters {
    pub fn reject(&self, code: &str) {
        if let Some(i) = REASONS.iter().position(|r| *r == code) {
            self.rejected[i].fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn gauge(out: &mut String, name: &str, help: &str, value: i64) {
    let _ = writeln!(
        out,
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}"
    );
}

fn labelled(out: &mut String, name: &str, label: &str, help: &str, values: &[(String, i64)]) {
    let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
    for (key, value) in values {
        let _ = writeln!(out, "{name}{{{label}=\"{key}\"}} {value}");
    }
}

/// A labelled family: name, label, help and its rows.
type Family<'a> = (&'a str, &'a str, &'a str, &'a [(String, i64)]);

/// Renders the storage numbers and the in-process counters.
pub fn render(m: &Metrics, c: &Counters) -> String {
    let mut out = String::new();
    families(&mut out, m);
    gauges(&mut out, m);
    webhooks(&mut out, c);
    out
}

fn families(out: &mut String, m: &Metrics) {
    let rows: [Family<'_>; 6] = [
        ("sych_runs", "state", "Runs by state.", &m.runs_by_state),
        (
            "sych_runs_by_coverage",
            "coverage",
            "Completed runs by coverage.",
            &m.runs_by_coverage,
        ),
        ("sych_jobs", "state", "Jobs by state.", &m.jobs_by_state),
        (
            "sych_model_attempts",
            "state",
            "Model attempts by ledger state.",
            &m.attempts_by_state,
        ),
        (
            "sych_publications",
            "state",
            "Publication operations by state.",
            &m.publications_by_state,
        ),
        (
            "sych_feedback",
            "judgement",
            "Feedback by judgement.",
            &m.feedback_by_judgement,
        ),
    ];
    for (name, label, help, values) in rows {
        labelled(out, name, label, help, values);
    }
}

fn gauges(out: &mut String, m: &Metrics) {
    let gauges = [
        (
            "sych_unknown_obligations",
            "UNKNOWN obligations in completed results.",
            m.unknown_obligations,
        ),
        (
            "sych_queue_oldest_age_seconds",
            "Age of the oldest ready job.",
            m.oldest_queued_age,
        ),
        (
            "sych_model_input_tokens",
            "Reported input tokens.",
            m.input_tokens,
        ),
        (
            "sych_model_output_tokens",
            "Reported output tokens.",
            m.output_tokens,
        ),
        (
            "sych_cost_known_micro_usd",
            "Settled model money.",
            m.cost_known_micro_usd,
        ),
        (
            "sych_reserved_micro_usd",
            "Money held by active reservations and unknown charges.",
            m.reserved_micro_usd,
        ),
        (
            "sych_superseded_cost_micro_usd",
            "Money spent on superseded runs.",
            m.superseded_cost_micro_usd,
        ),
        (
            "sych_outbox_lag_seconds",
            "Age of the oldest pending publication.",
            m.oldest_pending_publication_age,
        ),
        (
            "sych_uncertain_age_seconds",
            "Age of the oldest uncertain publication.",
            m.oldest_uncertain_publication_age,
        ),
        (
            "sych_publication_repairs",
            "Late older writes repaired.",
            m.publication_repairs,
        ),
        (
            "sych_artifact_bytes",
            "Stored artifact bytes.",
            m.artifact_bytes,
        ),
        ("sych_artifacts", "Stored artifacts.", m.artifacts),
    ];
    for (name, help, value) in gauges {
        gauge(out, name, help, value);
    }
}

fn webhooks(out: &mut String, c: &Counters) {
    let load = |a: &AtomicU64| i64::try_from(a.load(Ordering::Relaxed)).unwrap_or(i64::MAX);
    let _ = writeln!(
        out,
        "# HELP sych_webhooks Webhook deliveries since start.\n# TYPE sych_webhooks counter"
    );
    let _ = writeln!(
        out,
        "sych_webhooks{{outcome=\"stored\"}} {}",
        load(&c.stored)
    );
    let _ = writeln!(
        out,
        "sych_webhooks{{outcome=\"duplicate\"}} {}",
        load(&c.duplicate)
    );
    for (reason, count) in REASONS.iter().zip(&c.rejected) {
        let _ = writeln!(
            out,
            "sych_webhooks{{outcome=\"rejected\",reason=\"{reason}\"}} {}",
            load(count)
        );
    }
}

/// `GET /metrics`.
pub async fn metrics(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<String, StatusCode> {
    if !app.metrics_allow.contains(&peer.ip()) {
        return Err(StatusCode::FORBIDDEN);
    }

    scrape(State(app)).await
}

/// The dedicated listener is private by deployment policy, not by forwarded client IP.
pub async fn scrape(State(app): State<Arc<App>>) -> Result<String, StatusCode> {
    let m = app
        .storage
        .metrics((app.now)())
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(render(&m, &app.counters))
}
