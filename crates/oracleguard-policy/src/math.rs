//! Deterministic integer math used by the evaluator.
//!
//! Owns: pure, total, integer-domain helpers used by the release-cap
//! evaluator. Every function here must be total over its input type
//! and must not panic on any value. Floating-point arithmetic is
//! forbidden at the crate level (`#![deny(clippy::float_arithmetic)]`).
//!
//! Does NOT own: the evaluator's decision flow (see
//! [`crate::evaluate`]), canonical types (those live in
//! `oracleguard-schemas`), or any I/O.

use oracleguard_schemas::reason::DisbursementReasonCode;

/// Upper threshold for the release-band mapping, in micro-USD.
///
/// At or above this price the evaluator selects [`BAND_HIGH_BPS`].
/// Comparison is inclusive on the lower bound, so
/// `price_microusd == THRESHOLD_HIGH_MICROUSD` selects the high band.
///
/// Pinned by Implementation Plan §9 and mirrored in
/// `docs/evaluator-contract.md` §6.
pub const THRESHOLD_HIGH_MICROUSD: u64 = 500_000;

/// Lower threshold for the release-band mapping, in micro-USD.
///
/// At or above this price (and below [`THRESHOLD_HIGH_MICROUSD`]) the
/// evaluator selects [`BAND_MID_BPS`]. Below this price the evaluator
/// selects [`BAND_LOW_BPS`]. Inclusive on the lower bound.
pub const THRESHOLD_MID_MICROUSD: u64 = 350_000;

/// 100% release band, in basis points.
pub const BAND_HIGH_BPS: u64 = 10_000;

/// 75% release band, in basis points.
pub const BAND_MID_BPS: u64 = 7_500;

/// 50% release band, in basis points.
pub const BAND_LOW_BPS: u64 = 5_000;

/// Basis-point denominator (`10_000 bps == 100%`).
///
/// Used by [`crate::math::compute_max_releasable_lovelace`] in Slice 03
/// as the fixed divisor for the basis-point multiplication.
pub const BASIS_POINT_SCALE: u64 = 10_000;

// Compile-time ordering guards. If the thresholds or band constants are
// ever reordered by accident, these `const` assertions fail to compile
// and the mistake never reaches tests or runtime.
const _: () = assert!(THRESHOLD_HIGH_MICROUSD > THRESHOLD_MID_MICROUSD);
const _: () = assert!(BAND_HIGH_BPS > BAND_MID_BPS);
const _: () = assert!(BAND_MID_BPS > BAND_LOW_BPS);
const _: () = assert!(BAND_HIGH_BPS == BASIS_POINT_SCALE);

/// Map an oracle price in micro-USD to the release-cap band in basis
/// points.
///
/// The function is total over `u64`: every price selects exactly one
/// band. Comparisons are inclusive on the lower bound, matching the
/// prose in Implementation Plan §9 and the contract doc.
///
/// `validate_oracle_fact_eval` in `oracleguard-schemas::reason` rejects
/// `price_microusd == 0` before the evaluator runs, so the zero-price
/// codepath of the low band is unreachable in practice. It remains
/// defined here so replay is well-defined across the whole `u64` domain.
///
/// ```
/// use oracleguard_policy::math::{
///     select_release_band_bps, BAND_HIGH_BPS, BAND_LOW_BPS, BAND_MID_BPS,
///     THRESHOLD_HIGH_MICROUSD, THRESHOLD_MID_MICROUSD,
/// };
/// assert_eq!(select_release_band_bps(THRESHOLD_HIGH_MICROUSD), BAND_HIGH_BPS);
/// assert_eq!(select_release_band_bps(THRESHOLD_MID_MICROUSD), BAND_MID_BPS);
/// assert_eq!(select_release_band_bps(THRESHOLD_MID_MICROUSD - 1), BAND_LOW_BPS);
/// ```
#[must_use]
pub const fn select_release_band_bps(price_microusd: u64) -> u64 {
    if price_microusd >= THRESHOLD_HIGH_MICROUSD {
        BAND_HIGH_BPS
    } else if price_microusd >= THRESHOLD_MID_MICROUSD {
        BAND_MID_BPS
    } else {
        BAND_LOW_BPS
    }
}

