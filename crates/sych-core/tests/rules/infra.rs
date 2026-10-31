//! TF-CHANGE and K8S-ROLLOUT over imported infrastructure reports: without
//! a report bound to the head they stay UNKNOWN; with one, each risky resource is a criterion.

use std::collections::BTreeMap;

use sych_core::kubernetes::{EvidenceStage, K8sObject, RenderedManifests};
use sych_core::terraform::{ReplaceOrder, ResourceChange, TerraformPlan};
use sych_core::{CiFacts, CiFormat, ObligationStatus, RunFacts};

use super::{changed, planned, policy, rule, snapshot};

fn plan_report(stale: bool) -> CiFacts {
    let mut facts = CiFacts::new(snapshot(), CiFormat::TerraformPlan, "plan.json".into());
    if stale {
        facts
            .limitations
            .push("STALE: the report tested another commit".into());
    }

    facts.terraform = Some(TerraformPlan {
        format_version: "1.2".into(),
        terraform_version: Some("1.9.5".into()),
        variables_sha256: None,
        timestamp: None,
        providers: BTreeMap::new(),
        resource_changes: vec![ResourceChange {
            address: "aws_db_instance.main".into(),
            actions: vec!["delete".into(), "create".into()],
            replace_order: Some(ReplaceOrder::DeleteBeforeCreate),
            replace_paths: vec!["engine_version".into()],
            changed: vec!["engine_version".into()],
            unknown_after: vec!["endpoint".into()],
            sensitive: vec!["password".into()],
            moved_from: None,
            depends_on: Vec::new(),
        }],
    });
    facts
}

fn tf_change(ci: Vec<CiFacts>) -> sych_core::Obligation {
    let facts = RunFacts {
        ci,
        ..RunFacts::default()
    };
    let p = planned(vec![changed("infra/db.tf")], &policy(&[]), &facts);
    rule(&p, "TF-CHANGE").expect("terraform rule").clone()
}

fn missing(o: &sych_core::Obligation) -> bool {
    matches!(&o.status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "TERRAFORM_PLAN_MISSING")
}

#[test]
fn tf_change_needs_a_plan_bound_to_the_head() {
    assert!(missing(&tf_change(Vec::new())));
    assert!(missing(&tf_change(vec![plan_report(true)])));

    let bound = tf_change(vec![plan_report(false)]);
    assert!(!missing(&bound));
    assert!(
        bound
            .acceptance_criterion
            .contains("delete/create aws_db_instance.main (delete before create: downtime)"),
        "{}",
        bound.acceptance_criterion
    );
    assert!(!bound.acceptance_criterion.contains("password"));
}

fn rollout(ci: Vec<CiFacts>) -> sych_core::Obligation {
    let facts = RunFacts {
        ci,
        ..RunFacts::default()
    };
    let p = planned(vec![changed("k8s/web.yaml")], &policy(&[]), &facts);
    rule(&p, "K8S-ROLLOUT").expect("rollout rule").clone()
}

/// K8S-ROLLOUT: without rendered manifests it is UNKNOWN; with them each risky
/// object is a criterion, and rendered YAML is never taken for an observed rollout.
#[test]
fn k8s_rollout_needs_rendered_manifests_and_stays_structural() {
    assert!(matches!(&rollout(Vec::new()).status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "K8S_MANIFESTS_MISSING"));

    let mut report = CiFacts::new(
        snapshot(),
        CiFormat::KubernetesRendered,
        "rendered.yaml".into(),
    );
    report.kubernetes = Some(RenderedManifests {
        objects: vec![K8sObject {
            kind: "Deployment".into(),
            name: "web".into(),
            selector_mismatch: true,
            strategy: Some("Recreate".into()),
            ..K8sObject::default()
        }],
        unsupported_kinds: vec!["ServiceMonitor".into()],
        stage: EvidenceStage::DryRun,
    });
    let o = rollout(vec![report]);
    assert!(
        o.acceptance_criterion
            .contains("Deployment web: selector does not match")
    );
    assert!(
        o.limitations
            .iter()
            .any(|l| l.contains("a dry-run is not a successful runtime rollout"))
    );
    assert!(
        o.limitations
            .iter()
            .any(|l| l.starts_with("ServiceMonitor"))
    );

    // Each stage states what it does not prove; rendered YAML is never an observed rollout.
    for (stage, says) in [
        (
            EvidenceStage::Rendered,
            "no dry-run or runtime rollout was observed",
        ),
        (
            EvidenceStage::Runtime,
            "evidence of this rollout only if taken after it",
        ),
    ] {
        let mut report = CiFacts::new(snapshot(), CiFormat::KubernetesRendered, "r.yaml".into());
        report.kubernetes = Some(RenderedManifests {
            stage,
            ..RenderedManifests::default()
        });
        let o = rollout(vec![report]);
        assert!(
            o.limitations.iter().any(|l| l.contains(says)),
            "{stage:?}: {:?}",
            o.limitations
        );
    }
}
