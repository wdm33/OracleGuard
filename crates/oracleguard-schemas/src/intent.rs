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

/// Returned by [`validate_intent_version`] when an intent carries a
/// schema version the current code does not know how to evaluate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVersionError {
    pub found: u16,
    pub expected: u16,
}

impl DisbursementIntentV1 {
    /// Sanctioned constructor for v1 intents. Hard-codes
    /// `intent_version = INTENT_VERSION_V1` so callers cannot silently
    /// emit an off-version intent.
    ///
    /// ## Oracle-boundary usage
    ///
    /// The `oracle_fact` and `oracle_provenance` parameters map to
    /// disjoint roles and MUST NOT be constructed from each other or
    /// from shared sources that blur the boundary:
    ///
    /// - `oracle_fact: OracleFactEvalV1` — the evaluation-domain fact.
    ///   Participates in authorization and in `intent_id`. Must be
    ///   produced by `oracleguard_adapter::charli3::normalize_aggstate_datum`
    ///   (or an equivalent public normalization path) and MUST use the
    ///   canonical identifiers from
    ///   [`crate::oracle::ASSET_PAIR_ADA_USD`] and
    ///   [`crate::oracle::SOURCE_CHARLI3`].
    ///
    /// - `oracle_provenance: OracleFactProvenanceV1` — the audit-only
    ///   fetch metadata. Does NOT participate in authorization and is
    ///   excluded from `intent_id` by the identity projection in
    ///   [`crate::encoding::intent_id`].
    ///
    /// ```
    /// use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
    /// use oracleguard_schemas::intent::DisbursementIntentV1;
    /// use oracleguard_schemas::oracle::{
    ///     OracleFactEvalV1, OracleFactProvenanceV1,
    ///     ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
    /// };
    ///
    /// // Evaluation-domain fact: canonical identifiers + integer price.
    /// let oracle_fact = OracleFactEvalV1 {
    ///     asset_pair: ASSET_PAIR_ADA_USD,
    ///     price_microusd: 258_000,
    ///     source: SOURCE_CHARLI3,
    /// };
    ///
    /// // Provenance-domain metadata: creation/expiry timestamps and
    /// // the UTxO reference we read the datum from. Audit-only.
    /// let oracle_provenance = OracleFactProvenanceV1 {
    ///     timestamp_unix: 1_776_428_823_000,
    ///     expiry_unix: 1_776_429_423_000,
    ///     aggregator_utxo_ref: [0x44; 32],
    /// };
    ///
    /// let intent = DisbursementIntentV1::new_v1(
    ///     [0x11; 32],
    ///     [0x22; 32],
    ///     [0x33; 32],
    ///     oracle_fact,
    ///     oracle_provenance,
    ///     1_000_000_000,
    ///     CardanoAddressV1 { bytes: [0x55; 57], length: 57 },
    ///     AssetIdV1::ADA,
    /// );
    /// assert_eq!(intent.oracle_fact.price_microusd, 258_000);
    /// ```
    #[allow(clippy::too_many_arguments)]
    pub fn new_v1(
        policy_ref: [u8; 32],
        allocation_id: [u8; 32],
        requester_id: [u8; 32],
        oracle_fact: OracleFactEvalV1,
        oracle_provenance: OracleFactProvenanceV1,
        requested_amount_lovelace: u64,
        destination: CardanoAddressV1,
        asset: AssetIdV1,
    ) -> Self {
        Self {
            intent_version: INTENT_VERSION_V1,
            policy_ref,
            allocation_id,
            requester_id,
            oracle_fact,
            oracle_provenance,
            requested_amount_lovelace,
            destination,
            asset,
        }
    }
}

/// Accept only the currently supported schema version.
///
/// The MVP accepts exactly `intent_version == INTENT_VERSION_V1`.
/// Anything else is an explicit [`UnsupportedVersionError`], not a
/// silent pass-through, so that schema drift cannot slip past the
/// v1 boundary.
pub fn validate_intent_version(
    intent: &DisbursementIntentV1,
) -> Result<(), UnsupportedVersionError> {
    if intent.intent_version == INTENT_VERSION_V1 {
        Ok(())
    } else {
        Err(UnsupportedVersionError {
            found: intent.intent_version,
            expected: INTENT_VERSION_V1,
        })
    }
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

    #[test]
    fn new_v1_constructor_hard_codes_version() {
        let i = DisbursementIntentV1::new_v1(
            [0x11; 32],
            [0x22; 32],
            [0x33; 32],
            OracleFactEvalV1 {
                asset_pair: *b"ADA/USD\0\0\0\0\0\0\0\0\0",
                price_microusd: 450_000,
                source: *b"charli3\0\0\0\0\0\0\0\0\0",
            },
            OracleFactProvenanceV1 {
                timestamp_unix: 1,
                expiry_unix: 2,
                aggregator_utxo_ref: [0x44; 32],
            },
            1_000_000_000,
            CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            AssetIdV1::ADA,
        );
        assert_eq!(i.intent_version, INTENT_VERSION_V1);
    }

    #[test]
    fn validate_intent_version_accepts_v1() {
        assert_eq!(validate_intent_version(&sample_intent()), Ok(()));
    }

    #[test]
    fn validate_intent_version_rejects_unknown() {
        let mut i = sample_intent();
        i.intent_version = 2;
        assert_eq!(
            validate_intent_version(&i),
            Err(UnsupportedVersionError {
                found: 2,
                expected: INTENT_VERSION_V1,
            })
        );
    }

    #[test]
    fn validate_intent_version_rejects_zero() {
        let mut i = sample_intent();
        i.intent_version = 0;
        assert!(validate_intent_version(&i).is_err());
    }
}
