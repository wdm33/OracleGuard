//! Canonical policy bytes.
//!
//! Owns: canonicalization of policy JSON into stable bytes suitable for
//! hashing. `policy_ref` derivation (slice 02) consumes the output of
//! this module and produces the authoritative 32-byte policy identity.
//!
//! Does NOT own: storage, retrieval, or transport of policy documents
//! (those are shell concerns) and does NOT adjudicate policy
//! correctness (Katiba is the policy authority).

use serde_json::Value;

/// Errors returned by [`canonicalize_policy_json`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyCanonError {
    /// Input was not well-formed JSON.
    InvalidJson,
    /// A JSON number with a fractional part or exponent was encountered.
    /// OracleGuard forbids floats on the authoritative surface.
    FloatForbidden,
    /// Serializing a string or key to canonical JSON failed. Should not
    /// occur for valid `serde_json::Value` inputs; surfaced rather than
    /// panicked.
    EncodeFailure,
}

/// Produce canonical policy bytes from a JSON policy document.
///
/// The canonical form is:
/// 1. strict JSON parse,
/// 2. rejection of any floating-point number,
/// 3. recursive lexicographic sort of object keys by key bytes,
/// 4. compact output with no insignificant whitespace.
///
/// See `docs/policy-canonicalization.md` for the full specification.
pub fn canonicalize_policy_json(raw: &[u8]) -> Result<Vec<u8>, PolicyCanonError> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| PolicyCanonError::InvalidJson)?;
    let mut out: Vec<u8> = Vec::new();
    write_canonical(&value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), PolicyCanonError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if n.is_f64() {
                return Err(PolicyCanonError::FloatForbidden);
            }
            let text = n.to_string();
            out.extend_from_slice(text.as_bytes());
        }
        Value::String(s) => {
            let encoded = serde_json::to_string(s).map_err(|_| PolicyCanonError::EncodeFailure)?;
            out.extend_from_slice(encoded.as_bytes());
        }
        Value::Array(arr) => {
            out.push(b'[');
            let mut first = true;
            for item in arr {
                if !first {
                    out.push(b',');
                }
                first = false;
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(obj) => {
            let mut entries: Vec<(&String, &Value)> = obj.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push(b'{');
            let mut first = true;
            for (k, v) in entries {
                if !first {
                    out.push(b',');
                }
                first = false;
                let encoded_key =
                    serde_json::to_string(k).map_err(|_| PolicyCanonError::EncodeFailure)?;
                out.extend_from_slice(encoded_key.as_bytes());
                out.push(b':');
                write_canonical(v, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout
)]
mod tests {
    use super::*;

    const FIXTURE_PRETTY: &[u8] = include_bytes!("../../../fixtures/policy_v1.json");
    const FIXTURE_CANONICAL: &[u8] = include_bytes!("../../../fixtures/policy_v1.canonical.bytes");

    fn canonicalize(raw: &[u8]) -> Vec<u8> {
        match canonicalize_policy_json(raw) {
            Ok(bytes) => bytes,
            Err(e) => panic!("canonicalize failed: {e:?}"),
        }
    }

    #[test]
    fn canonical_bytes_stable_across_loads() {
        let first = canonicalize(FIXTURE_PRETTY);
        let second = canonicalize(FIXTURE_PRETTY);
        assert_eq!(first, second);
    }

    #[test]
    fn canonical_bytes_match_golden() {
        let bytes = canonicalize(FIXTURE_PRETTY);
        assert_eq!(bytes.as_slice(), FIXTURE_CANONICAL);
    }

    #[test]
    fn whitespace_and_key_order_do_not_affect_canonical_bytes() {
        let a = br#"{"schema":"oracleguard.policy.v1","policy_version":1,"anchored_commitment":"katiba://policy/constitutional-release/v1","release_cap_basis_points":7500,"allowed_assets":["ADA"]}"#;
        let b = br#"
            {
                "allowed_assets" : [ "ADA" ],
                "release_cap_basis_points" : 7500,
                "policy_version"          :1,
                "anchored_commitment":"katiba://policy/constitutional-release/v1",
                "schema": "oracleguard.policy.v1"
            }
        "#;
        let ca = canonicalize(a);
        let cb = canonicalize(b);
        assert_eq!(ca, cb);
    }

    #[test]
    fn nested_objects_have_keys_sorted_at_every_depth() {
        let input = br#"{"outer_z":{"b":1,"a":2},"outer_a":[{"z":1,"a":2}]}"#;
        let bytes = canonicalize(input);
        let expected = br#"{"outer_a":[{"a":2,"z":1}],"outer_z":{"a":2,"b":1}}"#;
        assert_eq!(bytes.as_slice(), &expected[..]);
    }

    #[test]
    fn float_value_is_rejected() {
        let input = br#"{"release_cap_basis_points":75.0}"#;
        let err = canonicalize_policy_json(input).expect_err("expected float rejection");
        assert_eq!(err, PolicyCanonError::FloatForbidden);
    }

    #[test]
    fn float_with_exponent_is_rejected() {
        let input = br#"{"x":1e2}"#;
        let err = canonicalize_policy_json(input).expect_err("expected float rejection");
        assert_eq!(err, PolicyCanonError::FloatForbidden);
    }

    #[test]
    fn malformed_json_is_rejected() {
        let input = br#"{"unterminated": "#;
        let err = canonicalize_policy_json(input).expect_err("expected parse error");
        assert_eq!(err, PolicyCanonError::InvalidJson);
    }

    #[test]
    fn mutation_of_semantic_bytes_changes_canonical_output() {
        let original = canonicalize(FIXTURE_PRETTY);
        let mutated = br#"{"schema":"oracleguard.policy.v1","policy_version":1,"anchored_commitment":"katiba://policy/constitutional-release/v1","release_cap_basis_points":5000,"allowed_assets":["ADA"]}"#;
        let mutated_bytes = canonicalize(mutated);
        assert_ne!(original, mutated_bytes);
    }

    #[test]
    fn non_canonical_raw_bytes_differ_from_canonical_bytes() {
        // The pretty-printed fixture itself must not be confused with its
        // canonical form. A reader who skips canonicalization and hashes
        // the pretty bytes would get a different value.
        assert_ne!(FIXTURE_PRETTY, FIXTURE_CANONICAL);
    }
}
