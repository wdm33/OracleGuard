//! Cluster 3 Slice 05 — Provenance non-interference (schema layer).
//!
//! The Cluster 2 encoding tests already prove that provenance-only
//! mutations leave `intent_id` unchanged. This integration test goes
//! one level deeper: it asserts that provenance-only mutations leave
//! the *bytes* fed to the intent-id hash unchanged, and that the
//! full-intent canonical bytes differ by no more than the bytes of
//! `oracle_provenance` itself.
//!
//! If either property breaks, the eval/provenance boundary has
//! regressed and Cluster 3 must be reopened.

use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
use oracleguard_schemas::encoding::{encode_intent, encode_intent_identity, intent_id};
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};

fn base_intent() -> DisbursementIntentV1 {
    DisbursementIntentV1::new_v1(
        [0x11; 32],
        [0x22; 32],
        [0x33; 32],
        OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: 258_000,
            source: SOURCE_CHARLI3,
        },
        OracleFactProvenanceV1 {
            timestamp_unix: 1_776_428_823_000,
            expiry_unix: 1_776_429_423_000,
            aggregator_utxo_ref: [0x44; 32],
        },
        1_000_000_000,
        CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        },
        AssetIdV1::ADA,
    )
}

#[test]
fn identity_bytes_are_invariant_under_timestamp_mutation() {
    let base = encode_intent_identity(&base_intent()).expect("encode base");
    let mut mutated = base_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    let mutated_bytes = encode_intent_identity(&mutated).expect("encode mutated");
    assert_eq!(base, mutated_bytes);
}

#[test]
fn identity_bytes_are_invariant_under_expiry_mutation() {
    let base = encode_intent_identity(&base_intent()).expect("encode base");
    let mut mutated = base_intent();
    mutated.oracle_provenance.expiry_unix = u64::MAX;
    let mutated_bytes = encode_intent_identity(&mutated).expect("encode mutated");
    assert_eq!(base, mutated_bytes);
}

#[test]
fn identity_bytes_are_invariant_under_utxo_ref_mutation() {
    let base = encode_intent_identity(&base_intent()).expect("encode base");
    let mut mutated = base_intent();
    mutated.oracle_provenance.aggregator_utxo_ref = [0xFF; 32];
    let mutated_bytes = encode_intent_identity(&mutated).expect("encode mutated");
    assert_eq!(base, mutated_bytes);
}

#[test]
fn full_intent_bytes_change_when_provenance_changes() {
    // Dual of the above: the full canonical encoding IS sensitive to
    // provenance (so evidence can record it), but the identity
    // projection is not. This test pins the full-canonical surface as
    // provenance-sensitive by design.
    let base = encode_intent(&base_intent()).expect("encode base");
    let mut mutated = base_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    let mutated_bytes = encode_intent(&mutated).expect("encode mutated");
    assert_ne!(base, mutated_bytes);
}

#[test]
fn intent_id_is_invariant_under_any_provenance_mutation_combination() {
    let base = intent_id(&base_intent()).expect("id base");
    let mut mutated = base_intent();
    mutated.oracle_provenance.timestamp_unix = 0;
    mutated.oracle_provenance.expiry_unix = 1;
    mutated.oracle_provenance.aggregator_utxo_ref = [0xFF; 32];
    let mutated_id = intent_id(&mutated).expect("id mutated");
    assert_eq!(base, mutated_id);
}

#[test]
fn identity_projection_omits_provenance_bytes_entirely() {
    // The identity bytes must be strictly shorter than the full bytes,
    // and the full-bytes-minus-identity-bytes difference must be
    // exactly the bytes contributed by the provenance fields. We
    // cannot compute the exact postcard size without re-encoding, so
    // instead we encode provenance by itself and assert the diff is
    // that size.
    let full = encode_intent(&base_intent()).expect("full");
    let identity = encode_intent_identity(&base_intent()).expect("identity");
    let provenance_bytes =
        postcard::to_allocvec(&base_intent().oracle_provenance).expect("encode provenance");
    assert_eq!(full.len(), identity.len() + provenance_bytes.len());
}
