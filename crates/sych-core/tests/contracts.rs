//! Fixture-level contracts: old fixtures stay readable, versions are enforced,
//! canonical hashes and generated schemas are reproducible.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::{env, fs};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sych_core::api;
use sych_core::{
    DecodeError, Evidence, ObligationStatus, PolicySnapshot, ReviewResult, SourceSnapshot,
    canonical_json, canonical_sha256, decode, schema,
};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> String {
    fs::read_to_string(root().join("fixtures/core").join(name)).expect("fixture is readable")
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(name: &str) -> T {
    let value: T = decode(&fixture(name)).expect("v1 fixture decodes");
    let json = serde_json::to_string(&value).expect("serializes");
    assert_eq!(decode::<T>(&json).expect("re-decodes"), value, "{name}");
    value
}

#[test]
fn v1_fixtures_decode_and_round_trip() {
    round_trip::<SourceSnapshot>("v1/source-snapshot.json");
    round_trip::<PolicySnapshot>("v1/policy-snapshot.json");
    let evidence = round_trip::<Evidence>("v1/evidence.json");
    // Fields added with defaults must not break older documents that omit them.
    assert!(evidence.code_refs.is_empty() && evidence.limitations.is_empty());
    let result = round_trip::<ReviewResult>("v1/review-result.json");
    assert!(matches!(
        &result.obligations[1].status,
        ObligationStatus::Unknown { reason_code, .. } if reason_code == "SEMANTIC_PROVIDER_UNAVAILABLE"
    ));
}

#[test]
fn unknown_major_version_is_rejected_explicitly() {
    assert!(matches!(
        decode::<SourceSnapshot>(&fixture("source-snapshot-v2.json")),
        Err(DecodeError::UnsupportedVersion { found: 2 })
    ));
    let unversioned = fixture("v1/source-snapshot.json").replace("\"schema_version\": 1,", "");
    assert!(matches!(
        decode::<SourceSnapshot>(&unversioned),
        Err(DecodeError::MissingVersion)
    ));
}

#[test]
fn unknown_obligation_requires_reason_and_next_evidence() {
    let broken = fixture("v1/review-result.json")
        .replace("\"reason_code\": \"SEMANTIC_PROVIDER_UNAVAILABLE\",", "");
    assert!(matches!(
        decode::<ReviewResult>(&broken),
        Err(DecodeError::Invalid(_))
    ));
}

#[test]
fn canonical_hash_is_stable() {
    let result: ReviewResult = decode(&fixture("v1/review-result.json")).expect("decodes");
    let reordered: serde_json::Value =
        serde_json::from_str(&fixture("v1/review-result.json")).expect("json");
    assert_eq!(
        canonical_json(&result).expect("canonical"),
        canonical_json(&reordered).expect("canonical"),
    );
    assert_eq!(
        canonical_sha256(&result).expect("hash").to_string(),
        fixture("v1/review-result.sha256").trim(),
    );
}

/// Regenerate with `SYCH_BLESS=1 cargo test -p sych-core --test contracts` (`just schemas`).
#[test]
fn generated_schemas_match_committed() {
    let bless = env::var_os("SYCH_BLESS").is_some();
    for (stem, schema) in schema::document_schemas() {
        let path = root().join("schemas").join(format!("{stem}.schema.json"));
        let generated = serde_json::to_string_pretty(&schema).expect("schema serializes") + "\n";
        if bless {
            fs::write(&path, &generated).expect("schema is writable");
        } else {
            let committed = fs::read_to_string(&path).unwrap_or_default();
            assert!(
                committed == generated,
                "{} is stale; run `just schemas`",
                path.display()
            );
        }
    }
}

/// Every `$ref` of the `OpenAPI` document resolves inside it, so generators can read it.
#[test]
fn every_openapi_reference_resolves() {
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::String(r)) = o.get("$ref") {
                    out.push(r.clone());
                }

                o.values().for_each(|v| refs(v, out));
            }
            Value::Array(a) => a.iter().for_each(|v| refs(v, out)),
            _ => {}
        }
    }

    let doc = api::openapi();
    let mut found = Vec::new();
    refs(&doc, &mut found);
    assert!(!found.is_empty());
    for r in found {
        let target = r.strip_prefix('#').and_then(|p| doc.pointer(p));
        assert!(target.is_some(), "{r} does not resolve");
    }
}

/// Every `{name}` of a path template is a required path parameter of each of its operations.
#[test]
fn every_path_template_names_its_parameters() {
    let doc = api::openapi();
    let paths = doc["paths"].as_object().expect("paths");
    for (path, item) in paths {
        let names: Vec<&str> = path
            .split('/')
            .filter_map(|s| s.strip_prefix('{')?.strip_suffix('}'))
            .collect();
        for (method, operation) in item.as_object().expect("item") {
            for name in &names {
                let declared = operation["parameters"].as_array().is_some_and(|ps| {
                    ps.iter()
                        .any(|p| p["in"] == "path" && p["name"] == *name && p["required"] == true)
                });
                assert!(declared, "{method} {path} does not declare {{{name}}}");
            }
        }
    }
}

/// The `OpenAPI` description of `/v1`, generated from the same DTOs (`just schemas`).
#[test]
fn generated_openapi_matches_committed() {
    let path = root().join("schemas/openapi.json");
    let generated = serde_json::to_string_pretty(&api::openapi()).expect("openapi") + "\n";
    if env::var_os("SYCH_BLESS").is_some() {
        fs::write(&path, &generated).expect("openapi is writable");
    }

    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale; run `just schemas`",
        path.display()
    );
}
