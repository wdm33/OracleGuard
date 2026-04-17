//! Cardano fulfillment boundary.
//!
//! Owns: the pure field-copy mapping from
//! [`oracleguard_schemas::effect::AuthorizedEffectV1`] into a typed
//! settlement request.
//!
//! Later Cluster 7 slices extend this module with the deny/no-tx
//! closure and
//! [`SettlementBackend`](crate::cardano::CardanoCliSettlementBackend)
//! trait (Slice 04), and the typed fulfillment-outcome surface that
//! Cluster 8 evidence work consumes (Slice 05).
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

use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};

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
}