/// Compute the maximum releasable amount in lovelace for an allocation
/// basis and a basis-point band.
///
/// The formula is `(allocation_lovelace * release_cap_bps) / 10_000`,
/// computed with a `u128` intermediate so the multiplication cannot
/// overflow for any pair of `u64` inputs. Floor division is the only
/// sanctioned rounding rule — fractional lovelace never round up.
///
/// Returns `Some(cap)` when the final value fits in `u64`, and `None`
/// when the post-division value exceeds `u64::MAX`. A `None` return is
/// deterministic: the evaluator maps it to
/// [`oracleguard_schemas::reason::DisbursementReasonCode::ReleaseCapExceeded`]
/// in Slice 04/05. For valid bands (`≤ BASIS_POINT_SCALE`), `None` is
/// unreachable — proven by explicit tests below and in Slice 04.
///
/// ```
/// use oracleguard_policy::math::{
///     compute_max_releasable_lovelace, BAND_HIGH_BPS, BAND_LOW_BPS, BAND_MID_BPS,
/// };
/// assert_eq!(
///     compute_max_releasable_lovelace(1_000_000_000, BAND_MID_BPS),
///     Some(750_000_000),
/// );
/// assert_eq!(
///     compute_max_releasable_lovelace(1_000_000_000, BAND_HIGH_BPS),
///     Some(1_000_000_000),
/// );
/// assert_eq!(
///     compute_max_releasable_lovelace(1_000_000_000, BAND_LOW_BPS),
///     Some(500_000_000),
/// );
/// ```
#[must_use]
pub const fn compute_max_releasable_lovelace(
    allocation_lovelace: u64,
    release_cap_bps: u64,
) -> Option<u64> {
    // `u64 * u64` always fits in `u128`, so this multiplication is
    // total. The Option comes from the post-division downcast only.
    let product = (allocation_lovelace as u128) * (release_cap_bps as u128);
    let cap_u128 = product / (BASIS_POINT_SCALE as u128);
    if cap_u128 > u64::MAX as u128 {
        None
    } else {
        Some(cap_u128 as u64)
    }
}

