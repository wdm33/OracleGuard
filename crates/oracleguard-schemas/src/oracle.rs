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

/// Returned by [`check_freshness`] when the oracle datum's
/// `expiry_unix` has already passed at the given wall-clock value.
///
/// Kept as its own error type so freshness failure cannot be confused
/// with CBOR-shape or price-validation failures. Callers map this to
/// [`crate::reason::DisbursementReasonCode::OracleStale`] at the
/// authorization boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OracleStale {
    /// `OracleFactProvenanceV1::expiry_unix` — expiry posix ms from
    /// the datum.
    pub expiry_unix_ms: u64,
    /// Wall-clock value the freshness check was evaluated against.
    pub now_unix_ms: u64,
}

/// Pure freshness check for an [`OracleFactProvenanceV1`].
///
/// `now_unix_ms` is an explicit argument so this function stays
/// deterministic and testable. The caller — a shell — is responsible
/// for reading the wall clock. An expired datum must not reach the
/// evaluator; the grant gate treats [`OracleStale`] as the typed
/// rejection and maps it onto
/// [`crate::reason::DisbursementReasonCode::OracleStale`].
///
/// A datum is fresh when `now_unix_ms < expiry_unix`. Equality at
/// expiry is stale by design (the datum is no longer usable at the
/// instant it expires).
pub fn check_freshness(
    provenance: &OracleFactProvenanceV1,
    now_unix_ms: u64,
) -> Result<(), OracleStale> {
    if now_unix_ms < provenance.expiry_unix {
        Ok(())
    } else {
        Err(OracleStale {
            expiry_unix_ms: provenance.expiry_unix,
            now_unix_ms,
        })
    }
}

/// Canonical 16-byte identifier for the ADA/USD asset pair.
///
/// The seven ASCII bytes of `"ADA/USD"` followed by nine zero bytes.
/// All authoritative oracle paths use this exact byte sequence; no
/// alias, casing variant, or whitespace-padded form is accepted.
pub const ASSET_PAIR_ADA_USD: [u8; 16] = *b"ADA/USD\0\0\0\0\0\0\0\0\0";

/// Canonical 16-byte identifier for the Charli3 pull-oracle source.
///
/// The seven ASCII bytes of `"charli3"` followed by nine zero bytes.
/// All authoritative oracle paths use this exact byte sequence; no
/// alias, casing variant, or whitespace-padded form is accepted.
pub const SOURCE_CHARLI3: [u8; 16] = *b"charli3\0\0\0\0\0\0\0\0\0";

/// Map a human label to its canonical 16-byte asset-pair identifier.
///
/// The mapping is deliberately strict: only the exact label
/// `"ADA/USD"` returns `Some(ASSET_PAIR_ADA_USD)`. Any alias, casing
/// variant, or padded form returns `None`. This prevents two
/// semantically equivalent labels from producing different
/// evaluation-domain identity bytes.
pub fn canonical_asset_pair(label: &str) -> Option<[u8; 16]> {
    match label {
        "ADA/USD" => Some(ASSET_PAIR_ADA_USD),
        _ => None,
    }
}

/// Map a human label to its canonical 16-byte oracle-source identifier.
///
/// Strict mapping, for the same reasons as [`canonical_asset_pair`].
/// Only `"charli3"` maps; `"Charli3"`, `" charli3"`, and any other
/// variant return `None`.
pub fn canonical_source(label: &str) -> Option<[u8; 16]> {
    match label {
        "charli3" => Some(SOURCE_CHARLI3),
        _ => None,
    }
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

    // ---- Slice 03: canonical asset-pair and source identity ----

    #[test]
    fn asset_pair_constant_starts_with_ascii_ada_usd() {
        assert_eq!(&ASSET_PAIR_ADA_USD[..7], b"ADA/USD");
        assert_eq!(&ASSET_PAIR_ADA_USD[7..], &[0u8; 9]);
    }

    #[test]
    fn source_constant_starts_with_ascii_charli3() {
        assert_eq!(&SOURCE_CHARLI3[..7], b"charli3");
        assert_eq!(&SOURCE_CHARLI3[7..], &[0u8; 9]);
    }

    #[test]
    fn canonical_asset_pair_accepts_exact_ada_usd_label() {
        assert_eq!(canonical_asset_pair("ADA/USD"), Some(ASSET_PAIR_ADA_USD));
    }

    #[test]
    fn canonical_asset_pair_rejects_casing_variants() {
        assert_eq!(canonical_asset_pair("ada/usd"), None);
        assert_eq!(canonical_asset_pair("Ada/Usd"), None);
        assert_eq!(canonical_asset_pair("ADA/usd"), None);
    }

    #[test]
    fn canonical_asset_pair_rejects_whitespace_variants() {
        assert_eq!(canonical_asset_pair(" ADA/USD"), None);
        assert_eq!(canonical_asset_pair("ADA/USD "), None);
        assert_eq!(canonical_asset_pair("ADA / USD"), None);
    }

    #[test]
    fn canonical_asset_pair_rejects_separator_variants() {
        assert_eq!(canonical_asset_pair("ADA-USD"), None);
        assert_eq!(canonical_asset_pair("ADA_USD"), None);
        assert_eq!(canonical_asset_pair("ADAUSD"), None);
    }

    #[test]
    fn canonical_asset_pair_rejects_unknown_pairs() {
        assert_eq!(canonical_asset_pair("BTC/USD"), None);
        assert_eq!(canonical_asset_pair(""), None);
    }

    #[test]
    fn canonical_source_accepts_exact_charli3_label() {
        assert_eq!(canonical_source("charli3"), Some(SOURCE_CHARLI3));
    }

    #[test]
    fn canonical_source_rejects_casing_variants() {
        assert_eq!(canonical_source("Charli3"), None);
        assert_eq!(canonical_source("CHARLI3"), None);
        assert_eq!(canonical_source("charli_3"), None);
    }

    #[test]
    fn canonical_source_rejects_whitespace_variants() {
        assert_eq!(canonical_source(" charli3"), None);
        assert_eq!(canonical_source("charli3 "), None);
    }

    #[test]
    fn canonical_source_rejects_unknown_providers() {
        assert_eq!(canonical_source("pyth"), None);
        assert_eq!(canonical_source(""), None);
    }

    #[test]
    fn canonical_constants_round_trip_through_postcard() {
        // The constants must survive canonical encoding unchanged.
        // Any postcard-version bump that alters fixed-array layout
        // would surface here and be treated as a canonical-byte change.
        let eval = OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: 1,
            source: SOURCE_CHARLI3,
        };
        let bytes = postcard::to_allocvec(&eval).expect("encode");
        let (decoded, rest): (OracleFactEvalV1, _) =
            postcard::take_from_bytes(&bytes).expect("decode");
        assert!(rest.is_empty());
        assert_eq!(decoded.asset_pair, ASSET_PAIR_ADA_USD);
        assert_eq!(decoded.source, SOURCE_CHARLI3);
    }
}
