//! Cluster 7 Slice 06 — fulfillment boundary and determinism tests.
//!
//! Integration suite that proves the Cluster 7 fulfillment boundary
//! mechanically (not just in prose):
//!
//! - exact effect-to-settlement field copy-through
//! - ADA-only MVP asset guard
//! - deny/no-tx closure across every canonical reason code
//! - pending-receipt closure
//! - allow-path tx-hash capture via a deterministic fixture backend
//! - end-to-end byte stability of the Cluster 6 → Cluster 7 chain
//!
//! These assertions sit alongside the unit tests in
//! `adapter::settlement` and `adapter::cardano`; this file exists so
//! reviewers have one place to point when auditing the cluster's
//! "mechanically proven" claim.

use std::cell::RefCell;
use std::path::PathBuf;

use oracleguard_adapter::cardano::{CardanoTxHashV1, SettlementBackend, SettlementSubmitError};
use oracleguard_adapter::intake::{
    parse_cli_receipt, render_cli_receipt, IntentReceiptV1, IntentStatus,
};
use oracleguard_adapter::settlement::{
    fulfill, guard_mvp_asset, settlement_request_from_effect, FulfillmentOutcome,
    FulfillmentRejection, SettlementRequest,
};
use oracleguard_adapter::ziranity_submit::{build_cli_args, SubmitConfig};
use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::encoding::{decode_intent, emit_payload};
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

fn committed_authorized_receipt(effect: AuthorizedEffectV1) -> IntentReceiptV1 {
    IntentReceiptV1 {
        intent_id: [0xAA; 32],
        status: IntentStatus::Committed,
        committed_height: 100,
        output: AuthorizationResult::Authorized { effect },
    }
}

