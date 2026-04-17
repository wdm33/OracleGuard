//! Cluster 6 Slice 06 — routing determinism and boundary tests.
//!
//! Integration suite that proves the Cluster 6 boundary claims
//! mechanically (not just in prose):
//!
//! - routing is single-authority and invariant under non-authority
//!   field changes
//! - payload emission is byte-stable and round-trips
//! - CLI argument construction is deterministic and isolates field
//!   changes
//! - consensus-output intake is strict and lossless
//!
//! These assertions sit alongside the unit tests in each module; this
//! file exists so reviewers have one place to point when auditing the
//! cluster's "mechanically proven" claim.

use std::path::PathBuf;

use oracleguard_adapter::intake::{
    parse_cli_receipt, render_cli_receipt, IntentReceiptV1, IntentStatus,
};
use oracleguard_adapter::ziranity_submit::{build_cli_args, payload_bytes, SubmitConfig};
use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::encoding::{decode_intent, emit_payload, encode_intent};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::{DisbursementIntentV1, INTENT_VERSION_V1};
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};
use oracleguard_schemas::reason::DisbursementReasonCode;
use oracleguard_schemas::routing::{route, DISBURSEMENT_OCU_ID};

fn sample_intent() -> DisbursementIntentV1 {
    DisbursementIntentV1 {
        intent_version: INTENT_VERSION_V1,
        policy_ref: [0x11; 32],
        allocation_id: [0x22; 32],
        requester_id: [0x33; 32],
        oracle_fact: OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: 450_000,
            source: SOURCE_CHARLI3,
        },
        oracle_provenance: OracleFactProvenanceV1 {
            timestamp_unix: 1_713_000_000_000,
            expiry_unix: 1_713_000_300_000,
            aggregator_utxo_ref: [0x44; 32],
        },
        requested_amount_lovelace: 700_000_000,
        destination: CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        },
        asset: AssetIdV1::ADA,
    }
}

fn sample_effect() -> AuthorizedEffectV1 {
    AuthorizedEffectV1 {
        policy_ref: [0x11; 32],
        allocation_id: [0x22; 32],
        requester_id: [0x33; 32],
        destination: CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        },
        asset: AssetIdV1::ADA,
        authorized_amount_lovelace: 700_000_000,
        release_cap_basis_points: 7_500,
    }
}

fn sample_config() -> SubmitConfig {
    SubmitConfig::new_mvp(
        PathBuf::from("/tmp/og/intent.payload"),
        "127.0.0.1".to_string(),
        10_400,
        PathBuf::from("/tmp/og/devnet/bootstrap.bundle"),
    )
}

// -------------------------------------------------------------------
// Routing invariance under non-authority mutations
// -------------------------------------------------------------------

#[test]
fn routing_is_single_authority_for_sample_intent() {
    assert_eq!(route(&sample_intent()), DISBURSEMENT_OCU_ID);
}

#[test]
fn routing_is_invariant_over_64_combined_mutations() {
    // Sweep a mix of non-authority field changes and assert the
    // target never moves. This is the integration-level analogue of
    // the per-field unit tests in oracleguard-schemas::routing.
    let base = route(&sample_intent());
    for i in 0u64..64 {
        let mut m = sample_intent();
        m.destination.bytes[(i as usize) % 57] = i as u8;
        m.oracle_fact.price_microusd = i.wrapping_add(1);
        m.oracle_provenance.timestamp_unix = i;
        m.oracle_provenance.expiry_unix = i.wrapping_mul(2);
        m.oracle_provenance.aggregator_utxo_ref[(i as usize) % 32] = i as u8;
        m.requested_amount_lovelace = i.wrapping_add(1);
        m.requester_id[(i as usize) % 32] = i as u8;
        m.allocation_id[(i as usize) % 32] = i as u8;
        m.policy_ref[(i as usize) % 32] = i as u8;
        assert_eq!(route(&m), base, "routing drifted at i={i}");
    }
}

// -------------------------------------------------------------------
// Payload emission determinism + round-trip
// -------------------------------------------------------------------

#[test]
fn emit_payload_is_byte_stable_across_calls() {
    let intent = sample_intent();
    let a = emit_payload(&intent).expect("emit a");
    let b = emit_payload(&intent).expect("emit b");
    assert_eq!(a, b);
}

#[test]
fn emit_payload_equals_encode_intent_at_integration_scope() {
    let intent = sample_intent();
    assert_eq!(
        emit_payload(&intent).expect("emit"),
        encode_intent(&intent).expect("encode"),
    );
}

#[test]
fn emit_payload_round_trips_via_decode() {
    let intent = sample_intent();
    let bytes = emit_payload(&intent).expect("emit");
    let decoded = decode_intent(&bytes).expect("decode");
    assert_eq!(decoded, intent);
}

#[test]
fn payload_bytes_adapter_reexport_matches_schemas_emit() {
    let intent = sample_intent();
    assert_eq!(
        payload_bytes(&intent).expect("adapter"),
        emit_payload(&intent).expect("schemas"),
    );
}

// -------------------------------------------------------------------
// CLI wrapper determinism + field isolation
// -------------------------------------------------------------------

#[test]
fn build_cli_args_is_stable_for_identical_config() {
    let c = sample_config();
    assert_eq!(build_cli_args(&c), build_cli_args(&c));
}

