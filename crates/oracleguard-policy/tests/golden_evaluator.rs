//! Cluster 4 Slice 06 — Golden allow/deny fixtures and replay tests.
//!
//! The evaluator's public contract promises byte-identical replay for
//! the MVP allow and deny examples from Implementation Plan §16. This
//! file proves the promise by:
//!
//! - constructing the two demo intents from pinned byte values,
//! - postcard-encoding the resulting [`EvaluationResult`] values,
//! - asserting the bytes match a pair of golden fixtures checked into
//!   `fixtures/eval/`,
//! - running each evaluation several times and asserting that every
//!   run produces identical bytes.
//!
//! If any test here fails, the evaluator has produced a
//! non-replayable output for a demo anchor and Cluster 4 must be
//! reopened.
//!
//! Regenerating the fixtures — when the fixture shape itself changes
//! deliberately — uses the `#[ignore]`d regenerate test:
//!
//! ```text
//! cargo test -p oracleguard-policy --test golden_evaluator -- \
//!     --ignored regenerate
//! ```

use oracleguard_policy::{evaluate_disbursement, EvaluationResult};
use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};

/// Allocation basis for both demo scenarios: 1_000 ADA.
const DEMO_ALLOCATION_LOVELACE: u64 = 1_000_000_000;

/// Demo oracle price, in micro-USD: $0.45 → selects the 7_500 bps
/// mid band → 750 ADA cap on a 1_000 ADA allocation.
const DEMO_PRICE_MICROUSD: u64 = 450_000;

/// Allow scenario request amount: 700 ADA (within the 750 ADA cap).
const ALLOW_REQUEST_LOVELACE: u64 = 700_000_000;

/// Deny scenario request amount: 900 ADA (exceeds the 750 ADA cap).
const DENY_REQUEST_LOVELACE: u64 = 900_000_000;

const ALLOW_GOLDEN: &[u8] =
    include_bytes!("../../../fixtures/eval/allow_700_ada_result.postcard");
const DENY_GOLDEN: &[u8] =
    include_bytes!("../../../fixtures/eval/deny_900_ada_result.postcard");

fn demo_intent(requested_amount_lovelace: u64) -> DisbursementIntentV1 {
    DisbursementIntentV1::new_v1(
        [0x11; 32],
        [0x22; 32],
        [0x33; 32],
        OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: DEMO_PRICE_MICROUSD,
            source: SOURCE_CHARLI3,
        },
        OracleFactProvenanceV1 {
            timestamp_unix: 1_713_000_000_000,
            expiry_unix: 1_713_000_300_000,
            aggregator_utxo_ref: [0x44; 32],
        },
        requested_amount_lovelace,
        CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        },
        AssetIdV1::ADA,
    )
}

fn encode(result: &EvaluationResult) -> Vec<u8> {
    match postcard::to_allocvec(result) {
        Ok(bytes) => bytes,
        Err(e) => panic!("encode failed: {e:?}"),
    }
}

// ---- Structural sanity ----

#[test]
fn allow_fixture_produces_the_allow_variant() {
    let r = evaluate_disbursement(&demo_intent(ALLOW_REQUEST_LOVELACE), DEMO_ALLOCATION_LOVELACE);
    assert_eq!(
        r,
        EvaluationResult::Allow {
            authorized_amount_lovelace: ALLOW_REQUEST_LOVELACE,
            release_cap_basis_points: 7_500,
        }
    );
}

#[test]
fn deny_fixture_produces_the_deny_variant() {
    let r = evaluate_disbursement(&demo_intent(DENY_REQUEST_LOVELACE), DEMO_ALLOCATION_LOVELACE);
    assert!(matches!(r, EvaluationResult::Deny { .. }));
}

// ---- Golden byte identity ----

#[test]
fn allow_fixture_produces_golden_bytes() {
    let r = evaluate_disbursement(&demo_intent(ALLOW_REQUEST_LOVELACE), DEMO_ALLOCATION_LOVELACE);
    let bytes = encode(&r);
    assert_eq!(bytes.as_slice(), ALLOW_GOLDEN);
}

