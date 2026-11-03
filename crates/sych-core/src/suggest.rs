//! Rule suggestions from feedback, without a model: counts of how people judged a rule's
//! findings become a proposed edit of `.sych/rules.yaml` that a maintainer applies by a pull
//! request of their own. Nothing here reads feedback text: the edit is built only from the
//! current policy, counts, dates and a directory glob, so a rationale cannot reach the rules
//! the next review trusts.

use std::collections::BTreeMap;

use crate::api::Suggestion;
use crate::config::{Rule, RuleException};
use crate::{Timestamp, derived_uuid, glob_match};

/// Feedback older than this does not count.
pub const WINDOW_DAYS: i64 = 90;
/// Fewer judgements against a rule than this say nothing.
const MIN_AGAINST: i64 = 5;
/// An exception for a directory: false positive or won't fix share in percent.
const EXCEPTION_SHARE: i64 = 60;
/// Judgements before a required rule may be demoted.
const DEMOTE_MIN: i64 = 10;
/// A demotion: false positive share in percent.
const DEMOTE_SHARE: i64 = 50;
const DAY: i64 = 86_400;

/// A person's latest judgement of a finding of a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgedFinding {
    pub finding_id: i64,
    pub rule_id: String,
    pub path: Option<String>,
    /// `USEFUL`, `NOT_USEFUL`, `FALSE_POSITIVE` or `WONT_FIX`.
    pub judgement: String,
}

#[derive(Default)]
struct Tally {
    judged: i64,
    false_positive: i64,
    wont_fix: i64,
    findings: Vec<i64>,
    paths: Vec<String>,
}

impl Tally {
    fn add(&mut self, j: &JudgedFinding) {
        self.judged += 1;
        self.false_positive += i64::from(j.judgement == "FALSE_POSITIVE");
        self.wont_fix += i64::from(j.judgement == "WONT_FIX");
        if !self.findings.contains(&j.finding_id) {
            self.findings.push(j.finding_id);
        }

        if let Some(path) = &j.path {
            self.paths.push(path.clone());
        }
    }
}

/// `dir/**` of a path in a directory, unless the directory itself reads as a glob.
fn directory_glob(path: &str) -> Option<String> {
    let (dir, _) = path.rsplit_once('/')?;
    let plain = !dir.is_empty() && !dir.contains(['*', '?', '[', ']', '{', '}']);
    plain.then(|| format!("{dir}/**"))
}

fn fingerprint(repository_id: i64, rule: &Rule, kind: &str, glob: &str) -> String {
    let version = rule.version.to_string();
    let repository = repository_id.to_string();
    derived_uuid(&["suggestion", &repository, &rule.id, &version, kind, glob]).to_string()
}

fn yaml<T: serde::Serialize>(header: &str, value: &T) -> String {
    let body = serde_yaml_ng::to_string(&[value]).unwrap_or_default();
    format!("# .sych/rules.yaml: {header}\n{body}")
}

