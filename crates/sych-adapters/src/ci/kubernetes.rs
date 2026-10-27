//! Rendered Kubernetes manifests (`helm template`, `kustomize build`): multi-document YAML
//! reduced to rollout structure. Env values, `ConfigMap` and `Secret` data are never read.

use serde::Deserialize;
use serde_json::Value;
use sych_core::CiFacts;
use sych_core::kubernetes::{K8sObject, RenderedManifests};

use super::{MAX_RECORDS, malformed};
use crate::ci::CiError;

const NORMALIZED: &[&str] = &[
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "Job",
    "CronJob",
    "Pod",
];
/// Built-in kinds with no rollout behaviour of their own.
const PASSIVE: &[&str] = &[
    "Service",
    "ConfigMap",
    "Secret",
    "Ingress",
    "ServiceAccount",
    "Role",
    "RoleBinding",
    "ClusterRole",
    "ClusterRoleBinding",
    "PersistentVolumeClaim",
    "Namespace",
    "NetworkPolicy",
    "HorizontalPodAutoscaler",
    "PodDisruptionBudget",
];

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |v, k| v.get(k))?.as_str()
}

fn names(items: &Value, path: &[&str]) -> Vec<String> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| str_at(i, path).map(str::to_owned))
        .collect()
}

fn pod_spec<'a>(kind: &str, spec: &'a Value) -> &'a Value {
    match kind {
        "Pod" => spec,
        "CronJob" => &spec["jobTemplate"]["spec"]["template"]["spec"],
        _ => &spec["template"]["spec"],
    }
}

fn object(doc: &Value, kind: &str) -> K8sObject {
    let spec = &doc["spec"];
    let pod = pod_spec(kind, spec);
    let containers: Vec<&Value> = ["initContainers", "containers"]
        .iter()
        .flat_map(|k| pod[*k].as_array().into_iter().flatten())
        .collect();
    let main = pod["containers"].as_array().into_iter().flatten();
    let annotations = doc["metadata"]["annotations"].as_object();
    let annotation = |k: &str| annotations.and_then(|a| a.get(k)).and_then(Value::as_str);
    let selector = spec["selector"]["matchLabels"].as_object();
    let labels = &spec["template"]["metadata"]["labels"];
    let mut secret_refs = names(&pod["volumes"], &["secret", "secretName"]);
    let mut config_refs = names(&pod["volumes"], &["configMap", "name"]);
    for c in &containers {
        secret_refs.extend(names(&c["env"], &["valueFrom", "secretKeyRef", "name"]));
        secret_refs.extend(names(&c["envFrom"], &["secretRef", "name"]));
        config_refs.extend(names(&c["env"], &["valueFrom", "configMapKeyRef", "name"]));
        config_refs.extend(names(&c["envFrom"], &["configMapRef", "name"]));
    }
    for refs in [&mut secret_refs, &mut config_refs] {
        refs.sort();
        refs.dedup();
    }
    let mut persistent_claims = names(&pod["volumes"], &["persistentVolumeClaim", "claimName"]);
    persistent_claims.extend(names(&spec["volumeClaimTemplates"], &["metadata", "name"]));
    K8sObject {
        kind: kind.to_owned(),
        name: str_at(doc, &["metadata", "name"]).unwrap_or("?").to_owned(),
        namespace: str_at(doc, &["metadata", "namespace"]).map(str::to_owned),
        replicas: spec["replicas"].as_u64(),
        strategy: str_at(spec, &["strategy", "type"])
            .or_else(|| str_at(spec, &["updateStrategy", "type"]))
            .map(str::to_owned),
        selector_mismatch: selector
            .is_some_and(|s| s.iter().any(|(k, v)| labels.get(k) != Some(v))),
        without_readiness: main
            .filter(|c| c.get("readinessProbe").is_none())
            .filter_map(|c| c["name"].as_str().map(str::to_owned))
            .collect(),
        termination_grace_seconds: pod["terminationGracePeriodSeconds"].as_u64(),
        pre_stop: containers
            .iter()
            .any(|c| c["lifecycle"].get("preStop").is_some()),
        persistent_claims,
        secret_refs,
        config_refs,
        hooks: [
            "helm.sh/hook",
            "argocd.argoproj.io/hook",
            "argocd.argoproj.io/sync-wave",
        ]
        .into_iter()
        .filter_map(|k| annotation(k).map(|v| format!("{k}={v}")))
        .collect(),
        deletion_policy: annotation("helm.sh/resource-policy")
            .map(|p| format!("helm.sh/resource-policy={p}"))
            .or_else(|| {
                annotation("argocd.argoproj.io/sync-options")
                    .filter(|o| o.contains("Prune=false"))
                    .map(|o| format!("argocd.argoproj.io/sync-options={o}"))
            }),
    }
}

