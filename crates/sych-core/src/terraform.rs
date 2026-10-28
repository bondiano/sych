//! A Terraform plan reduced to allowlisted structure (spec §14.1, roadmap M3.6): addresses,
//! actions, replacement order and the *names* of changed, unknown and sensitive attributes.
//! No attribute value is kept, so nothing sensitive reaches a prompt, log or artifact.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Applicability, ObligationStatus, PlannedObligation};

/// How a replacement is ordered; Terraform distinguishes the two action lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceOrder {
    /// `["delete", "create"]`: the resource is gone for a while.
    DeleteBeforeCreate,
    /// `["create", "delete"]`: `create_before_destroy`.
    CreateBeforeDelete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResourceChange {
    pub address: String,
    /// As the plan lists them: `no-op`, `create`, `read`, `update`, `delete`.
    pub actions: Vec<String>,
    pub replace_order: Option<ReplaceOrder>,
    /// Attributes whose change forces the replacement.
    pub replace_paths: Vec<String>,
    /// Top-level attributes whose planned value differs; names only.
    pub changed: Vec<String>,
    /// Attributes known only after apply.
    pub unknown_after: Vec<String>,
    /// Attributes marked sensitive before or after.
    pub sensitive: Vec<String>,
    /// `previous_address` of a moved resource.
    pub moved_from: Option<String>,
    /// Explicit `depends_on` of its root-module configuration, as addresses.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl ResourceChange {
    /// Deletes or replaces something that exists now.
    pub fn destructive(&self) -> bool {
        self.actions.iter().any(|a| a == "delete")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerraformPlan {
    pub format_version: String,
    pub terraform_version: Option<String>,
    /// Digest of the plan's input variables; their values are never kept.
    pub variables_sha256: Option<String>,
    /// When the plan was made (`timestamp`, Terraform 1.5+).
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Provider → version constraint from the configuration; the lock file's hashes are not
    /// part of plan JSON.
    #[serde(default)]
    pub providers: BTreeMap<String, String>,
    /// Resources with an action other than `no-op` or `read`.
    pub resource_changes: Vec<ResourceChange>,
}

/// Plan resources listed in a TF-CHANGE criterion; the rest is a limitation, not dropped silently.
const LISTED: usize = 40;

/// Binds TF-CHANGE to the plans imported for the head; without a bound plan it stays
/// `UNKNOWN` instead of being guessed from the HCL diff.
pub(crate) fn bind(planned: &mut PlannedObligation, plans: &[(&TerraformPlan, bool)]) {
    if planned.obligation.rule_id != "TF-CHANGE" {
        return;
    }

    let bound: Vec<&TerraformPlan> = plans.iter().filter(|(_, b)| *b).map(|(p, _)| *p).collect();
    if bound.is_empty() {
        let gap = if plans.is_empty() {
            "a Terraform plan (`terraform show -json`) of the changed configuration"
        } else {
            "a Terraform plan bound to the reviewed head; the imported one tested another commit"
        };
        planned.obligation.applicability = Applicability::Unknown;
        planned.obligation.status = ObligationStatus::Unknown {
            reason_code: "TERRAFORM_PLAN_MISSING".into(),
            next_evidence_needed: gap.into(),
        };
        return;
    }

    let changes: Vec<&ResourceChange> = bound
        .iter()
        .flat_map(|p| &p.resource_changes)
        .filter(|c| c.destructive())
        .collect();
    planned
        .criteria
        .extend(changes.iter().take(LISTED).map(|c| {
            let order = match c.replace_order {
                Some(ReplaceOrder::DeleteBeforeCreate) => " (delete before create: downtime)",
                Some(ReplaceOrder::CreateBeforeDelete) => " (create before delete)",
                None => "",
            };
            // "Intended" alone cannot be checked against code; what keeps the data can.
            format!(
                "The plan (imported for this head, not in the snapshot) will {} {}{order}, \
                 forced by {:?}; unknown after apply {:?}. MET only if the changed configuration \
                 shows the loss is handled: the resource holds no data, or create_before_destroy, \
                 prevent_destroy, a snapshot/restore or moved block keeps it; NOT_MET if a \
                 stateful resource is destroyed with nothing that keeps its data; anchor the \
                 changed lines that force it.",
                c.actions.join("/"),
                c.address,
                c.replace_paths,
                c.unknown_after,
            )
        }));
    if changes.is_empty() {
        planned
            .criteria
            .push("Plan: no resource is deleted or replaced.".into());
    }

    if changes.len() > LISTED {
        planned.obligation.limitations.push(format!(
            "{} more deleted or replaced resources are not listed",
            changes.len() - LISTED
        ));
    }

    planned.obligation.acceptance_criterion = planned.criteria.join("; ");
}