fn committed_denied_receipt(
    reason: DisbursementReasonCode,
    gate: AuthorizationGate,
) -> IntentReceiptV1 {
    IntentReceiptV1 {
        intent_id: [0xBB; 32],
        status: IntentStatus::Committed,
        committed_height: 100,
        output: AuthorizationResult::Denied { reason, gate },
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

/// Deterministic fixture backend that records every settlement
/// request and returns a pinned tx hash. Seeded substitute — not a
/// mock of any functional core.
struct CountingFixtureBackend {
    tx_hash: CardanoTxHashV1,
    requests: RefCell<Vec<SettlementRequest>>,
}

impl CountingFixtureBackend {
    fn new(tx_hash: CardanoTxHashV1) -> Self {
        Self {
            tx_hash,
            requests: RefCell::new(Vec::new()),
        }
    }

    fn submit_count(&self) -> usize {
        self.requests.borrow().len()
    }

    fn last_request(&self) -> Option<SettlementRequest> {
        self.requests.borrow().last().copied()
    }
}

impl SettlementBackend for CountingFixtureBackend {
    fn submit(
        &self,
        request: &SettlementRequest,
    ) -> Result<CardanoTxHashV1, SettlementSubmitError> {
        self.requests.borrow_mut().push(*request);
        Ok(self.tx_hash)
    }
}

// -------------------------------------------------------------------
// Slice 02 — exact copy-through (integration scope)
// -------------------------------------------------------------------

#[test]
fn copy_through_is_exact_across_amount_sweep() {
    for amount in [
        0u64,
        1,
        100,
        1_000_000,
        700_000_000,
        1_000_000_000,
        u64::MAX / 2,
        u64::MAX,
    ] {
        let mut effect = sample_effect();
        effect.authorized_amount_lovelace = amount;
        let req = settlement_request_from_effect(&effect, [0x00; 32]);
        assert_eq!(req.amount_lovelace, amount);
        assert_eq!(req.destination, effect.destination);
        assert_eq!(req.asset, effect.asset);
    }
}

#[test]
fn copy_through_is_exact_across_destination_sweep() {
    for i in 0u8..32 {
        let mut effect = sample_effect();
        effect.destination.bytes[i as usize] = i.wrapping_add(0x80);
        let req = settlement_request_from_effect(&effect, [0x00; 32]);
        assert_eq!(req.destination.bytes, effect.destination.bytes);
        assert_eq!(req.destination.length, effect.destination.length);
    }
}

#[test]
fn copy_through_is_independent_of_policy_allocation_requester_and_cap() {
    let mut base = sample_effect();
    base.policy_ref = [0xAA; 32];
    base.allocation_id = [0xBB; 32];
    base.requester_id = [0xCC; 32];
    base.release_cap_basis_points = 5_000;

    let mut other = base;
    other.policy_ref = [0xDD; 32];
    other.allocation_id = [0xEE; 32];
    other.requester_id = [0xFF; 32];
    other.release_cap_basis_points = 10_000;

    let req_a = settlement_request_from_effect(&base, [0x00; 32]);
    let req_b = settlement_request_from_effect(&other, [0x00; 32]);
    assert_eq!(req_a, req_b);
}

// -------------------------------------------------------------------
// Slice 03 — ADA-only guard at integration scope
// -------------------------------------------------------------------

#[test]
fn guard_rejects_any_non_ada_asset() {
    let mut effect = sample_effect();
    effect.asset.policy_id = [0x01; 28];
    assert_eq!(
        guard_mvp_asset(&effect),
        Err(FulfillmentRejection::NonAdaAsset),
    );
}

// -------------------------------------------------------------------
// Slice 04 — deny / no-tx closure at integration scope
// -------------------------------------------------------------------

#[test]
fn deny_path_never_calls_backend_across_all_reason_codes() {
    let cases = [
        (
            DisbursementReasonCode::PolicyNotFound,
            AuthorizationGate::Anchor,
        ),
        (
            DisbursementReasonCode::AllocationNotFound,
            AuthorizationGate::Registry,
        ),
        (
            DisbursementReasonCode::SubjectNotAuthorized,
            AuthorizationGate::Registry,
        ),
        (
            DisbursementReasonCode::AssetMismatch,
            AuthorizationGate::Registry,
        ),
        (
            DisbursementReasonCode::OracleStale,
            AuthorizationGate::Grant,
        ),
        (
            DisbursementReasonCode::OraclePriceZero,
            AuthorizationGate::Grant,
        ),
        (DisbursementReasonCode::AmountZero, AuthorizationGate::Grant),
        (
            DisbursementReasonCode::ReleaseCapExceeded,
            AuthorizationGate::Grant,
        ),
    ];
    for (reason, gate) in cases {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
        let receipt = committed_denied_receipt(reason, gate);
        let out = fulfill(&receipt, &backend).expect("fulfill");
        assert_eq!(out, FulfillmentOutcome::DeniedUpstream { reason, gate });
        assert_eq!(backend.submit_count(), 0, "backend called for {reason:?}");
    }
}

#[test]
fn non_ada_authorized_effect_short_circuits_at_integration_scope() {
    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
    let mut effect = sample_effect();
    effect.asset.policy_id = [0xAB; 28];
    let receipt = committed_authorized_receipt(effect);
    let out = fulfill(&receipt, &backend).expect("fulfill");
    assert_eq!(
        out,
        FulfillmentOutcome::RejectedAtFulfillment {
            kind: FulfillmentRejection::NonAdaAsset,
        }
    );
    assert_eq!(backend.submit_count(), 0);
}

#[test]
fn pending_receipt_never_calls_backend() {
    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
    let receipt = IntentReceiptV1 {
        intent_id: [0xCC; 32],
        status: IntentStatus::Pending,
        committed_height: 0,
        output: AuthorizationResult::Authorized {
            effect: sample_effect(),
        },
    };
    let out = fulfill(&receipt, &backend).expect("fulfill");
    assert_eq!(
        out,
        FulfillmentOutcome::RejectedAtFulfillment {
            kind: FulfillmentRejection::ReceiptNotCommitted,
        }
    );
    assert_eq!(backend.submit_count(), 0);
}

// -------------------------------------------------------------------
// Slice 05 — allow-path tx-hash capture
// -------------------------------------------------------------------

#[test]
fn allow_path_captures_tx_hash_from_backend() {
    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xCD; 32]));
    let receipt = committed_authorized_receipt(sample_effect());
    let out = fulfill(&receipt, &backend).expect("fulfill");
    assert_eq!(
        out,
        FulfillmentOutcome::Settled {
            tx_hash: CardanoTxHashV1([0xCD; 32]),
        }
    );
    assert_eq!(backend.submit_count(), 1);
}

