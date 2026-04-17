//! Authorized effect types and their supporting addresses / asset ids.
//!
//! Cluster 2 locks the minimal shapes needed to close the intent
//! schema: a fixed-width [`CardanoAddressV1`] for the disbursement
//! destination and a fixed-width [`AssetIdV1`] for the asset being
//! moved. Cluster 7 extends this module with the authorized-effect
//! types produced by the evaluator and consumed by the Cardano
//! settlement shell.
//!
//! Does NOT own: execution of effects (adapter), release-cap decision
//! logic (policy), or alternate interpretations of these identifiers.

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

/// Canonical fixed-width Cardano address.
///
/// Shelley-era Cardano addresses are up to 57 bytes when decoded from
/// their bech32 form on mainnet or testnet (28-byte payment credential
/// plus 28-byte stake credential plus a 1-byte header). Fixing the
/// width at 57 lets `CardanoAddressV1` participate in canonical byte
/// identity without heap layout creeping in.
///
/// Shorter addresses (enterprise or reward-only) are zero-padded on the
/// right up to 57 bytes with an explicit `length`, so the adapter can
/// re-encode them to bech32 without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CardanoAddressV1 {
    #[serde(with = "BigArray")]
    pub bytes: [u8; 57],
    pub length: u8,
}

/// Canonical fixed-width Cardano asset identifier.
///
/// A Cardano asset is identified by a 28-byte minting policy id and an
/// asset name of up to 32 bytes. ADA is represented as the all-zero
/// policy id and the empty asset name (`asset_name_len = 0`).
///
/// The `asset_name` field is zero-padded to 32 bytes and `asset_name_len`
/// records the semantic length, so byte identity is stable and the
/// adapter can faithfully reconstruct the on-chain identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AssetIdV1 {
    pub policy_id: [u8; 28],
    pub asset_name: [u8; 32],
    pub asset_name_len: u8,
}

impl AssetIdV1 {
    /// The canonical ADA asset id: zero policy, empty asset name.
    pub const ADA: AssetIdV1 = AssetIdV1 {
        policy_id: [0u8; 28],
        asset_name: [0u8; 32],
        asset_name_len: 0,
    };
}

