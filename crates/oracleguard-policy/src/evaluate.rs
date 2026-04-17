//! Release-cap evaluator entry point.
//!
//! Owns: the deterministic function that maps canonical semantic inputs
//! (a [`DisbursementIntentV1`] and a caller-supplied allocation basis)
//! to an [`EvaluationResult`]. The evaluator is pure: no I/O, no
//! floating-point arithmetic, no wall-clock reads, no mutable state.
//!
//! Does NOT own: input collection, anchor-gate / registry-gate lookups
//! (Cluster 5), freshness (freshness is checked in the adapter before
//! the evaluator runs), effect execution, or evidence persistence.
//!
//! See `docs/evaluator-contract.md` for the full public contract.

use oracleguard_schemas::intent::{DisbursementIntentV1, INTENT_VERSION_V1};
use oracleguard_schemas::reason::{validate_oracle_fact_eval, DisbursementReasonCode};

use crate::error::EvaluationResult;
use crate::math::{compute_max_releasable_lovelace, decide_grant, select_release_band_bps};

/// Evaluate a disbursement intent against a caller-supplied allocation
/// basis and return a typed [`EvaluationResult`].
///
/// This is the single public entry point for the release-cap decision
/// core. It consumes exactly two deterministic inputs:
///
/// - `intent` — a canonical v1 disbursement intent. The caller is
///   responsible for having normalized the intent through
///   [`oracleguard_schemas::intent::validate_intent_version`] before
///   calling; non-v1 intents are a Cluster 5 anchor-gate concern. In
///   debug builds a non-v1 intent triggers a `debug_assert!`; in
///   release builds the evaluator proceeds on the assumption that the
///   caller has already validated the version.
/// - `allocation_basis_lovelace` — the approved allocation size, in
///   lovelace, that the requester is drawing from. In Cluster 5 the
///   registry gate resolves this from `intent.allocation_id`; in
///   Cluster 4 it is passed directly so the evaluator can be tested in
///   isolation.
///
/// ## Grant-gate sequencing
///
/// The evaluator runs these checks in exactly this order. Each check
/// emits exactly one typed reason; there is no fallthrough and no
/// silent best-effort behavior.
///
/// 1. `intent.requested_amount_lovelace == 0` →
///    [`DisbursementReasonCode::AmountZero`].
/// 2. [`validate_oracle_fact_eval`] rejects the fact →
///    [`DisbursementReasonCode::OraclePriceZero`].
/// 3. Select a release band from
///    `intent.oracle_fact.price_microusd` via
///    [`select_release_band_bps`].
/// 4. Compute `max_releasable` via
///    [`compute_max_releasable_lovelace`]. `None` →
///    [`DisbursementReasonCode::ReleaseCapExceeded`].
/// 5. [`decide_grant`] against `requested_amount_lovelace`:
///    - `None` → `Allow { requested, band_bps }`.
///    - `Some(ReleaseCapExceeded)` →
///      [`DisbursementReasonCode::ReleaseCapExceeded`].
///
/// ## What this function will never do
///
/// - Perform I/O or allocate for I/O.
/// - Read a wall clock.
/// - Produce a non-enumerated reason code.
/// - Return `Allow` with an amount different from
///   `intent.requested_amount_lovelace`.
/// - Consult `intent.oracle_provenance` for any authorization purpose.
#[must_use]
pub fn evaluate_disbursement(
    intent: &DisbursementIntentV1,
    allocation_basis_lovelace: u64,
) -> EvaluationResult {
    // Caller contract: the anchor gate (Cluster 5) validates the
    // schema version before calling. In debug builds we trip on a
    // violation; in release builds we proceed on the assumption.
    debug_assert!(
        intent.intent_version == INTENT_VERSION_V1,
        "evaluate_disbursement called with non-v1 intent; the anchor gate must reject this earlier",
    );

    // 1. AmountZero.
    if intent.requested_amount_lovelace == 0 {
        return EvaluationResult::Deny {
            reason: DisbursementReasonCode::AmountZero,
        };
    }

    // 2. OraclePriceZero (and any other structural oracle rejection
    //    that validate_oracle_fact_eval currently pins — today that
    //    is exactly the zero-price rule).
    if let Err(reason) = validate_oracle_fact_eval(&intent.oracle_fact) {
        return EvaluationResult::Deny { reason };
    }

    // 3. Band selection — total over u64, pure.
    let band_bps = select_release_band_bps(intent.oracle_fact.price_microusd);

    // 4. Cap computation — total multiplication with fallible u64
    //    downcast. For the three valid band constants the downcast
    //    never fails (proven in math::tests); the None branch exists
    //    only as a structural closure.
    let max_releasable = match compute_max_releasable_lovelace(allocation_basis_lovelace, band_bps)
    {
        Some(cap) => cap,
        None => {
            return EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        }
    };

    // 5. Grant decision — emits only None or ReleaseCapExceeded.
    match decide_grant(max_releasable, intent.requested_amount_lovelace) {
        None => EvaluationResult::Allow {
            authorized_amount_lovelace: intent.requested_amount_lovelace,
            release_cap_basis_points: band_bps,
        },
        Some(reason) => EvaluationResult::Deny { reason },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
    use oracleguard_schemas::oracle::{
        OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
    };

    // Demo-anchored intent builder. Price defaults to the 450k μUSD
    // mid-band anchor from Implementation Plan §16; callers override
    // the fields they want to exercise.
    fn demo_intent(requested: u64) -> DisbursementIntentV1 {
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
            requested_amount_lovelace: requested,
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
        }
    }

    const DEMO_ALLOCATION: u64 = 1_000_000_000;

    // ---- Demo allow and deny (Implementation Plan §16) ----

    #[test]
    fn demo_allow_700_ada_at_mid_band() {
        // 1_000 ADA allocation, 0.45 USD mid band → 750 ADA cap.
        // 700 ADA request is within the cap.
        let r = evaluate_disbursement(&demo_intent(700_000_000), DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Allow {
                authorized_amount_lovelace: 700_000_000,
                release_cap_basis_points: 7_500,
            }
        );
    }

    #[test]
    fn demo_deny_900_ada_at_mid_band() {
        let r = evaluate_disbursement(&demo_intent(900_000_000), DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    // ---- AmountZero ----

    #[test]
    fn zero_amount_denies_with_amount_zero() {
        let r = evaluate_disbursement(&demo_intent(0), DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::AmountZero,
            }
        );
    }

    #[test]
    fn zero_amount_wins_over_zero_price_ordering() {
        // When both AmountZero and OraclePriceZero are triggerable,
        // the evaluator must emit AmountZero (checked first). This
        // pins the ordering against silent reshuffles.
        let mut i = demo_intent(0);
        i.oracle_fact.price_microusd = 0;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::AmountZero,
            }
        );
    }

    // ---- OraclePriceZero ----

    #[test]
    fn zero_price_denies_with_oracle_price_zero() {
        let mut i = demo_intent(700_000_000);
        i.oracle_fact.price_microusd = 0;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::OraclePriceZero,
            }
        );
    }

    // ---- Band-specific allow and deny ----

    #[test]
    fn high_band_allows_100_percent_of_allocation() {
        // price >= 500_000 μUSD → 100% band → 1_000 ADA cap.
        let mut i = demo_intent(DEMO_ALLOCATION);
        i.oracle_fact.price_microusd = 500_000;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Allow {
                authorized_amount_lovelace: DEMO_ALLOCATION,
                release_cap_basis_points: 10_000,
            }
        );
    }

    #[test]
    fn high_band_denies_above_100_percent() {
        let mut i = demo_intent(DEMO_ALLOCATION + 1);
        i.oracle_fact.price_microusd = 500_000;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    #[test]
    fn low_band_allows_under_50_percent() {
        // price < 350_000 μUSD → 50% band → 500 ADA cap.
        let mut i = demo_intent(400_000_000);
        i.oracle_fact.price_microusd = 100_000;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Allow {
                authorized_amount_lovelace: 400_000_000,
                release_cap_basis_points: 5_000,
            }
        );
    }

    #[test]
    fn low_band_denies_above_50_percent() {
        let mut i = demo_intent(600_000_000);
        i.oracle_fact.price_microusd = 100_000;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    // ---- Boundaries ----

    #[test]
    fn request_equal_to_cap_allows() {
        // Mid-band cap on a 1_000 ADA allocation is 750 ADA. Equality
        // at the cap allows.
        let r = evaluate_disbursement(&demo_intent(750_000_000), DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Allow {
                authorized_amount_lovelace: 750_000_000,
                release_cap_basis_points: 7_500,
            }
        );
    }

    #[test]
    fn request_one_above_cap_denies() {
        let r = evaluate_disbursement(&demo_intent(750_000_001), DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    #[test]
    fn price_at_mid_threshold_uses_mid_band() {
        let mut i = demo_intent(750_000_000);
        i.oracle_fact.price_microusd = 350_000;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Allow {
                authorized_amount_lovelace: 750_000_000,
                release_cap_basis_points: 7_500,
            }
        );
    }

    #[test]
    fn price_one_below_mid_threshold_uses_low_band() {
        // 349_999 μUSD falls into the low band → 500 ADA cap. 750 ADA
        // request must deny.
        let mut i = demo_intent(750_000_000);
        i.oracle_fact.price_microusd = 349_999;
        let r = evaluate_disbursement(&i, DEMO_ALLOCATION);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    // ---- Allocation edges ----

    #[test]
    fn zero_allocation_denies_any_nonzero_request() {
        // Cap is 0 for every band when allocation is 0. Any nonzero
        // request denies with ReleaseCapExceeded.
        let r = evaluate_disbursement(&demo_intent(1), 0);
        assert_eq!(
            r,
            EvaluationResult::Deny {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
            }
        );
    }

    // ---- Determinism ----

    #[test]
    fn evaluate_is_deterministic_for_identical_inputs() {
        let intent = demo_intent(700_000_000);
        let a = evaluate_disbursement(&intent, DEMO_ALLOCATION);
        let b = evaluate_disbursement(&intent, DEMO_ALLOCATION);
        assert_eq!(a, b);
    }

    #[test]
    fn evaluate_is_deterministic_across_representative_cases() {
        let cases: &[(u64, u64, u64)] = &[
            (0, 450_000, DEMO_ALLOCATION),
            (700_000_000, 450_000, DEMO_ALLOCATION),
            (750_000_000, 350_000, DEMO_ALLOCATION),
            (900_000_000, 450_000, DEMO_ALLOCATION),
            (DEMO_ALLOCATION, 500_000, DEMO_ALLOCATION),
            (400_000_000, 100_000, DEMO_ALLOCATION),
        ];
        for (req, price, alloc) in cases {
            let mut i = demo_intent(*req);
            i.oracle_fact.price_microusd = *price;
            let a = evaluate_disbursement(&i, *alloc);
            let b = evaluate_disbursement(&i, *alloc);
            assert_eq!(a, b, "evaluator varied for ({req}, {price}, {alloc})");
        }
    }

    // ---- Provenance non-interference (evaluator layer) ----

    #[test]
    fn provenance_timestamp_does_not_affect_decision() {
        let base = evaluate_disbursement(&demo_intent(700_000_000), DEMO_ALLOCATION);
        let mut mutated = demo_intent(700_000_000);
        mutated.oracle_provenance.timestamp_unix = u64::MAX;
        let r = evaluate_disbursement(&mutated, DEMO_ALLOCATION);
        assert_eq!(base, r);
    }

    #[test]
    fn provenance_expiry_does_not_affect_decision() {
        let base = evaluate_disbursement(&demo_intent(700_000_000), DEMO_ALLOCATION);
        let mut mutated = demo_intent(700_000_000);
        mutated.oracle_provenance.expiry_unix = 0;
        let r = evaluate_disbursement(&mutated, DEMO_ALLOCATION);
        assert_eq!(base, r);
    }

    #[test]
    fn provenance_utxo_ref_does_not_affect_decision() {
        let base = evaluate_disbursement(&demo_intent(700_000_000), DEMO_ALLOCATION);
        let mut mutated = demo_intent(700_000_000);
        mutated.oracle_provenance.aggregator_utxo_ref = [0xFF; 32];
        let r = evaluate_disbursement(&mutated, DEMO_ALLOCATION);
        assert_eq!(base, r);
    }

    #[test]
    fn provenance_mutation_also_preserves_deny_outcomes() {
        // Non-interference must hold on the deny side too. A 900 ADA
        // request against a mid-band cap denies; mutating provenance
        // must not flip that outcome.
        let base = evaluate_disbursement(&demo_intent(900_000_000), DEMO_ALLOCATION);
        let mut mutated = demo_intent(900_000_000);
        mutated.oracle_provenance.timestamp_unix = 0;
        mutated.oracle_provenance.expiry_unix = 1;
        mutated.oracle_provenance.aggregator_utxo_ref = [0xCC; 32];
        let r = evaluate_disbursement(&mutated, DEMO_ALLOCATION);
        assert_eq!(base, r);
    }
}
