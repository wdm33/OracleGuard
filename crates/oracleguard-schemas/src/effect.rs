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