/// Compare a requested amount to a precomputed release cap and return
/// the grant-gate outcome.
///
/// - `None` means the request is within the cap and must be allowed.
/// - `Some(DisbursementReasonCode::ReleaseCapExceeded)` means the
///   request exceeds the cap and must be denied with exactly that
///   canonical reason.
///
/// Equality at the cap (`requested == max_releasable_lovelace`) is
/// treated as within-cap. This is the sole sanctioned rounding rule
/// for the boundary and it is pinned in `docs/evaluator-contract.md §5`.
///
/// The function is total and pure: no panics, no I/O, no allocation.
/// It does not re-check `requested == 0` — that is an `AmountZero`
/// concern, handled upstream in `evaluate_disbursement` before this
/// function is called. Keeping the two checks separate means each
/// step emits exactly one typed reason.
#[must_use]
pub const fn decide_grant(
    max_releasable_lovelace: u64,
    requested_amount_lovelace: u64,
) -> Option<DisbursementReasonCode> {
    if requested_amount_lovelace > max_releasable_lovelace {
        Some(DisbursementReasonCode::ReleaseCapExceeded)
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn band_constants_match_policy_table() {
        assert_eq!(THRESHOLD_HIGH_MICROUSD, 500_000);
        assert_eq!(THRESHOLD_MID_MICROUSD, 350_000);
        assert_eq!(BAND_HIGH_BPS, 10_000);
        assert_eq!(BAND_MID_BPS, 7_500);
        assert_eq!(BAND_LOW_BPS, 5_000);
        assert_eq!(BASIS_POINT_SCALE, 10_000);
    }

    // Ordering between thresholds and bands is enforced at compile
    // time by `const _: () = assert!(...)` in the module scope, not
    // here — a runtime `assert!` on two `const`s would be optimized
    // out and clippy rejects it.

    // ---- High-band boundary ----

    #[test]
    fn price_at_high_threshold_selects_high_band() {
        assert_eq!(select_release_band_bps(500_000), BAND_HIGH_BPS);
    }

    #[test]
    fn price_just_above_high_threshold_selects_high_band() {
        assert_eq!(select_release_band_bps(500_001), BAND_HIGH_BPS);
    }

    #[test]
    fn price_just_below_high_threshold_selects_mid_band() {
        assert_eq!(select_release_band_bps(499_999), BAND_MID_BPS);
    }

    // ---- Mid-band boundary ----

    #[test]
    fn price_at_mid_threshold_selects_mid_band() {
        assert_eq!(select_release_band_bps(350_000), BAND_MID_BPS);
    }

    #[test]
    fn price_just_above_mid_threshold_selects_mid_band() {
        assert_eq!(select_release_band_bps(350_001), BAND_MID_BPS);
    }

    #[test]
    fn price_just_below_mid_threshold_selects_low_band() {
        assert_eq!(select_release_band_bps(349_999), BAND_LOW_BPS);
    }

    // ---- Domain extremes ----

    #[test]
    fn zero_price_selects_low_band() {
        // The evaluator rejects zero-price intents before calling this
        // helper; the behavior is defined only to keep the function
        // total across the u64 domain.
        assert_eq!(select_release_band_bps(0), BAND_LOW_BPS);
    }

    #[test]
    fn one_microusd_selects_low_band() {
        assert_eq!(select_release_band_bps(1), BAND_LOW_BPS);
    }

    #[test]
    fn u64_max_price_selects_high_band() {
        assert_eq!(select_release_band_bps(u64::MAX), BAND_HIGH_BPS);
    }

    // ---- Demo anchor ----

    #[test]
    fn demo_price_450k_selects_mid_band() {
        // Implementation Plan §16 demo price: $0.45 → 75% band.
        assert_eq!(select_release_band_bps(450_000), BAND_MID_BPS);
    }

    #[test]
    fn charli3_golden_258k_selects_low_band() {
        // Live Preprod AggState golden (Cluster 3) captured at
        // 258_000 micro-USD — below the 350_000 mid threshold.
        assert_eq!(select_release_band_bps(258_000), BAND_LOW_BPS);
    }

    // ---- Determinism ----

    #[test]
    fn band_selection_is_deterministic() {
        for price in [0u64, 1, 349_999, 350_000, 499_999, 500_000, u64::MAX] {
            let a = select_release_band_bps(price);
            let b = select_release_band_bps(price);
            assert_eq!(a, b, "price {price} varied across calls");
        }
    }

    // ---- compute_max_releasable_lovelace: demo anchors ----

    #[test]
    fn demo_allocation_mid_band_yields_750_ada() {
        // Implementation Plan §16: 1_000 ADA × 75% = 750 ADA.
        assert_eq!(
            compute_max_releasable_lovelace(1_000_000_000, BAND_MID_BPS),
            Some(750_000_000),
        );
    }

    #[test]
    fn demo_allocation_high_band_yields_1000_ada() {
        assert_eq!(
            compute_max_releasable_lovelace(1_000_000_000, BAND_HIGH_BPS),
            Some(1_000_000_000),
        );
    }

    #[test]
    fn demo_allocation_low_band_yields_500_ada() {
        assert_eq!(
            compute_max_releasable_lovelace(1_000_000_000, BAND_LOW_BPS),
            Some(500_000_000),
        );
    }

    // ---- compute_max_releasable_lovelace: zero inputs ----

    #[test]
    fn zero_allocation_yields_zero_cap_at_every_band() {
        assert_eq!(compute_max_releasable_lovelace(0, BAND_LOW_BPS), Some(0));
        assert_eq!(compute_max_releasable_lovelace(0, BAND_MID_BPS), Some(0));
        assert_eq!(compute_max_releasable_lovelace(0, BAND_HIGH_BPS), Some(0));
    }

    #[test]
    fn zero_bps_yields_zero_cap_for_any_allocation() {
        assert_eq!(compute_max_releasable_lovelace(0, 0), Some(0));
        assert_eq!(compute_max_releasable_lovelace(1, 0), Some(0));
        assert_eq!(compute_max_releasable_lovelace(u64::MAX, 0), Some(0));
    }

    // ---- compute_max_releasable_lovelace: floor rounding ----

    #[test]
    fn division_rounds_toward_zero_when_product_is_not_divisible() {
        // 7 * 7_500 = 52_500 / 10_000 = 5 (with remainder 2_500). Floor
        // truncation is the only sanctioned rounding rule.
        assert_eq!(compute_max_releasable_lovelace(7, BAND_MID_BPS), Some(5));
    }

    #[test]
    fn division_rounds_down_not_up_on_half() {
        // 1 * 5_000 = 5_000 / 10_000 = 0 (remainder 5_000). No banker's
        // rounding, no half-up.
        assert_eq!(compute_max_releasable_lovelace(1, BAND_LOW_BPS), Some(0));
    }

    // ---- compute_max_releasable_lovelace: large-value round-trip ----

    #[test]
    fn u64_max_allocation_high_band_yields_u64_max() {
        // release_cap_bps == BASIS_POINT_SCALE means "pass allocation
        // through unchanged". Must round-trip u64::MAX without
        // truncation or None.
        assert_eq!(
            compute_max_releasable_lovelace(u64::MAX, BAND_HIGH_BPS),
            Some(u64::MAX),
        );
    }

    #[test]
    fn u64_max_allocation_at_every_valid_band_is_some() {
        // Structural guarantee: for every band the evaluator will
        // select, the downcast is total at u64::MAX allocation.
        assert!(compute_max_releasable_lovelace(u64::MAX, BAND_LOW_BPS).is_some());
        assert!(compute_max_releasable_lovelace(u64::MAX, BAND_MID_BPS).is_some());
        assert!(compute_max_releasable_lovelace(u64::MAX, BAND_HIGH_BPS).is_some());
    }

    // ---- compute_max_releasable_lovelace: determinism ----

    #[test]
    fn cap_computation_is_deterministic() {
        for (alloc, bps) in [
            (0u64, 0u64),
            (1, BAND_LOW_BPS),
            (1_000_000_000, BAND_MID_BPS),
            (1_000_000_000, BAND_HIGH_BPS),
            (u64::MAX, BAND_HIGH_BPS),
        ] {
            let a = compute_max_releasable_lovelace(alloc, bps);
            let b = compute_max_releasable_lovelace(alloc, bps);
            assert_eq!(a, b, "cap varied for ({alloc}, {bps})");
        }
    }

    // ---- compute_max_releasable_lovelace: overflow closure ----

    #[test]
    fn oversized_bps_triggers_none_downcast() {
        // Non-band inputs can exceed BASIS_POINT_SCALE. Under those
        // conditions the post-division value may exceed u64::MAX; the
        // function must return None rather than wrap or panic.
        //
        // u64::MAX * (BASIS_POINT_SCALE + 1) / BASIS_POINT_SCALE > u64::MAX
        // by construction.
        assert_eq!(
            compute_max_releasable_lovelace(u64::MAX, BASIS_POINT_SCALE + 1),
            None,
        );
    }

    #[test]
    fn maximum_oversized_bps_is_still_total() {
        // u64::MAX * u64::MAX fits in u128 (the largest value is
        // 2^128 - 2^65 + 1, which is < 2^128). The function must not
        // panic and must return None.
        assert_eq!(compute_max_releasable_lovelace(u64::MAX, u64::MAX), None,);
    }

    #[test]
    fn oversized_bps_with_tiny_allocation_still_fits() {
        // With a tiny allocation the product fits in u64 even when
        // release_cap_bps is oversized. The function must return Some.
        //
        // 1 * (BASIS_POINT_SCALE + 1) / BASIS_POINT_SCALE = 1 (floor).
        assert_eq!(
            compute_max_releasable_lovelace(1, BASIS_POINT_SCALE + 1),
            Some(1),
        );
    }

    #[test]
    fn overflow_closure_is_deterministic() {
        let a = compute_max_releasable_lovelace(u64::MAX, u64::MAX);
        let b = compute_max_releasable_lovelace(u64::MAX, u64::MAX);
        assert_eq!(a, b);
    }

    // ---- decide_grant ----

    #[test]
    fn decide_grant_allows_when_request_below_cap() {
        assert_eq!(decide_grant(750_000_000, 700_000_000), None);
    }

    #[test]
    fn decide_grant_allows_when_request_equals_cap() {
        // Boundary: equality at the cap is an allow. Pinned by the
        // contract doc §5.
        assert_eq!(decide_grant(750_000_000, 750_000_000), None);
    }

    #[test]
    fn decide_grant_denies_when_request_above_cap() {
        assert_eq!(
            decide_grant(750_000_000, 900_000_000),
            Some(DisbursementReasonCode::ReleaseCapExceeded),
        );
    }

    #[test]
    fn decide_grant_allows_zero_request_against_zero_cap() {
        // decide_grant does not re-check AmountZero; the evaluator
        // handles that upstream. A zero request against a zero cap is
        // "within cap" by comparison, so decide_grant returns None.
        assert_eq!(decide_grant(0, 0), None);
    }

    #[test]
    fn decide_grant_denies_nonzero_request_against_zero_cap() {
        assert_eq!(
            decide_grant(0, 1),
            Some(DisbursementReasonCode::ReleaseCapExceeded),
        );
    }

    #[test]
    fn decide_grant_is_total_at_u64_max() {
        assert_eq!(decide_grant(u64::MAX, u64::MAX), None);
        assert_eq!(
            decide_grant(0, u64::MAX),
            Some(DisbursementReasonCode::ReleaseCapExceeded)
        );
        assert_eq!(decide_grant(u64::MAX, 0), None);
    }

    #[test]
    fn decide_grant_is_deterministic() {
        for (cap, req) in [
            (0u64, 0u64),
            (750_000_000, 700_000_000),
            (750_000_000, 750_000_000),
            (750_000_000, 900_000_000),
            (u64::MAX, u64::MAX),
        ] {
            let a = decide_grant(cap, req);
            let b = decide_grant(cap, req);
            assert_eq!(a, b, "decide_grant varied for ({cap}, {req})");
        }
    }

    #[test]
    fn decide_grant_emits_only_release_cap_exceeded() {
        // Exhaustive: for every deny branch the function must emit
        // exactly DisbursementReasonCode::ReleaseCapExceeded — never
        // any other typed reason.
        for (cap, req) in [
            (0u64, 1u64),
            (1, 2),
            (750_000_000, 900_000_000),
            (0, u64::MAX),
        ] {
            assert_eq!(
                decide_grant(cap, req),
                Some(DisbursementReasonCode::ReleaseCapExceeded),
            );
        }
    }

    // ---- Composite: cap pipeline never panics on valid band inputs ----

    #[test]
    fn full_cap_pipeline_is_total_for_valid_bands() {
        // For every price-derived band and every allocation in u64,
        // compute_max_releasable_lovelace must return Some and
        // decide_grant must emit either None or ReleaseCapExceeded —
        // never panic and never emit any other reason.
        let prices = [
            0u64,
            1,
            258_000,
            349_999,
            350_000,
            450_000,
            500_000,
            u64::MAX,
        ];
        let allocations = [0u64, 1, 1_000_000_000, u64::MAX];
        let requests = [0u64, 1, 700_000_000, 900_000_000, u64::MAX];
        for price in prices {
            for alloc in allocations {
                let bps = select_release_band_bps(price);
                let cap = match compute_max_releasable_lovelace(alloc, bps) {
                    Some(c) => c,
                    None => panic!("band {bps} produced None for allocation {alloc}"),
                };
                for req in requests {
                    let outcome = decide_grant(cap, req);
                    match outcome {
                        None | Some(DisbursementReasonCode::ReleaseCapExceeded) => {}
                        Some(other) => panic!("unexpected reason {other:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn every_band_output_is_from_the_frozen_set() {
        // Exhaustive sweep over a dense set of boundary and interior
        // points; every output must be one of the three pinned band
        // constants.
        let sample_prices = [
            0u64,
            1,
            100_000,
            349_998,
            349_999,
            350_000,
            350_001,
            450_000,
            499_999,
            500_000,
            500_001,
            1_000_000,
            u64::MAX - 1,
            u64::MAX,
        ];
        for price in sample_prices {
            let bps = select_release_band_bps(price);
            assert!(
                bps == BAND_LOW_BPS || bps == BAND_MID_BPS || bps == BAND_HIGH_BPS,
                "price {price} produced non-band value {bps}"
            );
        }
    }
}
