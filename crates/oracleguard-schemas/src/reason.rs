//! Reason codes.
//!
//! Owns: the closed set of canonical reason codes emitted alongside
//! evaluator decisions and verifier findings.
//!
//! Does NOT own: the decision logic that selects a reason (policy
//! crate), nor the rendering of reasons for human display (shell or
//! verifier report).

use serde::{Deserialize, Serialize};

use crate::oracle::OracleFactEvalV1;

/// Closed set of typed reasons a [`crate::intent::DisbursementIntentV1`]
/// can be denied.
///
/// The variant set is frozen at v1. The evaluator MUST emit exactly
/// one of these for any deny outcome; the verifier MUST accept exactly
/// this set. Adding, removing, or reordering variants is a canonical-
/// byte change and therefore a schema-version boundary crossing — see
/// `docs/canonical-encoding.md` §Version boundary.
///
/// Cluster 3 Slice 04 pins the shape here so the oracle-rejection path
/// can produce a typed reason without forward-referencing the Cluster 4
/// evaluator. The gate assignment column documents which of the three
/// validation gates (anchor / registry / grant) produces each reason at
/// evaluation time.
///
/// | Variant                | Gate      | Trigger                                                                 |
/// |------------------------|-----------|-------------------------------------------------------------------------|
/// | `PolicyNotFound`       | anchor    | `policy_ref` is not registered                                           |
/// | `AllocationNotFound`   | registry  | `allocation_id` does not exist or is not approved                        |
/// | `OracleStale`          | grant     | oracle datum's `expiry_unix` has passed by the time of evaluation        |
/// | `OraclePriceZero`      | grant     | canonical `price_microusd` is zero (economically meaningless)            |
/// | `AmountZero`           | grant     | `requested_amount_lovelace` is zero                                      |
/// | `ReleaseCapExceeded`   | grant     | requested amount exceeds the release cap computed from the oracle price  |
/// | `SubjectNotAuthorized` | registry  | `requester_id` is not authorized for this allocation                     |
/// | `AssetMismatch`        | registry  | intent asset does not match the allocation's approved asset              |
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisbursementReasonCode {
    /// Anchor gate: the referenced policy is not registered.
    PolicyNotFound = 0,
    /// Registry gate: the referenced allocation does not exist or is
    /// not approved.
    AllocationNotFound = 1,
    /// Grant gate: the oracle datum has expired and must not drive a
    /// release-cap computation.
    OracleStale = 2,
    /// Grant gate: the canonical oracle price is zero.
    OraclePriceZero = 3,
    /// Grant gate: the requested withdrawal amount is zero.
    AmountZero = 4,
    /// Grant gate: the requested amount exceeds the release cap.
    ReleaseCapExceeded = 5,
    /// Registry gate: the requester is not authorized for the
    /// referenced allocation.
    SubjectNotAuthorized = 6,
    /// Registry gate: the intent asset does not match the allocation
    /// asset.
    AssetMismatch = 7,
}

/// Validate the structural preconditions of an [`OracleFactEvalV1`]
/// that are checkable without a policy or allocation context.
///
/// Currently pins only the zero-price rejection rule. The freshness
/// check belongs in the adapter shell because it depends on wall-clock
/// state that must not leak into this pure crate; see
/// `oracleguard_adapter::charli3::check_freshness`.
///
/// Returns `Ok(())` if the fact is structurally usable; returns the
/// typed reason otherwise.
pub fn validate_oracle_fact_eval(
    fact: &OracleFactEvalV1,
) -> Result<(), DisbursementReasonCode> {
    if fact.price_microusd == 0 {
        return Err(DisbursementReasonCode::OraclePriceZero);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::oracle::{ASSET_PAIR_ADA_USD, SOURCE_CHARLI3};

    fn fact_with_price(price_microusd: u64) -> OracleFactEvalV1 {
        OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd,
            source: SOURCE_CHARLI3,
        }
    }

    // Pattern-match destructuring guard: if a variant is added,
    // removed, or reordered this test fails to compile — which is
    // the correct signal that the canonical reason-code surface has
    // changed and the version boundary must be considered.
    #[test]
    fn variant_list_is_frozen_at_eight_codes() {
        let codes = [
            DisbursementReasonCode::PolicyNotFound,
            DisbursementReasonCode::AllocationNotFound,
            DisbursementReasonCode::OracleStale,
            DisbursementReasonCode::OraclePriceZero,
            DisbursementReasonCode::AmountZero,
            DisbursementReasonCode::ReleaseCapExceeded,
            DisbursementReasonCode::SubjectNotAuthorized,
            DisbursementReasonCode::AssetMismatch,
        ];
        // This match is exhaustive by construction and will fail to
        // compile if a new variant is added without updating the list.
        for code in codes {
            let _: u8 = match code {
                DisbursementReasonCode::PolicyNotFound => 0,
                DisbursementReasonCode::AllocationNotFound => 1,
                DisbursementReasonCode::OracleStale => 2,
                DisbursementReasonCode::OraclePriceZero => 3,
                DisbursementReasonCode::AmountZero => 4,
                DisbursementReasonCode::ReleaseCapExceeded => 5,
                DisbursementReasonCode::SubjectNotAuthorized => 6,
                DisbursementReasonCode::AssetMismatch => 7,
            };
        }
    }

    #[test]
    fn repr_u8_discriminants_are_stable() {
        // Discriminants are part of the canonical-byte surface because
        // postcard serializes enums by variant index. Pinning them
        // here forces the surface to stay stable across refactors.
        assert_eq!(DisbursementReasonCode::PolicyNotFound as u8, 0);
        assert_eq!(DisbursementReasonCode::AllocationNotFound as u8, 1);
        assert_eq!(DisbursementReasonCode::OracleStale as u8, 2);
        assert_eq!(DisbursementReasonCode::OraclePriceZero as u8, 3);
        assert_eq!(DisbursementReasonCode::AmountZero as u8, 4);
        assert_eq!(DisbursementReasonCode::ReleaseCapExceeded as u8, 5);
        assert_eq!(DisbursementReasonCode::SubjectNotAuthorized as u8, 6);
        assert_eq!(DisbursementReasonCode::AssetMismatch as u8, 7);
    }

    #[test]
    fn reason_code_postcard_round_trips() {
        let original = DisbursementReasonCode::OraclePriceZero;
        let bytes = postcard::to_allocvec(&original).expect("encode");
        let (decoded, rest) =
            postcard::take_from_bytes::<DisbursementReasonCode>(&bytes).expect("decode");
        assert!(rest.is_empty());
        assert_eq!(decoded, original);
    }

    #[test]
    fn validate_oracle_fact_eval_accepts_nonzero_price() {
        assert_eq!(validate_oracle_fact_eval(&fact_with_price(1)), Ok(()));
        assert_eq!(
            validate_oracle_fact_eval(&fact_with_price(258_000)),
            Ok(())
        );
    }

    #[test]
    fn validate_oracle_fact_eval_rejects_zero_price() {
        assert_eq!(
            validate_oracle_fact_eval(&fact_with_price(0)),
            Err(DisbursementReasonCode::OraclePriceZero)
        );
    }

    #[test]
    fn validate_oracle_fact_eval_is_pure_and_stable() {
        let a = validate_oracle_fact_eval(&fact_with_price(0));
        let b = validate_oracle_fact_eval(&fact_with_price(0));
        assert_eq!(a, b);
        let c = validate_oracle_fact_eval(&fact_with_price(1));
        let d = validate_oracle_fact_eval(&fact_with_price(1));
        assert_eq!(c, d);
    }
}
