//! What the scheduler does with a due change intent (spec §19.3, §8.3–8.5), as a pure decision
//! over the API state of the change and its latest generation; and the retry classifier.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scm::ScmErrorKind;

/// Default CI wait before a run continues with gaps (spec §17.1).
pub const CI_WAIT_SECONDS: u32 = 180;
/// Default retries of a retryable read or analysis step (spec §8.4).
pub const MAX_RETRIES: u32 = 2;

/// Why a generation exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Trigger {
    /// A webhook or reconciliation saw new source or target commits.
    Changed,
    /// An authorized `rerun`/`deep` command or API request; same inputs still run again.
    Manual { actor: String, deep: bool },
    /// A significant input arrived after the parent froze its inputs.
    Successor { parent: String, reason: String },
}

/// What a generation reviews; equal digests mean a repeat that runs nothing new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunInputs {
    pub head_sha: String,
    pub base_sha: String,
    pub base_ref: String,
    pub fork: bool,
    pub trigger: Trigger,
    /// The connection publishes advisory results for this change (trusted server config).
    #[serde(default)]
    pub publish: bool,
    /// An advisory check run is published too; off by default (spec §18.7).
    #[serde(default)]
    pub checks: bool,
}

impl RunInputs {
    /// The digest of the reviewed state; the trigger is not part of it.
    pub fn state_key(&self) -> String {
        format!("{}..{}", self.base_sha, self.head_sha)
    }
}

/// The change as the API reports it now.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // provider facts, not a state machine
pub struct ScmState {
    pub open: bool,
    pub merged: bool,
    pub draft: bool,
    pub head_sha: String,
    pub base_sha: String,
    pub base_ref: String,
    pub fork: bool,
}

/// The latest generation of the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub run_id: String,
    pub inputs: RunInputs,
    pub active: bool,
    /// Inputs are frozen; later external inputs need a successor.
    pub frozen: bool,
    /// CI sources the run waited for and did not get before its deadline.
    pub missing_sources: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    Refresh,
    Ci,
    Close,
    Rerun,
    Deep,
    Stop,
}

