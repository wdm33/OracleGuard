//! Cluster 4 Slice 05 — Provenance non-interference (evaluator layer).
//!
//! The Cluster 3 closeout handoff requires an evaluator-layer proof
//! that `oracle_provenance` mutations do not change the evaluator's
//! output. The schemas crate's integration tests already prove this at
//! the canonical-bytes layer (identity projection omits provenance).
//! This file carries the same invariant forward to the evaluator's
//! `EvaluationResult`.
//!
//! If any test here fails, the eval/provenance boundary has regressed
//! at the decision layer and Cluster 3 must be reopened alongside
//! Cluster 4.

use oracleguard_policy::{evaluate_disbursement, EvaluationResult};
use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};

const ALLOCATION: u64 = 1_000_000_000;

fn allow_intent() -> DisbursementIntentV1 {
    // 700 ADA against a mid-band cap → Allow.
    DisbursementIntentV1::new_v1(
        [0x11; 32],
        [0x22; 32],
        [0x33; 32],
        OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: 450_000,
            source: SOURCE_CHARLI3,
        },
        OracleFactProvenanceV1 {
            timestamp_unix: 1_776_428_823_000,
            expiry_unix: 1_776_429_423_000,
            aggregator_utxo_ref: [0x44; 32],
        },
        700_000_000,
        CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        },
        AssetIdV1::ADA,
    )
}

fn deny_intent() -> DisbursementIntentV1 {
    // 900 ADA against a mid-band cap → Deny { ReleaseCapExceeded }.
    let mut i = allow_intent();
    i.requested_amount_lovelace = 900_000_000;
    i
}

#[test]
fn timestamp_mutation_preserves_allow_result() {
    let base = evaluate_disbursement(&allow_intent(), ALLOCATION);
    assert!(matches!(base, EvaluationResult::Allow { .. }));
    let mut mutated = allow_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
    mutated.oracle_provenance.timestamp_unix = u64::MAX;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn expiry_mutation_preserves_allow_result() {
    let base = evaluate_disbursement(&allow_intent(), ALLOCATION);
    let mut mutated = allow_intent();
    mutated.oracle_provenance.expiry_unix = 0;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
    mutated.oracle_provenance.expiry_unix = u64::MAX;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn utxo_ref_mutation_preserves_allow_result() {
    let base = evaluate_disbursement(&allow_intent(), ALLOCATION);
    let mut mutated = allow_intent();
    mutated.oracle_provenance.aggregator_utxo_ref = [0xFF; 32];
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn combined_provenance_mutation_preserves_allow_result() {
    let base = evaluate_disbursement(&allow_intent(), ALLOCATION);
    let mut mutated = allow_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    mutated.oracle_provenance.expiry_unix = 1;
    mutated.oracle_provenance.aggregator_utxo_ref = [0xCC; 32];
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn timestamp_mutation_preserves_deny_result() {
    let base = evaluate_disbursement(&deny_intent(), ALLOCATION);
    assert!(matches!(base, EvaluationResult::Deny { .. }));
    let mut mutated = deny_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
    mutated.oracle_provenance.timestamp_unix = u64::MAX;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn expiry_mutation_preserves_deny_result() {
    let base = evaluate_disbursement(&deny_intent(), ALLOCATION);
    let mut mutated = deny_intent();
    mutated.oracle_provenance.expiry_unix = 0;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
    mutated.oracle_provenance.expiry_unix = u64::MAX;
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn utxo_ref_mutation_preserves_deny_result() {
    let base = evaluate_disbursement(&deny_intent(), ALLOCATION);
    let mut mutated = deny_intent();
    mutated.oracle_provenance.aggregator_utxo_ref = [0xFF; 32];
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}

#[test]
fn combined_provenance_mutation_preserves_deny_result() {
    let base = evaluate_disbursement(&deny_intent(), ALLOCATION);
    let mut mutated = deny_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    mutated.oracle_provenance.expiry_unix = 1;
    mutated.oracle_provenance.aggregator_utxo_ref = [0xCC; 32];
    assert_eq!(base, evaluate_disbursement(&mutated, ALLOCATION));
}