/// Canonical v1 authorized-effect shape.
///
/// Produced exclusively by a successful three-gate authorization
/// (see `oracleguard_policy::authorize::authorize_disbursement`). This
/// struct is the single source of truth for what settlement may
/// execute: every field is copied verbatim from the already-authorized
/// intent (`policy_ref`, `allocation_id`, `requester_id`, `destination`,
/// `asset`) or derived from the evaluator's typed decision
/// (`authorized_amount_lovelace`, `release_cap_basis_points`).
///
/// The Cardano settlement shell MUST NOT emit any transaction whose
/// fields diverge from these bytes. The offline verifier MUST recheck
/// every field against the intent and the evaluator before accepting
/// evidence.
///
/// All fields are fixed-width so canonical byte identity does not
/// depend on heap layout. The field list is locked for v1; any change
/// is a canonical-byte version-boundary crossing.
///
/// # Cluster 7 Slice 01 — fulfillment-input surface
///
/// `AuthorizedEffectV1` is the **sole typed input** to the Cardano
/// fulfillment boundary owned by `oracleguard-adapter::settlement`.
/// Settlement copies three fields verbatim into the Cardano
/// transaction:
///
/// | Effect field                 | Settlement parameter |
/// |------------------------------|----------------------|
/// | `destination`                | Cardano output address |
/// | `asset`                      | Cardano output asset (MVP: must equal [`AssetIdV1::ADA`]) |
/// | `authorized_amount_lovelace` | Cardano output amount (lovelace) |
///
/// `policy_ref`, `allocation_id`, `requester_id`, and
/// `release_cap_basis_points` are authorization-provenance fields; they
/// MUST NOT influence the Cardano output. The fulfillment boundary's
/// exact-copy rule is pinned by
/// `oracleguard_adapter::settlement::settlement_request_from_effect`
/// and exercised in the `fulfillment_boundary` integration suite.
///
/// ## Hard prohibitions downstream of this type
///
/// - No adapter or verifier crate may define an alternative
///   execution-command surface. This struct is the only typed
///   authorized-effect input.
/// - No fulfillment path may re-run authorization, re-derive
///   `release_cap_basis_points`, reinterpret `destination`, or
///   substitute an asset.
/// - No silent normalization — including "just to canonicalize the
///   address" — is permitted between this struct and the emitted
///   transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AuthorizedEffectV1 {
    /// 32-byte governing-policy identity, byte-identical to the
    /// authorized intent's `policy_ref`.
    pub policy_ref: [u8; 32],
    /// 32-byte allocation identifier, byte-identical to the authorized
    /// intent's `allocation_id`.
    pub allocation_id: [u8; 32],
    /// 32-byte stable subject identifier, byte-identical to the
    /// authorized intent's `requester_id`.
    pub requester_id: [u8; 32],
    /// Registry-approved destination address. Byte-identical to the
    /// authorized intent's `destination`. The registry gate guarantees
    /// membership in the allocation's approved-destination set before
    /// this field is populated.
    pub destination: CardanoAddressV1,
    /// Asset being moved. Byte-identical to the authorized intent's
    /// `asset`. The registry gate guarantees the asset matches the
    /// allocation's approved asset.
    pub asset: AssetIdV1,
    /// Exact amount (in lovelace) that the grant gate authorized.
    /// Always equals the authorized intent's
    /// `requested_amount_lovelace`; the evaluator never silently
    /// reduces the request.
    pub authorized_amount_lovelace: u64,
    /// Basis-point band that authorized the request. One of
    /// `oracleguard_policy::math::BAND_LOW_BPS`,
    /// `BAND_MID_BPS`, `BAND_HIGH_BPS`.
    pub release_cap_basis_points: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

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
    fn authorized_effect_exposes_exact_fixed_width_fields() {
        // Destructuring guard: adding or removing a field must fail
        // this test to compile, forcing a canonical-byte version-
        // boundary conversation.
        let AuthorizedEffectV1 {
            policy_ref,
            allocation_id,
            requester_id,
            destination,
            asset,
            authorized_amount_lovelace,
            release_cap_basis_points,
        } = sample_effect();
        assert_eq!(policy_ref, [0x11; 32]);
        assert_eq!(allocation_id, [0x22; 32]);
        assert_eq!(requester_id, [0x33; 32]);
        assert_eq!(destination.length, 57);
        assert_eq!(asset, AssetIdV1::ADA);
        assert_eq!(authorized_amount_lovelace, 700_000_000);
        assert_eq!(release_cap_basis_points, 7_500);
    }

    #[test]
    fn authorized_effect_postcard_round_trips() {
        let original = sample_effect();
        let bytes = postcard::to_allocvec(&original).expect("encode");
        let (decoded, rest) =
            postcard::take_from_bytes::<AuthorizedEffectV1>(&bytes).expect("decode");
        assert!(rest.is_empty());
        assert_eq!(decoded, original);
    }

    #[test]
    fn authorized_effect_equality_is_field_wise() {
        let a = sample_effect();
        let b = sample_effect();
        assert_eq!(a, b);
        let mut c = sample_effect();
        c.authorized_amount_lovelace = 700_000_001;
        assert_ne!(a, c);
    }

    #[test]
    fn authorized_effect_is_copy_and_equates_by_value() {
        let a = sample_effect();
        let b = a;
        assert_eq!(a, b);
    }

    /// Cluster 7 Slice 01 — the schemas crate must independently pin
    /// the demo effect's canonical bytes against the shared fixture so
    /// the fixture↔struct identity is asserted at the authority crate,
    /// not only from the policy crate.
    #[test]
    fn authorized_effect_golden_fixture_decodes_to_canonical_demo_effect() {
        const GOLDEN: &[u8] =
            include_bytes!("../../../fixtures/authorize/authorized_effect_golden.postcard");
        let (decoded, rest) =
            postcard::take_from_bytes::<AuthorizedEffectV1>(GOLDEN).expect("decode golden");
        assert!(rest.is_empty(), "golden fixture must have no trailing bytes");
        assert_eq!(decoded, sample_effect());
    }
}
