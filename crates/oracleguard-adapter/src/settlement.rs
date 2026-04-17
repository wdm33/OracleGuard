//! Cardano fulfillment boundary.
//!
//! Owns: the pure field-copy mapping from
//! [`oracleguard_schemas::effect::AuthorizedEffectV1`] into a typed
//! settlement request.
//!
//! Slice 04 closes the deny/no-tx boundary and wires the allow path
//! to a [`SettlementBackend`](crate::cardano::SettlementBackend)
//! via the [`fulfill`] entrypoint. The typed
//! [`FulfillmentOutcome`] surface is what Cluster 8 evidence work
//! consumes.
//!
//! Does NOT own:
//!
//! - The authorization decision (that is
//!   `oracleguard_policy::authorize::authorize_disbursement`).
//! - Canonical byte identity of the authorized effect (that is
//!   `oracleguard_schemas::effect::AuthorizedEffectV1`).
//! - Transaction-body construction or signing (that is the
//!   `cardano-cli`-backed settlement backend in
//!   `oracleguard_adapter::cardano`, hidden behind a trait so this
//!   module stays pure).
//! - Evidence rendering or verification (that is Cluster 8).
//!
//! ## Layering
//!
//! This module is structurally BLUE/GREEN: every function in its
//! production source is pure (no I/O, no wall-clock reads, no mutable
//! global state).

use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::reason::DisbursementReasonCode;

use crate::cardano::{CardanoTxHashV1, SettlementBackend, SettlementSubmitError};
use crate::intake::{IntentReceiptV1, IntentStatus};

// -------------------------------------------------------------------
// Slice 02 — Exact effect-to-settlement mapping
// -------------------------------------------------------------------

/// Typed settlement request consumed by a settlement backend.
///
/// Every field is copied verbatim from an
/// [`AuthorizedEffectV1`] except `intent_id`, which the caller
/// supplies from the consensus receipt. The struct exists so the
/// settlement backend has a narrow, inspectable input surface — there
/// is no place for "helpful" wallet logic to slip in a different
/// address, amount, or asset.
///
/// The mapping is pinned by
/// [`settlement_request_from_effect`] and exercised in the
/// fulfillment-boundary integration suite
/// (`crates/oracleguard-adapter/tests/fulfillment_boundary.rs`, added
/// in Slice 06).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SettlementRequest {
    /// Destination address. Byte-identical to the authorized effect's
    /// `destination`, including the `length` field.
    pub destination: CardanoAddressV1,
    /// Asset to be moved. Byte-identical to the authorized effect's
    /// `asset`. Slice 03 will add an MVP-only guard requiring this to
    /// equal [`AssetIdV1::ADA`] before the request reaches the
    /// backend.
    pub asset: AssetIdV1,
    /// Amount in lovelace. Byte-identical to the authorized effect's
    /// `authorized_amount_lovelace`. Never rounded, never reduced.
    pub amount_lovelace: u64,
    /// 32-byte BLAKE3 `intent_id` from the consensus receipt. Used by
    /// the backend for idempotency / correlation only; it is NOT a
    /// Cardano transaction field.
    pub intent_id: [u8; 32],
}

// -------------------------------------------------------------------
// Slice 03 — ADA-only MVP fulfillment guard
// -------------------------------------------------------------------

/// Closed set of fulfillment-side rejection kinds.
///
/// These are distinct from
/// [`oracleguard_schemas::reason::DisbursementReasonCode`]: an
/// upstream deny carries a reason and a gate; a fulfillment-side
/// rejection means authorization was allowed but the MVP shell still
/// cannot execute the transaction. Slice 04 extends this enum with
/// `ReceiptNotCommitted` for pending-status receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FulfillmentRejection {
    /// The authorized effect's asset is not ADA. The MVP supports
    /// ADA-only settlement; a non-ADA authorized effect indicates a
    /// registry misconfiguration upstream and is surfaced as a typed
    /// rejection rather than silently dropped.
    NonAdaAsset,
    /// The consensus receipt is not in [`IntentStatus::Committed`].
    /// Settlement requires a committed decision; pending receipts are
    /// never executed, and the caller MUST re-query before acting.
    ReceiptNotCommitted,
}

/// Reject any authorized effect whose asset is not byte-identical to
/// [`AssetIdV1::ADA`].
///
/// The MVP settlement path supports exactly one asset: ADA. This
/// guard is the mechanical enforcement of that scope; any future
/// multi-asset slice must revisit it alongside the registry gate and
/// the evidence verifier.
pub fn guard_mvp_asset(effect: &AuthorizedEffectV1) -> Result<(), FulfillmentRejection> {
    if effect.asset != AssetIdV1::ADA {
        return Err(FulfillmentRejection::NonAdaAsset);
    }
    Ok(())
}