pub fn parse(text: &str, facts: &mut CiFacts) -> Result<(), CiError> {
    let mut rendered = RenderedManifests::default();
    let mut docs = Vec::new();
    for doc in serde_yaml_ng::Deserializer::from_str(text) {
        let v = Value::deserialize(doc).map_err(|e| malformed(facts, &e))?;
        match v["kind"].as_str() {
            Some(k) if k.ends_with("List") => {
                docs.extend(v["items"].as_array().cloned().unwrap_or_default());
            }
            _ if v.is_null() => {}
            _ => docs.push(v),
        }
    }
    for doc in &docs {
        let Some(kind) = doc["kind"].as_str() else {
            return Err(malformed(facts, &"a document without kind"));
        };
        if NORMALIZED.contains(&kind) {
            if rendered.objects.len() == MAX_RECORDS {
                facts
                    .limitations
                    .push(format!("objects over {MAX_RECORDS} were not read"));
                break;
            }

            rendered.objects.push(object(doc, kind));
        } else if !PASSIVE.contains(&kind) && !rendered.unsupported_kinds.iter().any(|k| k == kind)
        {
            rendered.unsupported_kinds.push(kind.to_owned());
        }
    }
    facts.kubernetes = Some(rendered);
    Ok(())
}

#[cfg(test)]
mod tests {
    use sych_core::{CiFormat, SourceSnapshotId, sha256};

    use super::*;

    fn facts() -> CiFacts {
        CiFacts::new(
            SourceSnapshotId(sha256(b"s")),
            CiFormat::KubernetesRendered,
            "rendered.yaml".into(),
        )
    }

    const RENDERED: &str = r"---
apiVersion: apps/v1
kind: Deployment
metadata: {name: web, namespace: shop}
spec:
  replicas: 2
  strategy: {type: Recreate}
  selector: {matchLabels: {app: web}}
  template:
    metadata: {labels: {app: website}}
    spec:
      terminationGracePeriodSeconds: 5
      containers:
        - name: app
          env:
            - {name: DB_PASSWORD, valueFrom: {secretKeyRef: {name: db, key: password}}}
            - {name: MODE, value: hunter2}
      volumes:
        - {name: data, persistentVolumeClaim: {claimName: web-data}}
---
apiVersion: batch/v1
kind: Job
metadata:
  name: migrate
  annotations: {helm.sh/hook: pre-upgrade, helm.sh/hook-weight: '-5'}
spec:
  template:
    spec:
      containers: [{name: migrate, readinessProbe: {exec: {command: [true]}}}]
---
apiVersion: v1
kind: Secret
metadata: {name: db}
stringData: {password: hunter2}
---
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata: {name: web}
";

    #[test]
    fn rendered_manifests_keep_rollout_structure_and_no_values() {
        let mut f = facts();
        parse(RENDERED, &mut f).expect("rendered");
        let m = f.kubernetes.expect("kubernetes");
        assert_eq!(m.objects.len(), 2, "Secret data is never normalized");
        let web = &m.objects[0];
        assert!(web.selector_mismatch, "app: web vs app: website");
        assert_eq!(web.strategy.as_deref(), Some("Recreate"));
        assert_eq!(web.without_readiness, ["app"]);
        assert_eq!(web.persistent_claims, ["web-data"]);
        assert_eq!(web.secret_refs, ["db"]);
        let job = &m.objects[1];
        assert_eq!(job.hooks, ["helm.sh/hook=pre-upgrade"]);
        assert_eq!(m.unsupported_kinds, ["ServiceMonitor"]);
        let kept = serde_json::to_string(&m).expect("json");
        assert!(!kept.contains("hunter2"), "{kept}");
    }

    #[test]
    fn malformed_yaml_and_kindless_documents_are_errors() {
        assert!(parse("kind: [", &mut facts()).is_err());
        assert!(parse("apiVersion: v1\nmetadata: {name: x}\n", &mut facts()).is_err());
    }
}
