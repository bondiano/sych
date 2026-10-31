//! Canonical JSON and SHA-256 identity.
//!
//! Object keys are sorted bytewise and output is compact, independent of field declaration
//! order or `serde_json/preserve_order`. Array order is kept: callers sort sets by their key
//! before hashing.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Sha256Digest;

pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut out = Vec::new();
    write_canonical(&serde_json::to_value(value)?, &mut out)?;
    Ok(out)
}

pub fn canonical_sha256<T: Serialize + ?Sized>(
    value: &T,
) -> Result<Sha256Digest, serde_json::Error> {
    Ok(sha256(&canonical_json(value)?))
}

pub fn sha256(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}

/// Deterministic `UUIDv8` from `parts`, so replaying the same inputs yields the same IDs.
pub fn derived_uuid(parts: &[&str]) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    let digest: [u8; 32] = hasher.finalize().into();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }

                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
            out.push(b'{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }

                serde_json::to_writer(&mut *out, key)?;
                out.push(b':');
                write_canonical(item, out)?;
            }
            out.push(b'}');
        }
        scalar => serde_json::to_writer(&mut *out, scalar)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn key_order_does_not_change_identity() {
        let a = json!({"b": 1, "a": {"y": [2, 1], "x": "ю"}});
        let b = json!({"a": {"x": "ю", "y": [2, 1]}, "b": 1});
        assert_eq!(canonical_json(&a).ok(), canonical_json(&b).ok());
        assert_eq!(
            canonical_json(&a).ok().as_deref(),
            Some(r#"{"a":{"x":"ю","y":[2,1]},"b":1}"#.as_bytes())
        );
    }

    #[test]
    fn array_order_is_significant() {
        assert_ne!(
            canonical_sha256(&json!([1, 2])).ok(),
            canonical_sha256(&json!([2, 1])).ok()
        );
    }
}