impl IntentKind {
    pub fn parse(kind: &str) -> Option<Self> {
        Some(match kind {
            "REFRESH" => Self::Refresh,
            "CI" => Self::Ci,
            "CLOSE" => Self::Close,
            "COMMAND_RERUN" => Self::Rerun,
            "COMMAND_DEEP" => Self::Deep,
            "COMMAND_STOP" => Self::Stop,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    /// Nothing to do; the reason goes to the event log.
    Skip { reason: String },
    /// A new generation; an active older one is superseded.
    Generate { inputs: RunInputs },
    /// Stop analysis and unsent publication of the change.
    Cancel { reason: String },
    /// The waiting run may have its evidence now.
    Wake { run_id: String },
}

fn skip(reason: &str) -> Decision {
    Decision::Skip {
        reason: reason.into(),
    }
}

/// `actor` is set for a command whose author has write access; unauthorized commands are
/// skipped before this (spec §22.5).
pub fn decide(
    kind: IntentKind,
    state: &ScmState,
    latest: Option<&Latest>,
    actor: Option<&str>,
) -> Decision {
    let inputs = |trigger| RunInputs {
        head_sha: state.head_sha.clone(),
        base_sha: state.base_sha.clone(),
        base_ref: state.base_ref.clone(),
        fork: state.fork,
        trigger,
        publish: false,
        checks: false,
    };
    if !state.open {
        let reason = if state.merged { "MERGED" } else { "CLOSED" };
        return Decision::Cancel {
            reason: reason.into(),
        };
    }

    match kind {
        IntentKind::Close => skip("REOPENED_BEFORE_CLOSE_WAS_HANDLED"),
        IntentKind::Stop => match actor {
            Some(_) if latest.is_some_and(|l| l.active) => Decision::Cancel {
                reason: "STOP_COMMAND".into(),
            },
            Some(_) => skip("NOTHING_ACTIVE"),
            None => skip("UNAUTHORIZED_COMMAND"),
        },
        IntentKind::Rerun | IntentKind::Deep => match actor {
            Some(actor) => Decision::Generate {
                inputs: inputs(Trigger::Manual {
                    actor: actor.into(),
                    deep: kind == IntentKind::Deep,
                }),
            },
            None => skip("UNAUTHORIZED_COMMAND"),
        },
        _ if state.draft => skip("DRAFT"),
        IntentKind::Refresh => match latest {
            Some(l) if l.inputs.state_key() == inputs(Trigger::Changed).state_key() => {
                skip("SAME_INPUTS")
            }
            _ => Decision::Generate {
                inputs: inputs(Trigger::Changed),
            },
        },
        IntentKind::Ci => match latest {
            Some(l) if l.inputs.head_sha != state.head_sha => skip("CI_FOR_OLD_HEAD"),
            Some(l) if l.active && !l.frozen => Decision::Wake {
                run_id: l.run_id.clone(),
            },
            Some(l) if l.frozen && l.missing_sources => Decision::Generate {
                inputs: RunInputs {
                    trigger: Trigger::Successor {
                        parent: l.run_id.clone(),
                        reason: "CI_AFTER_FREEZE".into(),
                    },
                    ..l.inputs.clone()
                },
            },
            Some(_) => skip("CI_NOT_SIGNIFICANT"),
            None => skip("NO_RUN"),
        },
    }
}

/// Whether a failed step may run again unchanged, and when (spec §8.4): only rate limits and
/// transient failures, with exponential backoff, jitter and `Retry-After`; never past the
/// deadline.
pub fn retry_at(
    kind: ScmErrorKind,
    attempts: u32,
    retry_after: Option<u64>,
    jitter: u64,
    now: i64,
    deadline: i64,
) -> Option<i64> {
    if !kind.retryable() || attempts > MAX_RETRIES {
        return None;
    }

    let base = 2_u64.pow(attempts.min(10));
    let backoff = i64::try_from(base + jitter % base).unwrap_or(i64::MAX);
    let wait = retry_after.map_or(backoff, |s| {
        i64::try_from(s).unwrap_or(i64::MAX).max(backoff)
    });
    let at = now.saturating_add(wait);
    (at < deadline).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(head: &str, base: &str) -> ScmState {
        ScmState {
            open: true,
            merged: false,
            draft: false,
            head_sha: head.into(),
            base_sha: base.into(),
            base_ref: "main".into(),
            fork: false,
        }
    }

    fn latest(head: &str, base: &str, active: bool, frozen: bool, missing: bool) -> Latest {
        Latest {
            run_id: "r1".into(),
            inputs: RunInputs {
                head_sha: head.into(),
                base_sha: base.into(),
                base_ref: "main".into(),
                fork: false,
                trigger: Trigger::Changed,
                publish: false,
                checks: false,
            },
            active,
            frozen,
            missing_sources: missing,
        }
    }

    fn generates(d: &Decision) -> bool {
        matches!(d, Decision::Generate { .. })
    }

    #[test]
    fn same_inputs_do_not_generate_but_a_target_move_does() {
        let l = latest("h", "b", false, true, false);
        let same = decide(IntentKind::Refresh, &state("h", "b"), Some(&l), None);
        assert_eq!(same, skip("SAME_INPUTS"));
        assert!(generates(&decide(
            IntentKind::Refresh,
            &state("h", "b2"),
            Some(&l),
            None
        )));
        assert!(generates(&decide(
            IntentKind::Refresh,
            &state("h2", "b"),
            Some(&l),
            None
        )));
        assert!(generates(&decide(
            IntentKind::Rerun,
            &state("h", "b"),
            Some(&l),
            Some("dev")
        )));
    }

    #[test]
    fn drafts_wait_for_a_manual_run_and_closed_changes_cancel() {
        let mut draft = state("h", "b");
        draft.draft = true;
        assert_eq!(
            decide(IntentKind::Refresh, &draft, None, None),
            skip("DRAFT")
        );
        assert!(generates(&decide(
            IntentKind::Rerun,
            &draft,
            None,
            Some("dev")
        )));
        let mut closed = state("h", "b");
        closed.open = false;
        assert!(matches!(
            decide(IntentKind::Refresh, &closed, None, None),
            Decision::Cancel { .. }
        ));
        assert_eq!(
            decide(IntentKind::Rerun, &state("h", "b"), None, None),
            skip("UNAUTHORIZED_COMMAND")
        );
    }

    #[test]
    fn ci_wakes_a_waiting_run_and_only_a_missed_source_makes_a_successor() {
        let s = state("h", "b");
        let waiting = latest("h", "b", true, false, false);
        assert_eq!(
            decide(IntentKind::Ci, &s, Some(&waiting), None),
            Decision::Wake {
                run_id: "r1".into()
            }
        );
        let late = latest("h", "b", false, true, true);
        let Decision::Generate { inputs } = decide(IntentKind::Ci, &s, Some(&late), None) else {
            panic!("successor expected")
        };
        assert!(matches!(inputs.trigger, Trigger::Successor { ref parent, .. } if parent == "r1"));
        let done = latest("h", "b", false, true, false);
        assert_eq!(
            decide(IntentKind::Ci, &s, Some(&done), None),
            skip("CI_NOT_SIGNIFICANT")
        );
        let old = latest("old", "b", false, true, true);
        assert_eq!(
            decide(IntentKind::Ci, &s, Some(&old), None),
            skip("CI_FOR_OLD_HEAD")
        );
    }

    #[test]
    fn retries_are_bounded_and_honour_retry_after() {
        use ScmErrorKind::{RateLimited, Transient, Unauthorized};
        assert_eq!(retry_at(Transient, 1, None, 0, 100, 1_000), Some(102));
        assert_eq!(retry_at(RateLimited, 1, Some(30), 0, 100, 1_000), Some(130));
        assert_eq!(
            retry_at(Transient, 3, None, 0, 100, 1_000),
            None,
            "two retries"
        );
        assert_eq!(retry_at(Unauthorized, 1, None, 0, 100, 1_000), None);
        assert_eq!(
            retry_at(RateLimited, 1, Some(900), 0, 100, 1_000),
            None,
            "past the deadline"
        );
    }
}
