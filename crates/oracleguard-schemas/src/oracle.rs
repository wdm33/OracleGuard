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