/// Pure mapping from an authorized effect plus a consensus
/// `intent_id` into a [`SettlementRequest`].
///
/// The three authoritative fields — `destination`, `asset`,
/// `amount_lovelace` — are copied verbatim. No bech32 re-encoding, no
/// unit conversion, no rounding. `policy_ref`, `allocation_id`,
/// `requester_id`, and `release_cap_basis_points` are NOT consulted:
/// they are authorization-provenance fields and do not belong on the
/// Cardano transaction.
pub fn settlement_request_from_effect(
    effect: &AuthorizedEffectV1,
    intent_id: [u8; 32],
) -> SettlementRequest {
    SettlementRequest {
        destination: effect.destination,
        asset: effect.asset,
        amount_lovelace: effect.authorized_amount_lovelace,
        intent_id,
    }
}

// -------------------------------------------------------------------
// Slice 04 — Deny / no-transaction closure
// -------------------------------------------------------------------

/// Typed execution-outcome surface consumed by Cluster 8 evidence.
///
/// Every possible outcome of a single [`fulfill`] call maps to
/// exactly one variant. `Settled` is the only variant that
/// corresponds to a submitted Cardano transaction; the other variants
/// represent no-transaction closures with their cause made explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FulfillmentOutcome {
    /// The allow path executed and the settlement backend returned a
    /// captured transaction hash. This is the only outcome that
    /// corresponds to a submitted Cardano transaction.
    Settled {
        /// Captured 32-byte Cardano transaction id.
        tx_hash: CardanoTxHashV1,
    },
    /// Authorization was denied upstream of OracleGuard; no Cardano
    /// transaction is possible. `reason` and `gate` are copied
    /// verbatim from the consensus receipt — OracleGuard does not
    /// translate denial reasons.
    DeniedUpstream {
        /// Canonical denial reason produced by the failing gate.
        reason: DisbursementReasonCode,
        /// The failing gate. Invariant: `gate == reason.gate()`.
        gate: AuthorizationGate,
    },
    /// Authorization allowed the disbursement but the fulfillment
    /// boundary rejected it before the backend was called.
    RejectedAtFulfillment {
        /// Reason the fulfillment boundary refused to submit.
        kind: FulfillmentRejection,
    },
}

