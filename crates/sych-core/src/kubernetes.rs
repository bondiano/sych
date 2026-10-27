//! Rendered Kubernetes manifests reduced to the rollout structure of spec §14.2 (roadmap M3.7):
//! selectors, probes, strategy, termination, volumes, secret/config *names*, hooks. Rendered
//! YAML is structural evidence only: it never confirms that a rollout succeeded.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Applicability, ObligationStatus, PlannedObligation};

/// One workload, job or hook object; values of env, data and secrets are never kept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct K8sObject {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub replicas: Option<u64>,
    /// `RollingUpdate`, `Recreate`, `OnDelete`.
    pub strategy: Option<String>,
    /// The selector does not match the pod template labels: the controller adopts nothing.
    pub selector_mismatch: bool,
    /// Containers without a readiness probe.
    pub without_readiness: Vec<String>,
    pub termination_grace_seconds: Option<u64>,
    pub pre_stop: bool,
    pub persistent_claims: Vec<String>,
    pub secret_refs: Vec<String>,
    pub config_refs: Vec<String>,
    /// `helm.sh/hook`, `argocd.argoproj.io/hook` or a sync wave: ordering the controller owns.
    pub hooks: Vec<String>,
    /// `helm.sh/resource-policy: keep` or a prune annotation.
    pub deletion_policy: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenderedManifests {
    pub objects: Vec<K8sObject>,
    /// Kinds seen but not normalized (CRDs and the like): a limitation, not "no risk".
    pub unsupported_kinds: Vec<String>,
}

const WORKLOADS: &[&str] = &["Deployment", "StatefulSet", "DaemonSet"];

/// Binds K8S-ROLLOUT to the manifests imported for the head; without them it stays `UNKNOWN`.
pub(crate) fn bind(planned: &mut PlannedObligation, rendered: &[(&RenderedManifests, bool)]) {
    if planned.obligation.rule_id != "K8S-ROLLOUT" {
        return;
    }

    let bound: Vec<&RenderedManifests> = rendered
        .iter()
        .filter(|(_, b)| *b)
        .map(|(m, _)| *m)
        .collect();
    if bound.is_empty() {
        planned.obligation.applicability = Applicability::Unknown;
        planned.obligation.status = ObligationStatus::Unknown {
            reason_code: "K8S_MANIFESTS_MISSING".into(),
            next_evidence_needed: "rendered manifests of the changed chart or kustomization, \
                                   bound to the reviewed head"
                .into(),
        };
        return;
    }

    let o = &mut planned.obligation;
    o.limitations.push(
        "rendered manifests are structural evidence: no dry-run or runtime rollout was observed, \
         and controller-specific ordering is not known"
            .into(),
    );
    for m in &bound {
        o.limitations.extend(
            m.unsupported_kinds
                .iter()
                .map(|k| format!("{k} is not normalized; its rollout behaviour is unchecked")),
        );
    }
    planned
        .criteria
        .extend(bound.iter().flat_map(|m| &m.objects).filter_map(criterion));
    planned.obligation.acceptance_criterion = planned.criteria.join("; ");
}

fn criterion(o: &K8sObject) -> Option<String> {
    let mut notes = Vec::new();
    if o.selector_mismatch {
        notes.push("selector does not match the pod template labels".to_owned());
    }

    if WORKLOADS.contains(&o.kind.as_str()) && !o.without_readiness.is_empty() {
        notes.push(format!("no readiness probe on {:?}", o.without_readiness));
    }

    if o.strategy.as_deref() == Some("Recreate") {
        notes.push("Recreate strategy: all pods stop before new ones start".into());
    }

    if !o.persistent_claims.is_empty() {
        notes.push(format!("persistent claims {:?}", o.persistent_claims));
    }

    if !o.hooks.is_empty() {
        notes.push(format!("ordered by {:?}", o.hooks));
    }

    if let Some(policy) = &o.deletion_policy {
        notes.push(format!("deletion policy {policy}"));
    }

    (!notes.is_empty()).then(|| {
        format!(
            "{} {}: {}; the rollout keeps its dependencies available.",
            o.kind,
            o.name,
            notes.join(", ")
        )
    })
}
