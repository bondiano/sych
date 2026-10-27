//! TF-CHANGE over an imported plan (roadmap M3.6): without a plan bound to the head it stays
//! UNKNOWN; with one, every deleted or replaced resource is a criterion to assess.

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
        resource_changes: vec![ResourceChange {
            address: "aws_db_instance.main".into(),
            actions: vec!["delete".into(), "create".into()],
            replace_order: Some(ReplaceOrder::DeleteBeforeCreate),
            replace_paths: vec!["engine_version".into()],
            changed: vec!["engine_version".into()],
            unknown_after: vec!["endpoint".into()],
            sensitive: vec!["password".into()],
            moved_from: None,
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
            .contains("aws_db_instance.main delete/create (delete before create: downtime)"),
        "{}",
        bound.acceptance_criterion
    );
    assert!(!bound.acceptance_criterion.contains("password"));
}