/// Execute the Cardano fulfillment boundary for a consensus receipt.
///
/// `fulfill` is the single public entry point that turns an
/// [`IntentReceiptV1`] into a typed [`FulfillmentOutcome`]. Its
/// branch structure is the mechanical no-tx closure (see
/// `docs/fulfillment-boundary.md §4`):
///
/// 1. If the receipt is not committed →
///    [`FulfillmentOutcome::RejectedAtFulfillment`] with
///    [`FulfillmentRejection::ReceiptNotCommitted`]. **Backend is not
///    called.**
/// 2. If the receipt carries
///    [`AuthorizationResult::Denied`] →
///    [`FulfillmentOutcome::DeniedUpstream`] with `reason` and `gate`
///    copied verbatim. **Backend is not called.**
/// 3. If the receipt carries [`AuthorizationResult::Authorized`] but
///    the ADA-only guard rejects →
///    [`FulfillmentOutcome::RejectedAtFulfillment`] with
///    [`FulfillmentRejection::NonAdaAsset`]. **Backend is not
///    called.**
/// 4. Otherwise a [`SettlementRequest`] is built via
///    [`settlement_request_from_effect`] and handed to
///    `backend.submit`. On success →
///    [`FulfillmentOutcome::Settled`] with the captured
///    [`CardanoTxHashV1`]. On backend error → the error is returned
///    verbatim to the caller.
///
/// The backend reference is only reachable from branch 4. A
/// structural audit test in this module pins that the production
/// source does not call any evaluator or authorization-gate
/// function.
pub fn fulfill(
    receipt: &IntentReceiptV1,
    backend: &impl SettlementBackend,
) -> Result<FulfillmentOutcome, SettlementSubmitError> {
    if receipt.status != IntentStatus::Committed {
        return Ok(FulfillmentOutcome::RejectedAtFulfillment {
            kind: FulfillmentRejection::ReceiptNotCommitted,
        });
    }

    match receipt.output {
        AuthorizationResult::Denied { reason, gate } => {
            Ok(FulfillmentOutcome::DeniedUpstream { reason, gate })
        }
        AuthorizationResult::Authorized { effect } => {
            if let Err(kind) = guard_mvp_asset(&effect) {
                return Ok(FulfillmentOutcome::RejectedAtFulfillment { kind });
            }
            let request = settlement_request_from_effect(&effect, receipt.intent_id);
            let tx_hash = backend.submit(&request)?;
            Ok(FulfillmentOutcome::Settled { tx_hash })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};

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

    #[test]
    fn mapping_copies_destination_exactly() {
        let effect = sample_effect();
        let req = settlement_request_from_effect(&effect, [0x00; 32]);
        assert_eq!(req.destination, effect.destination);
        assert_eq!(req.destination.length, effect.destination.length);
        assert_eq!(req.destination.bytes, effect.destination.bytes);
    }

    #[test]
    fn mapping_copies_asset_exactly() {
        let effect = sample_effect();
        let req = settlement_request_from_effect(&effect, [0x00; 32]);
        assert_eq!(req.asset, effect.asset);
        assert_eq!(req.asset.policy_id, effect.asset.policy_id);
        assert_eq!(req.asset.asset_name, effect.asset.asset_name);
        assert_eq!(req.asset.asset_name_len, effect.asset.asset_name_len);
    }

    #[test]
    fn mapping_copies_amount_exactly() {
        let mut effect = sample_effect();
        for amount in [
            0u64,
            1,
            1_000_000,
            700_000_000,
            1_000_000_000,
            u64::MAX / 2,
            u64::MAX,
        ] {
            effect.authorized_amount_lovelace = amount;
            let req = settlement_request_from_effect(&effect, [0x00; 32]);
            assert_eq!(req.amount_lovelace, amount);
        }
    }

    #[test]
    fn mapping_copies_intent_id_exactly() {
        let intent_id = [0x77; 32];
        let req = settlement_request_from_effect(&sample_effect(), intent_id);
        assert_eq!(req.intent_id, intent_id);
    }

    #[test]
    fn mapping_is_deterministic() {
        let effect = sample_effect();
        let a = settlement_request_from_effect(&effect, [0xAA; 32]);
        let b = settlement_request_from_effect(&effect, [0xAA; 32]);
        assert_eq!(a, b);
    }

    #[test]
    fn mapping_is_independent_of_release_cap_basis_points() {
        let mut a = sample_effect();
        let mut b = sample_effect();
        a.release_cap_basis_points = 5_000;
        b.release_cap_basis_points = 10_000;
        let ra = settlement_request_from_effect(&a, [0x00; 32]);
        let rb = settlement_request_from_effect(&b, [0x00; 32]);
        assert_eq!(ra, rb);
    }

    #[test]
    fn mapping_is_independent_of_policy_ref_allocation_requester() {
        let mut a = sample_effect();
        let mut b = sample_effect();
        a.policy_ref = [0x00; 32];
        b.policy_ref = [0xFF; 32];
        a.allocation_id = [0x00; 32];
        b.allocation_id = [0xFF; 32];
        a.requester_id = [0x00; 32];
        b.requester_id = [0xFF; 32];
        let ra = settlement_request_from_effect(&a, [0x00; 32]);
        let rb = settlement_request_from_effect(&b, [0x00; 32]);
        assert_eq!(ra, rb);
    }

    // ---- Slice 03 — ADA-only guard ----

    #[test]
    fn ada_asset_passes_guard() {
        let effect = sample_effect();
        assert_eq!(effect.asset, AssetIdV1::ADA);
        assert_eq!(guard_mvp_asset(&effect), Ok(()));
    }

    #[test]
    fn non_ada_policy_id_is_rejected() {
        let mut effect = sample_effect();
        effect.asset.policy_id = [0xAB; 28];
        assert_eq!(
            guard_mvp_asset(&effect),
            Err(FulfillmentRejection::NonAdaAsset),
        );
    }

    #[test]
    fn non_ada_asset_name_len_is_rejected() {
        let mut effect = sample_effect();
        effect.asset.asset_name_len = 5;
        effect.asset.asset_name[..5].copy_from_slice(b"OTHER");
        assert_eq!(
            guard_mvp_asset(&effect),
            Err(FulfillmentRejection::NonAdaAsset),
        );
    }

    #[test]
    fn non_zero_asset_name_bytes_with_zero_len_are_not_ada() {
        // ADA is *byte-identical* to AssetIdV1::ADA. A non-zero
        // padding byte with asset_name_len == 0 is structurally not
        // ADA and must be rejected.
        let mut effect = sample_effect();
        effect.asset.asset_name[0] = 0x01;
        assert_ne!(effect.asset, AssetIdV1::ADA);
        assert_eq!(
            guard_mvp_asset(&effect),
            Err(FulfillmentRejection::NonAdaAsset),
        );
    }

    #[test]
    fn guard_is_deterministic() {
        let effect = sample_effect();
        for _ in 0..4 {
            assert_eq!(guard_mvp_asset(&effect), Ok(()));
        }
    }

    // ---- Slice 04 — deny / no-tx closure ----

    use std::cell::RefCell;

    use oracleguard_policy::authorize::AuthorizationResult;
    use oracleguard_schemas::gate::AuthorizationGate;
    use oracleguard_schemas::reason::DisbursementReasonCode;

    use crate::cardano::{CardanoTxHashV1, SettlementSubmitError};
    use crate::intake::{IntentReceiptV1, IntentStatus};

    /// Deterministic fixture backend that records every
    /// [`SettlementRequest`] it sees and returns a pinned tx hash.
    ///
    /// This is a seeded deterministic substitute, not a mock of the
    /// functional core — see `CLAUDE.md "No-Mocks Contract"`.
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

    #[test]
    fn every_deny_reason_short_circuits_without_backend_call() {
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
            assert_eq!(
                out,
                FulfillmentOutcome::DeniedUpstream { reason, gate },
                "reason {reason:?} did not produce verbatim DeniedUpstream"
            );
            assert_eq!(
                backend.submit_count(),
                0,
                "backend called on deny path for {reason:?}"
            );
        }
    }

    #[test]
    fn pending_receipt_short_circuits_without_backend_call() {
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

    #[test]
    fn non_ada_authorized_effect_short_circuits_without_backend_call() {
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
    fn authorized_ada_effect_calls_backend_exactly_once() {
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
        let req = backend.last_request().expect("one request");
        let expected = settlement_request_from_effect(&sample_effect(), [0xAA; 32]);
        assert_eq!(req, expected);
    }

    #[test]
    fn fulfill_is_deterministic_for_deny() {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
        let receipt = committed_denied_receipt(
            DisbursementReasonCode::OracleStale,
            AuthorizationGate::Grant,
        );
        let a = fulfill(&receipt, &backend).expect("a");
        let b = fulfill(&receipt, &backend).expect("b");
        assert_eq!(a, b);
        assert_eq!(backend.submit_count(), 0);
    }

    #[test]
    fn fulfill_is_deterministic_for_reject() {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
        let mut effect = sample_effect();
        effect.asset.policy_id = [0xAB; 28];
        let receipt = committed_authorized_receipt(effect);
        let a = fulfill(&receipt, &backend).expect("a");
        let b = fulfill(&receipt, &backend).expect("b");
        assert_eq!(a, b);
        assert_eq!(backend.submit_count(), 0);
    }

    #[test]
    fn fulfill_is_deterministic_for_settled() {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0xEF; 32]));
        let receipt = committed_authorized_receipt(sample_effect());
        let a = fulfill(&receipt, &backend).expect("a");
        let b = fulfill(&receipt, &backend).expect("b");
        assert_eq!(a, b);
        assert_eq!(backend.submit_count(), 2);
    }

    #[test]
    fn settled_request_amount_equals_authorized_amount_exactly() {
        for amount in [1u64, 100, 700_000_000, 1_000_000_000, u64::MAX] {
            let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
            let mut effect = sample_effect();
            effect.authorized_amount_lovelace = amount;
            let receipt = committed_authorized_receipt(effect);
            fulfill(&receipt, &backend).expect("fulfill");
            let req = backend.last_request().expect("one request");
            assert_eq!(req.amount_lovelace, amount);
        }
    }

    #[test]
    fn settled_request_intent_id_equals_receipt_intent_id() {
        let backend = CountingFixtureBackend::new(CardanoTxHashV1([0x00; 32]));
        let mut receipt = committed_authorized_receipt(sample_effect());
        receipt.intent_id = [0x99; 32];
        fulfill(&receipt, &backend).expect("fulfill");
        let req = backend.last_request().expect("one request");
        assert_eq!(req.intent_id, [0x99; 32]);
    }

    // ---- Structural audit: production source must not recompute
    // authorization ----
    //
    // Mirrors `intake::intake_does_not_recompute_authorization`.

    #[test]
    fn settlement_production_source_does_not_call_authorize_or_evaluate() {
        const THIS_FILE: &str = include_str!("settlement.rs");
        let split_marker = format!("{}[cfg(test)]", '#');
        let (prod, _) = THIS_FILE
            .split_once(split_marker.as_str())
            .expect("production/test split marker must exist");
        let call = |s: &str| format!("{s}(");
        for sym in [
            "authorize_disbursement",
            "evaluate_disbursement",
            "run_anchor_gate",
            "run_registry_gate",
            "run_grant_gate",
        ] {
            let pat = call(sym);
            assert!(
                !prod.contains(&pat),
                "settlement production source must not call {sym}"
            );
        }
    }
}