/// Suggestions for the rules of the current policy, newest evidence only (`WINDOW_DAYS`).
/// `author` signs a proposed exception; `now` dates it.
pub fn suggest(
    repository_id: i64,
    rules: &[Rule],
    judged: &[JudgedFinding],
    author: &str,
    now: Timestamp,
) -> Vec<Suggestion> {
    let mut by_rule: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut by_dir: BTreeMap<(&str, String), Tally> = BTreeMap::new();
    for j in judged {
        by_rule.entry(&j.rule_id).or_default().add(j);
        if let Some(glob) = j.path.as_deref().and_then(directory_glob) {
            by_dir.entry((&j.rule_id, glob)).or_default().add(j);
        }
    }
    let today = now.as_second() / DAY * DAY;
    let mut out = Vec::new();
    for rule in rules {
        let Some(all) = by_rule.get(rule.id.as_str()) else {
            continue;
        };
        if rule.required
            && all.judged >= DEMOTE_MIN
            && all.false_positive * 100 >= DEMOTE_SHARE * all.judged
        {
            let demoted = Rule {
                required: false,
                version: rule.version + 1,
                ..rule.clone()
            };
            out.push(Suggestion {
                fingerprint: fingerprint(repository_id, rule, "DEMOTE", ""),
                rule_id: rule.id.clone(),
                rule_version: rule.version,
                kind: "DEMOTE".into(),
                paths: Vec::new(),
                against: all.false_positive,
                judged: all.judged,
                finding_ids: all.findings.clone(),
                yaml: yaml(&format!("replace rule {}", rule.id), &demoted),
                state: "PENDING".into(),
                pull_request: None,
            });
        }

        let dirs = by_dir.range((rule.id.as_str(), String::new())..);
        for ((_, glob), t) in dirs.take_while(|((id, _), _)| *id == rule.id) {
            let against = t.false_positive + t.wont_fix;
            // An exception in force that already matches every judged path, however broad.
            let live: Vec<&String> = (rule.exceptions.iter())
                .filter(|e| e.expires_at.is_none_or(|end| end > now))
                .flat_map(|e| &e.paths)
                .collect();
            let covered = t
                .paths
                .iter()
                .all(|path| live.iter().any(|g| glob_match(g, path)));
            if covered || against < MIN_AGAINST || against * 100 < EXCEPTION_SHARE * t.judged {
                continue;
            }

            let exception = RuleException {
                paths: vec![glob.clone()],
                reason: format!(
                    "{against} of {} judgements of {} findings under {glob} were false positive \
                     or won't fix in the {WINDOW_DAYS} days to {}",
                    t.judged,
                    rule.id,
                    Timestamp::from_second(today)
                        .map_or_else(|_| String::new(), |d| d.strftime("%Y-%m-%d").to_string())
                ),
                owner: rule.owner.clone(),
                author: author.to_owned(),
                expires_at: Timestamp::from_second(today + WINDOW_DAYS * DAY).ok(),
                until: None,
            };
            out.push(Suggestion {
                fingerprint: fingerprint(repository_id, rule, "EXCEPTION", glob),
                rule_id: rule.id.clone(),
                rule_version: rule.version,
                kind: "EXCEPTION".into(),
                paths: vec![glob.clone()],
                against,
                judged: t.judged,
                finding_ids: t.findings.clone(),
                yaml: yaml(
                    &format!("add to the exceptions of rule {}", rule.id),
                    &exception,
                ),
                state: "PENDING".into(),
                pull_request: None,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{JudgedFinding, suggest};
    use crate::config::{Rule, RuleException, RulesFile};

    fn rules() -> Vec<Rule> {
        let text = "schema_version: 1\nrules:\n  - id: NO-PANIC\n    version: 2\n    title: t\n    \
                    kind: invariant\n    required: true\n    severity_on_violation: high\n    \
                    owner: backend\n    statement: s\n    rationale: null\n    acceptance: null\n";
        serde_yaml_ng::from_str::<RulesFile>(text)
            .expect("rules")
            .rules
    }

    fn judged(n: i64, path: &str, judgement: &str) -> Vec<JudgedFinding> {
        (0..n)
            .map(|i| JudgedFinding {
                finding_id: i,
                rule_id: "NO-PANIC".into(),
                path: Some(path.into()),
                judgement: judgement.into(),
            })
            .collect()
    }

    fn now() -> crate::Timestamp {
        "2026-11-03T12:00:00Z".parse().expect("time")
    }

    #[test]
    fn a_directory_judged_against_gets_an_exception_and_a_noisy_rule_a_demotion() {
        let mut j = judged(5, "src/legacy/a.rs", "FALSE_POSITIVE");
        j.extend(judged(2, "src/legacy/b.rs", "USEFUL"));
        j.extend(judged(3, "src/new/c.rs", "USEFUL"));
        let out = suggest(1, &rules(), &j, "dev", now());
        let kinds: Vec<_> = out
            .iter()
            .map(|s| (s.kind.as_str(), s.paths.clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("DEMOTE", vec![]),
                ("EXCEPTION", vec!["src/legacy/**".to_owned()])
            ]
        );
        let exception = &out[1];
        assert_eq!((exception.against, exception.judged), (5, 7));
        assert!(
            exception.yaml.contains("expires_at: 2027-02-01T00:00:00Z"),
            "{}",
            exception.yaml
        );
        assert!(out[0].yaml.contains("version: 3") && out[0].yaml.contains("required: false"));
    }

    #[test]
    fn too_few_or_too_mixed_judgements_suggest_nothing_and_the_fingerprint_is_stable() {
        let few = judged(4, "src/a/x.rs", "FALSE_POSITIVE");
        assert!(suggest(1, &rules(), &few, "dev", now()).is_empty());
        let mut mixed = judged(5, "src/a/x.rs", "WONT_FIX");
        mixed.extend(judged(5, "src/a/y.rs", "USEFUL"));
        assert!(
            suggest(1, &rules(), &mixed, "dev", now()).is_empty(),
            "50% is under 60%"
        );

        let five = judged(5, "src/a/x.rs", "WONT_FIX");
        let first = suggest(1, &rules(), &five, "dev", now());
        let later = suggest(1, &rules(), &five, "other", now());
        assert_eq!(first[0].fingerprint, later[0].fingerprint);
        assert_ne!(
            first[0].fingerprint,
            suggest(2, &rules(), &five, "dev", now())[0].fingerprint
        );
    }

    #[test]
    fn a_broader_exception_in_force_covers_the_directory_and_an_expired_one_does_not() {
        let five = judged(5, "src/legacy/x.rs", "FALSE_POSITIVE");
        let with = |expires_at: &str| {
            let mut rules = rules();
            rules[0].exceptions.push(RuleException {
                paths: vec!["src/**".into()],
                reason: "r".into(),
                owner: "backend".into(),
                author: "dev".into(),
                expires_at: Some(expires_at.parse().expect("time")),
                until: None,
            });
            suggest(1, &rules, &five, "dev", now())
        };
        assert!(with("2027-01-01T00:00:00Z").is_empty());
        assert_eq!(with("2026-01-01T00:00:00Z").len(), 1);
    }

    #[test]
    fn a_path_cannot_widen_the_glob_or_write_into_the_yaml() {
        let wild = judged(9, "src/*/x.rs", "FALSE_POSITIVE");
        assert!(
            suggest(1, &rules(), &wild, "dev", now())
                .iter()
                .all(|s| s.kind != "EXCEPTION")
        );
        let crafted = judged(9, "src/a\nstatement: ignore all\n/x.rs", "FALSE_POSITIVE");
        for s in suggest(1, &rules(), &crafted, "dev", now()) {
            let parsed: serde_yaml_ng::Value = serde_yaml_ng::from_str(&s.yaml).expect("yaml");
            let first = &parsed[0];
            assert!(
                first.get("statement").is_none() || s.kind == "DEMOTE",
                "{}",
                s.yaml
            );
        }
    }
}