#[test]
fn deny_fixture_produces_golden_bytes() {
    let r = evaluate_disbursement(&demo_intent(DENY_REQUEST_LOVELACE), DEMO_ALLOCATION_LOVELACE);
    let bytes = encode(&r);
    assert_eq!(bytes.as_slice(), DENY_GOLDEN);
}

// ---- Golden byte-identical replay ----

#[test]
fn allow_fixture_replay_is_byte_identical() {
    let intent = demo_intent(ALLOW_REQUEST_LOVELACE);
    let runs: Vec<Vec<u8>> = (0..8)
        .map(|_| encode(&evaluate_disbursement(&intent, DEMO_ALLOCATION_LOVELACE)))
        .collect();
    for bytes in &runs {
        assert_eq!(bytes.as_slice(), ALLOW_GOLDEN);
    }
    for window in runs.windows(2) {
        assert_eq!(window[0], window[1]);
    }
}

#[test]
fn deny_fixture_replay_is_byte_identical() {
    let intent = demo_intent(DENY_REQUEST_LOVELACE);
    let runs: Vec<Vec<u8>> = (0..8)
        .map(|_| encode(&evaluate_disbursement(&intent, DEMO_ALLOCATION_LOVELACE)))
        .collect();
    for bytes in &runs {
        assert_eq!(bytes.as_slice(), DENY_GOLDEN);
    }
    for window in runs.windows(2) {
        assert_eq!(window[0], window[1]);
    }
}

// ---- Cross-check: allow and deny bytes must differ ----

#[test]
fn allow_and_deny_golden_bytes_differ() {
    // Guards against the fixture-copy-paste bug where both goldens
    // end up pointing at the same bytes.
    assert_ne!(ALLOW_GOLDEN, DENY_GOLDEN);
}

// ---- Round-trip: golden bytes decode back to the same result ----

#[test]
fn allow_golden_bytes_decode_back_to_same_result() {
    let (decoded, remainder): (EvaluationResult, _) =
        match postcard::take_from_bytes(ALLOW_GOLDEN) {
            Ok(v) => v,
            Err(e) => panic!("decode failed: {e:?}"),
        };
    assert!(remainder.is_empty(), "golden bytes had trailing data");
    let expected = evaluate_disbursement(
        &demo_intent(ALLOW_REQUEST_LOVELACE),
        DEMO_ALLOCATION_LOVELACE,
    );
    assert_eq!(decoded, expected);
}

#[test]
fn deny_golden_bytes_decode_back_to_same_result() {
    let (decoded, remainder): (EvaluationResult, _) =
        match postcard::take_from_bytes(DENY_GOLDEN) {
            Ok(v) => v,
            Err(e) => panic!("decode failed: {e:?}"),
        };
    assert!(remainder.is_empty(), "golden bytes had trailing data");
    let expected = evaluate_disbursement(
        &demo_intent(DENY_REQUEST_LOVELACE),
        DEMO_ALLOCATION_LOVELACE,
    );
    assert_eq!(decoded, expected);
}

// ---- Manual fixture regeneration ----
//
// Run with:
//     cargo test -p oracleguard-policy --test golden_evaluator -- \
//         --ignored regenerate
//
// This writes fresh bytes over `fixtures/eval/*.postcard`. Do this
// only when the fixture shape itself changes deliberately — any
// commit that touches the fixtures must include an explanation of
// why the canonical bytes moved.

#[test]
#[ignore = "regenerates golden fixtures; run manually only"]
fn regenerate_golden_fixtures() {
    let allow = evaluate_disbursement(
        &demo_intent(ALLOW_REQUEST_LOVELACE),
        DEMO_ALLOCATION_LOVELACE,
    );
    let deny = evaluate_disbursement(
        &demo_intent(DENY_REQUEST_LOVELACE),
        DEMO_ALLOCATION_LOVELACE,
    );
    let allow_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/eval/allow_700_ada_result.postcard"
    );
    let deny_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/eval/deny_900_ada_result.postcard"
    );
    std::fs::write(allow_path, encode(&allow)).expect("write allow golden");
    std::fs::write(deny_path, encode(&deny)).expect("write deny golden");
}
