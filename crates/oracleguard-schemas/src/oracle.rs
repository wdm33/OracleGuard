//! Oracle-observation canonical types.
//!
//! Owns: the fixed-width public shape of an oracle observation split
//! into an evaluation-domain struct (fields that may affect
//! authorization) and a provenance-domain struct (audit-only metadata).
//!
//! Does NOT own: Charli3 fetch, normalization policy, freshness
//! decisions, or any shell code. All of that lives in
//! `oracleguard-adapter` or `oracleguard-policy` and consumes the types
//! defined here. Cluster 3 refines the normalization semantics; Cluster
//! 2 only locks the public shape.

use serde::{Deserialize, Serialize};

/// Evaluation-domain oracle fact.
///
/// Contains only the fields that may legitimately affect an
/// authorization decision. All fields are fixed-width so canonical
/// byte identity does not depend on heap layout.
///
/// - `asset_pair` — canonical 16-byte asset-pair identifier (e.g. the
///   bytes of `"ADA/USD"` padded to 16 with zero bytes).
/// - `price_microusd` — the price from the oracle in integer
///   micro-USD (`1_000_000` microusd = 1 USD). Charli3's `price_map[0]`
///   is already in this unit on Preprod; no conversion is performed.
/// - `source` — canonical 16-byte oracle-source identifier
///   (e.g. `"charli3"` padded to 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OracleFactEvalV1 {
    pub asset_pair: [u8; 16],
    pub price_microusd: u64,
    pub source: [u8; 16],
}

/// Provenance-domain oracle metadata.
///
/// Contains audit-only fetch-side metadata. These fields MUST NOT
/// influence any authorization decision. Structural separation is the
/// mechanism by which that invariant is enforced: the evaluator
/// consumes only [`OracleFactEvalV1`], and this struct is carried
/// alongside for evidence purposes.
///
/// - `timestamp_unix` — oracle creation time, posix milliseconds
///   (Charli3 `price_map[1]` on Preprod).
/// - `expiry_unix` — oracle expiry, posix milliseconds (Charli3
///   `price_map[2]`). Used at most for freshness rejection, never for
///   authorization-relevant computation.
/// - `aggregator_utxo_ref` — 32-byte UTxO reference pointing at the
///   Charli3 `AggState` UTxO the adapter read. Enables offline
///   verifiers to re-query the on-chain source if desired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OracleFactProvenanceV1 {
    pub timestamp_unix: u64,
    pub expiry_unix: u64,
    pub aggregator_utxo_ref: [u8; 32],
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample_eval() -> OracleFactEvalV1 {
        OracleFactEvalV1 {
            asset_pair: *b"ADA/USD\0\0\0\0\0\0\0\0\0",
            price_microusd: 258_000,
            source: *b"charli3\0\0\0\0\0\0\0\0\0",
        }
    }

    fn sample_provenance() -> OracleFactProvenanceV1 {
        OracleFactProvenanceV1 {
            timestamp_unix: 1_713_000_000_000,
            expiry_unix: 1_713_000_300_000,
            aggregator_utxo_ref: [0x44; 32],
        }
    }

    // Destructuring guard: the pattern match below forces a compile
    // error if a field is silently added or removed from
    // `OracleFactEvalV1`. Adding a field is a canonical-byte change
    // and a Cluster 3 boundary change; it must not pass unnoticed.
    #[test]
    fn eval_struct_exposes_exact_fixed_width_fields() {
        let OracleFactEvalV1 {
            asset_pair,
            price_microusd,
            source,
        } = sample_eval();
        assert_eq!(asset_pair.len(), 16);
        assert_eq!(price_microusd, 258_000);
        assert_eq!(source.len(), 16);
    }

    // Destructuring guard: the pattern match below forces a compile
    // error if a field is silently added or removed from
    // `OracleFactProvenanceV1`. New provenance fields are allowed but
    // must be added deliberately and reviewed against the
    // non-interference tests before landing.
    #[test]
    fn provenance_struct_exposes_exact_fixed_width_fields() {
        let OracleFactProvenanceV1 {
            timestamp_unix,
            expiry_unix,
            aggregator_utxo_ref,
        } = sample_provenance();
        assert_eq!(timestamp_unix, 1_713_000_000_000);
        assert_eq!(expiry_unix, 1_713_000_300_000);
        assert_eq!(aggregator_utxo_ref.len(), 32);
    }

    #[test]
    fn eval_struct_is_copy_and_equates_by_value() {
        let a = sample_eval();
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn provenance_struct_is_copy_and_equates_by_value() {
        let a = sample_provenance();
        let b = a;
        assert_eq!(a, b);
    }
}
