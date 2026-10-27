//! `terraform show -json` of a saved plan, reduced to allowlisted structure before anything
//! else sees it: attribute values are compared and dropped, only names and paths are kept.

use serde_json::{Map, Value};
use sych_core::terraform::{ReplaceOrder, ResourceChange, TerraformPlan};
use sych_core::{CiFacts, canonical_sha256};

use super::{MAX_RECORDS, malformed};
use crate::ci::CiError;

/// Top-level attributes of `v` (an object of booleans or nested masks) that are set anywhere.
fn marked(v: &Value) -> Vec<String> {
    fn any(v: &Value) -> bool {
        match v {
            Value::Bool(b) => *b,
            Value::Array(a) => a.iter().any(any),
            Value::Object(o) => o.values().any(any),
            _ => false,
        }
    }
    v.as_object()
        .map(|o| {
            o.iter()
                .filter(|(_, v)| any(v))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn changed(before: &Value, after: &Value) -> Vec<String> {
    let empty = Map::new();
    let (b, a) = (
        before.as_object().unwrap_or(&empty),
        after.as_object().unwrap_or(&empty),
    );
    let mut keys: Vec<String> = b
        .keys()
        .chain(a.keys())
        .filter(|k| b.get(*k) != a.get(*k))
        .cloned()
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn path(p: &Value) -> String {
    p.as_array()
        .map(|parts| {
            parts
                .iter()
                .map(|s| s.as_str().map_or_else(|| s.to_string(), str::to_owned))
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default()
}

fn resource(r: &Value) -> Option<ResourceChange> {
    let change = &r["change"];
    let actions: Vec<String> = change["actions"]
        .as_array()?
        .iter()
        .filter_map(|a| a.as_str().map(str::to_owned))
        .collect();
    if actions.iter().all(|a| a == "no-op" || a == "read") {
        return None;
    }

    let replace_order = match actions.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["delete", "create"] => Some(ReplaceOrder::DeleteBeforeCreate),
        ["create", "delete"] => Some(ReplaceOrder::CreateBeforeDelete),
        _ => None,
    };
    let mut sensitive = marked(&change["before_sensitive"]);
    sensitive.extend(marked(&change["after_sensitive"]));
    sensitive.sort();
    sensitive.dedup();
    Some(ResourceChange {
        address: r["address"].as_str()?.to_owned(),
        replace_order,
        replace_paths: change["replace_paths"]
            .as_array()
            .map(|ps| ps.iter().map(path).collect())
            .unwrap_or_default(),
        changed: changed(&change["before"], &change["after"]),
        unknown_after: marked(&change["after_unknown"]),
        sensitive,
        moved_from: r["previous_address"].as_str().map(str::to_owned),
        actions,
    })
}

pub fn parse(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let plan: Value = serde_json::from_str(text).map_err(|e| malformed(facts, &e))?;
    let format_version = plan["format_version"]
        .as_str()
        .ok_or_else(|| malformed(facts, &"no format_version"))?;
    // Minor versions only add fields; another major can change what a field means.
    if format_version.split('.').next() != Some("1") {
        return Err(malformed(
            facts,
            &format!("unsupported plan format_version {format_version}"),
        ));
    }

    let Some(all) = plan["resource_changes"].as_array() else {
        if plan.get("resource_changes").is_some() {
            return Err(malformed(facts, &"resource_changes is not a list"));
        }

        facts
            .limitations
            .push("plan lists no resource_changes: nothing would change".into());
        facts.terraform = Some(TerraformPlan {
            format_version: format_version.to_owned(),
            terraform_version: plan["terraform_version"].as_str().map(str::to_owned),
            variables_sha256: None,
            resource_changes: Vec::new(),
        });
        return Ok(());
    };
    let mut resource_changes = Vec::new();
    for r in all {
        if resource_changes.len() == MAX_RECORDS {
            facts.limitations.push(format!(
                "{} resource changes over the limit were not read",
                all.len() - MAX_RECORDS
            ));
            break;
        }

        match resource(r) {
            Some(c) => resource_changes.push(c),
            // A change it cannot read is a limitation, never an empty plan.
            None if r["change"]["actions"].as_array().is_none()
                || r["address"].as_str().is_none() =>
            {
                facts
                    .limitations
                    .push("a resource change without address or actions was not read".into());
            }
            None => {}
        }
    }
    facts.terraform = Some(TerraformPlan {
        format_version: format_version.to_owned(),
        terraform_version: plan["terraform_version"].as_str().map(str::to_owned),
        variables_sha256: plan
            .get("variables")
            .and_then(|v| canonical_sha256(v).ok())
            .map(|d| d.to_string()),
        resource_changes,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use sych_core::{CiFormat, SourceSnapshotId, sha256};

    use super::*;

    fn facts() -> CiFacts {
        CiFacts::new(
            SourceSnapshotId(sha256(b"s")),
            CiFormat::TerraformPlan,
            "plan.json".into(),
        )
    }

    const PLAN: &str = r#"{
      "format_version": "1.2", "terraform_version": "1.9.5",
      "variables": {"db_password": {"value": "hunter2"}},
      "resource_changes": [
        {"address": "aws_db_instance.main", "change": {
          "actions": ["delete", "create"],
          "before": {"engine_version": "15", "password": "hunter2", "tags": {}},
          "after": {"engine_version": "16", "password": "hunter2", "tags": {}},
          "after_unknown": {"id": true, "endpoint": true},
          "before_sensitive": {"password": true}, "after_sensitive": {"password": true},
          "replace_paths": [["engine_version"]]}},
        {"address": "aws_lb.web", "previous_address": "aws_lb.old", "change": {
          "actions": ["create", "delete"], "before": {}, "after": {}, "after_unknown": {}}},
        {"address": "aws_s3_bucket.logs", "change": {"actions": ["no-op"]}},
        {"address": "data.aws_ami.x", "change": {"actions": ["read"]}}
      ]}"#;

    #[test]
    fn plan_keeps_structure_and_no_values() {
        let mut f = facts();
        parse(PLAN, &mut f).expect("plan");
        let plan = f.terraform.expect("terraform");
        assert_eq!(plan.resource_changes.len(), 2, "no-op and read are dropped");
        let db = &plan.resource_changes[0];
        assert_eq!(db.replace_order, Some(ReplaceOrder::DeleteBeforeCreate));
        assert_eq!(db.replace_paths, ["engine_version"]);
        assert_eq!(db.changed, ["engine_version"]);
        assert_eq!(db.unknown_after, ["endpoint", "id"]);
        assert_eq!(db.sensitive, ["password"]);
        let lb = &plan.resource_changes[1];
        assert_eq!(lb.replace_order, Some(ReplaceOrder::CreateBeforeDelete));
        assert_eq!(lb.moved_from.as_deref(), Some("aws_lb.old"));
        let kept = serde_json::to_string(&plan).expect("json");
        assert!(!kept.contains("hunter2"), "{kept}");
    }

    #[test]
    fn unsupported_major_and_garbage_are_errors_not_empty_plans() {
        assert!(
            parse(
                r#"{"format_version": "2.0", "resource_changes": []}"#,
                &mut facts()
            )
            .is_err()
        );
        assert!(
            parse(
                r#"{"format_version": "1.2", "resource_changes": {}}"#,
                &mut facts()
            )
            .is_err()
        );
        assert!(parse("not json", &mut facts()).is_err());
        let mut f = facts();
        parse(
            r#"{"format_version": "1.2", "resource_changes": [{"change": {"actions": ["delete"]}}]}"#,
            &mut f,
        )
        .expect("partial");
        assert!(f.limitations.iter().any(|l| l.contains("without address")));
    }
}