#[test]
fn build_cli_args_object_id_defaults_to_fixed_ocu() {
    let args = build_cli_args(&sample_config());
    let pos = args.iter().position(|a| a == "--object_id").expect("flag");
    let hex = &args[pos + 1];
    assert_eq!(hex.len(), 64);
    let mut expected = String::with_capacity(64);
    for b in DISBURSEMENT_OCU_ID {
        expected.push_str(&format!("{b:02x}"));
    }
    assert_eq!(hex, &expected);
}

#[test]
fn build_cli_args_destination_agnosticism_via_routing() {
    // Two intents differing only in destination must still route to
    // the same --object_id arg. Proves that a destination change in
    // the intent cannot leak into the submission wrapper.
    let intent_a = sample_intent();
    let mut intent_b = sample_intent();
    intent_b.destination.bytes[0] = 0xAB;

    let target_a = route(&intent_a);
    let target_b = route(&intent_b);
    assert_eq!(target_a, target_b);

    let config = SubmitConfig {
        object_id: target_a,
        ..sample_config()
    };
    let args_a = build_cli_args(&config);
    let config_b = SubmitConfig {
        object_id: target_b,
        ..sample_config()
    };
    let args_b = build_cli_args(&config_b);
    assert_eq!(args_a, args_b);
}

#[test]
fn build_cli_args_amount_agnosticism_via_routing() {
    // Same property for requested_amount_lovelace.
    let intent_a = sample_intent();
    let mut intent_b = sample_intent();
    intent_b.requested_amount_lovelace = 1;

    let config_a = SubmitConfig {
        object_id: route(&intent_a),
        ..sample_config()
    };
    let config_b = SubmitConfig {
        object_id: route(&intent_b),
        ..sample_config()
    };
    assert_eq!(build_cli_args(&config_a), build_cli_args(&config_b));
}

// -------------------------------------------------------------------
// Intake round-trip
// -------------------------------------------------------------------

#[test]
fn intake_round_trips_authorized_receipt() {
    let receipt = IntentReceiptV1 {
        intent_id: [0xAA; 32],
        status: IntentStatus::Committed,
        committed_height: 42,
        output: AuthorizationResult::Authorized {
            effect: sample_effect(),
        },
    };
    let bytes = render_cli_receipt(&receipt).expect("render");
    let parsed = parse_cli_receipt(&bytes).expect("parse");
    assert_eq!(parsed, receipt);
}

#[test]
fn intake_round_trips_denied_receipt() {
    let receipt = IntentReceiptV1 {
        intent_id: [0xBB; 32],
        status: IntentStatus::Committed,
        committed_height: 100,
        output: AuthorizationResult::Denied {
            reason: DisbursementReasonCode::ReleaseCapExceeded,
            gate: AuthorizationGate::Grant,
        },
    };
    let bytes = render_cli_receipt(&receipt).expect("render");
    let parsed = parse_cli_receipt(&bytes).expect("parse");
    assert_eq!(parsed, receipt);
}

#[test]
fn intake_is_stable_across_parse_calls() {
    let receipt = IntentReceiptV1 {
        intent_id: [0xCC; 32],
        status: IntentStatus::Committed,
        committed_height: 7,
        output: AuthorizationResult::Authorized {
            effect: sample_effect(),
        },
    };
    let bytes = render_cli_receipt(&receipt).expect("render");
    let a = parse_cli_receipt(&bytes).expect("a");
    let b = parse_cli_receipt(&bytes).expect("b");
    assert_eq!(a, b);
}

// -------------------------------------------------------------------
// End-to-end (sans live CLI): intent → route → args → receipt → parse
// -------------------------------------------------------------------

#[test]
fn end_to_end_cluster_6_boundary_is_byte_stable() {
    let intent = sample_intent();

    // Emit payload.
    let payload = emit_payload(&intent).expect("emit");
    let re_decoded = decode_intent(&payload).expect("decode");
    assert_eq!(re_decoded, intent);

    // Route.
    let target = route(&intent);
    assert_eq!(target, DISBURSEMENT_OCU_ID);

    // Build CLI args bound to the routed target.
    let config = SubmitConfig {
        object_id: target,
        ..sample_config()
    };
    let args_1 = build_cli_args(&config);
    let args_2 = build_cli_args(&config);
    assert_eq!(args_1, args_2);

    // Simulate consensus output (what the private-side Ziranity
    // runtime would produce via the three-gate closure).
    let receipt = IntentReceiptV1 {
        intent_id: [0xDD; 32],
        status: IntentStatus::Committed,
        committed_height: 1_000,
        output: AuthorizationResult::Authorized {
            effect: sample_effect(),
        },
    };
    let receipt_bytes = render_cli_receipt(&receipt).expect("render");
    let parsed = parse_cli_receipt(&receipt_bytes).expect("parse");

    // The parsed output is read verbatim, not recomputed.
    match parsed.output {
        AuthorizationResult::Authorized { effect } => {
            assert_eq!(effect.authorized_amount_lovelace, 700_000_000);
            assert_eq!(effect.release_cap_basis_points, 7_500);
            assert_eq!(effect.destination, intent.destination);
        }
        AuthorizationResult::Denied { .. } => panic!("expected Authorized"),
    }
}