#[test]
fn allow_path_request_amount_equals_authorized_amount_for_sweep() {
    for amount in [1u64, 700_000_000, 1_000_000_000, u64::MAX] {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xEE; 32]));
        let mut effect = sample_effect();
        effect.authorized_amount_lovelace = amount;
        let receipt = committed_authorized_receipt(effect);
        fulfill(&receipt, &backend).expect("fulfill");
        let req = backend.last_request().expect("one request");
        assert_eq!(req.amount_lovelace, amount);
    }
}

#[test]
fn allow_path_is_deterministic_across_repeats() {
    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xEF; 32]));
    let receipt = committed_authorized_receipt(sample_effect());
    let mut outs = Vec::new();
    for _ in 0..4 {
        outs.push(fulfill(&receipt, &backend).expect("fulfill"));
    }
    for window in outs.windows(2) {
        assert_eq!(window[0], window[1]);
    }
    assert_eq!(backend.submit_count(), 4);
}

// -------------------------------------------------------------------
// End-to-end Cluster 6 → Cluster 7 byte stability
// -------------------------------------------------------------------

#[test]
fn end_to_end_cluster_6_to_7_chain_is_byte_stable() {
    let intent = sample_intent();

    // Cluster 6 surfaces: payload + route + args.
    let payload = emit_payload(&intent).expect("emit");
    let re_decoded = decode_intent(&payload).expect("decode");
    assert_eq!(re_decoded, intent);
    assert_eq!(route(&intent), DISBURSEMENT_OCU_ID);

    let config = SubmitConfig {
        object_id: route(&intent),
        ..sample_config()
    };
    let _args = build_cli_args(&config);

    // Simulated consensus output.
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

    // Cluster 7: fulfill against deterministic backend.
    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xAB; 32]));
    let out = fulfill(&parsed, &backend).expect("fulfill");

    assert_eq!(
        out,
        FulfillmentOutcome::Settled {
            tx_hash: CardanoTxHashV1([0xAB; 32]),
        }
    );
    assert_eq!(backend.submit_count(), 1);

    // The captured request must mirror the intent's
    // destination/asset/amount byte-for-byte.
    let req = backend.last_request().expect("one request");
    assert_eq!(req.destination, intent.destination);
    assert_eq!(req.asset, intent.asset);
    assert_eq!(req.amount_lovelace, intent.requested_amount_lovelace);
    assert_eq!(req.intent_id, receipt.intent_id);
}

#[test]
fn end_to_end_deny_path_produces_no_cardano_tx() {
    let intent = sample_intent();
    let _payload = emit_payload(&intent).expect("emit");

    // Simulated deny-path receipt.
    let receipt = committed_denied_receipt(
        DisbursementReasonCode::OracleStale,
        AuthorizationGate::Grant,
    );
    let receipt_bytes = render_cli_receipt(&receipt).expect("render");
    let parsed = parse_cli_receipt(&receipt_bytes).expect("parse");

    let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xAB; 32]));
    let out = fulfill(&parsed, &backend).expect("fulfill");

    assert_eq!(
        out,
        FulfillmentOutcome::DeniedUpstream {
            reason: DisbursementReasonCode::OracleStale,
            gate: AuthorizationGate::Grant,
        }
    );
    assert_eq!(backend.submit_count(), 0);
}
