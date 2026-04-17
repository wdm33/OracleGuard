//! Disbursement intent types.
//!
//! Owns: canonical [`DisbursementIntentV1`] — the public, versioned
//! request surface for an OracleGuard-mediated disbursement. Every
//! downstream component (evaluator, adapter submission, verifier)
//! operates on this type.
//!
//! Does NOT own: intent construction from Charli3/Cardano inputs (that
//! is shell work in `oracleguard-adapter`), the decision to authorize a
//! release (that is `oracleguard-policy`), or transport of an intent
//! (that is the adapter's submission shell).

use serde::{Deserialize, Serialize};

use crate::effect::{AssetIdV1, CardanoAddressV1};
use crate::oracle::{OracleFactEvalV1, OracleFactProvenanceV1};

/// Current supported intent schema version.
///
/// See `docs/canonical-encoding.md` for the version-boundary rules.
pub const INTENT_VERSION_V1: u16 = 1;

/// Canonical public disbursement-intent shape for v1.
///
/// Every field is fixed-width so canonical byte identity does not
/// depend on heap layout. The field list is locked for v1; any change
/// requires a new versioned struct and a new canonical encoding path.
///
/// ## Field semantic roles
///
/// - `intent_version` — schema version. Only `1` is accepted under the
///   v1 surface.
/// - `policy_ref` — 32-byte governing-policy identity. See
///   `oracleguard_schemas::policy`.
/// - `allocation_id` — identifier of the pre-approved managed-pool
///   allocation the requester is drawing from.
/// - `requester_id` — stable subject identifier for the requesting
///   party. Registry-gate input.
/// - `oracle_fact` — evaluation-domain oracle observation. Participates
///   in authorization.
/// - `oracle_provenance` — audit-only fetch metadata. **MUST NOT**
///   affect authorization; excluded from intent-id (slice 05).
/// - `requested_amount_lovelace` — requested withdrawal amount in
///   lovelace (`1e6` lovelace = 1 ADA).
/// - `destination` — requester-proposed payout address. Validated by
///   the registry gate at evaluation time, not by this schema.
/// - `asset` — asset being moved. ADA is [`AssetIdV1::ADA`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisbursementIntentV1 {
    pub intent_version: u16,
    pub policy_ref: [u8; 32],
    pub allocation_id: [u8; 32],
    pub requester_id: [u8; 32],
    pub oracle_fact: OracleFactEvalV1,
    pub oracle_provenance: OracleFactProvenanceV1,
    pub requested_amount_lovelace: u64,
    pub destination: CardanoAddressV1,
    pub asset: AssetIdV1,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample_intent() -> DisbursementIntentV1 {
        DisbursementIntentV1 {
            intent_version: INTENT_VERSION_V1,
            policy_ref: [0x11; 32],
            allocation_id: [0x22; 32],
            requester_id: [0x33; 32],
            oracle_fact: OracleFactEvalV1 {
                asset_pair: *b"ADA/USD\0\0\0\0\0\0\0\0\0",
                price_microusd: 450_000,
                source: *b"charli3\0\0\0\0\0\0\0\0\0",
            },
            oracle_provenance: OracleFactProvenanceV1 {
                timestamp_unix: 1_713_000_000_000,
                expiry_unix: 1_713_000_300_000,
                aggregator_utxo_ref: [0x44; 32],
            },
            requested_amount_lovelace: 1_000_000_000,
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
        }
    }

    #[test]
    fn intent_constructor_exposes_all_fixed_width_fields() {
        // This pattern match is a compile-time guard against silent
        // field addition or removal. If the struct changes, the
        // destructuring fails and the slice 03 acceptance criterion is
        // re-opened.
        let DisbursementIntentV1 {
            intent_version,
            policy_ref,
            allocation_id,
            requester_id,
            oracle_fact,
            oracle_provenance,
            requested_amount_lovelace,
            destination,
            asset,
        } = sample_intent();
        assert_eq!(intent_version, INTENT_VERSION_V1);
        assert_eq!(policy_ref, [0x11; 32]);
        assert_eq!(allocation_id, [0x22; 32]);
        assert_eq!(requester_id, [0x33; 32]);
        assert_eq!(oracle_fact.price_microusd, 450_000);
        assert_eq!(oracle_provenance.timestamp_unix, 1_713_000_000_000);
        assert_eq!(requested_amount_lovelace, 1_000_000_000);
        assert_eq!(destination.length, 57);
        assert_eq!(asset, AssetIdV1::ADA);
    }

    #[test]
    fn intent_version_constant_is_one() {
        assert_eq!(INTENT_VERSION_V1, 1);
    }

    #[test]
    fn intent_round_trip_via_copy_preserves_equality() {
        let a = sample_intent();
        let b = a;
        assert_eq!(a, b);
    }
}
