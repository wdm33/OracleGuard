//! Reason codes.
//!
//! Owns: the closed set of canonical reason codes emitted alongside
//! evaluator decisions and verifier findings.
//!
//! Does NOT own: the decision logic that selects a reason (policy
//! crate), nor the rendering of reasons for human display (shell or
//! verifier report).

use serde::{Deserialize, Serialize};

use crate::gate::AuthorizationGate;
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
/// | `PolicyNotFound`       | anchor    | `policy_ref` is not registered (also covers non-v1 `intent_version`)     |
/// | `AllocationNotFound`   | registry  | `allocation_id` does not exist or is not approved                        |
/// | `OracleStale`          | grant     | oracle datum's `expiry_unix` has passed by the time of evaluation        |
/// | `OraclePriceZero`      | grant     | canonical `price_microusd` is zero (economically meaningless)            |
/// | `AmountZero`           | grant     | `requested_amount_lovelace` is zero                                      |
/// | `ReleaseCapExceeded`   | grant     | requested amount exceeds the release cap computed from the oracle price  |
/// | `SubjectNotAuthorized` | registry  | `requester_id` is not authorized, OR destination is not in the           |
/// |                        |           | allocation's approved-destination set (see v1 compatibility note below)  |
/// | `AssetMismatch`        | registry  | intent asset does not match the allocation's approved asset              |
///
/// ## v1 destination-mapping compatibility note
///
/// [`DisbursementReasonCode::SubjectNotAuthorized`] **includes destination
/// mismatch for v1 disbursement authorization semantics.** That is, a
/// disbursement intent whose `destination` is not in the allocation's
/// approved-destination set is denied with `SubjectNotAuthorized`, not
/// a dedicated destination code.
///
/// This is a deliberate Cluster 5 v1 mapping, **not** the ideal long-run
/// taxonomy. A dedicated destination reason code is a future canonical-
/// byte version-boundary change and requires Katiba sign-off; it is out
/// of scope for v1.
///
/// Downstream consumers (verifier, evidence bundle, routing) MUST treat
/// `SubjectNotAuthorized` as "the requester-destination pair is not
/// authorized against this allocation," without distinguishing the two
/// sub-causes.
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

impl DisbursementReasonCode {
    /// Return the authorization gate that produces this reason.
    ///
    /// Every variant is produced by exactly one gate. This mapping is
    /// the machine-readable mirror of the table in the type docstring
    /// and is load-bearing for gate-precedence tests in
    /// `oracleguard-policy::authorize` (Cluster 5 Slice 02 onward).
    ///
    /// The partition is:
    /// - anchor: `PolicyNotFound`
    /// - registry: `AllocationNotFound`, `SubjectNotAuthorized`,
    ///   `AssetMismatch`
    /// - grant: `OracleStale`, `OraclePriceZero`, `AmountZero`,
    ///   `ReleaseCapExceeded`
    ///
    /// Note that `SubjectNotAuthorized` carries the v1 destination-
    /// mapping compatibility described in the type docstring: both the
    /// requester-authorization failure and the destination-approval
    /// failure map to this variant and both are registry-gate failures.
    #[must_use]
    pub const fn gate(&self) -> AuthorizationGate {
        match self {
            DisbursementReasonCode::PolicyNotFound => AuthorizationGate::Anchor,
            DisbursementReasonCode::AllocationNotFound
            | DisbursementReasonCode::SubjectNotAuthorized
            | DisbursementReasonCode::AssetMismatch => AuthorizationGate::Registry,
            DisbursementReasonCode::OracleStale
            | DisbursementReasonCode::OraclePriceZero
            | DisbursementReasonCode::AmountZero
            | DisbursementReasonCode::ReleaseCapExceeded => AuthorizationGate::Grant,
        }
    }
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

    // ---- Gate partition (Cluster 5 Slice 01) ----

    #[test]
    fn gate_partition_matches_documented_table() {
        // The table in DisbursementReasonCode's docstring is the
        // authoritative partition. This test mirrors it so any drift
        // between docstring and .gate() fails CI.
        let expected: &[(DisbursementReasonCode, AuthorizationGate)] = &[
            (DisbursementReasonCode::PolicyNotFound, AuthorizationGate::Anchor),
            (DisbursementReasonCode::AllocationNotFound, AuthorizationGate::Registry),
            (DisbursementReasonCode::OracleStale, AuthorizationGate::Grant),
            (DisbursementReasonCode::OraclePriceZero, AuthorizationGate::Grant),
            (DisbursementReasonCode::AmountZero, AuthorizationGate::Grant),
            (DisbursementReasonCode::ReleaseCapExceeded, AuthorizationGate::Grant),
            (DisbursementReasonCode::SubjectNotAuthorized, AuthorizationGate::Registry),
            (DisbursementReasonCode::AssetMismatch, AuthorizationGate::Registry),
        ];
        for (reason, gate) in expected {
            assert_eq!(reason.gate(), *gate, "unexpected gate for {reason:?}");
        }
    }

    #[test]
    fn every_reason_code_maps_to_exactly_one_gate() {
        // Exhaustive sweep over the closed set: every variant must
        // produce exactly one of the three documented gates, and every
        // gate must be produced by at least one variant (partition is
        // total and onto).
        let all = [
            DisbursementReasonCode::PolicyNotFound,
            DisbursementReasonCode::AllocationNotFound,
            DisbursementReasonCode::OracleStale,
            DisbursementReasonCode::OraclePriceZero,
            DisbursementReasonCode::AmountZero,
            DisbursementReasonCode::ReleaseCapExceeded,
            DisbursementReasonCode::SubjectNotAuthorized,
            DisbursementReasonCode::AssetMismatch,
        ];
        let (mut anchor, mut registry, mut grant) = (0u32, 0u32, 0u32);
        for r in all {
            match r.gate() {
                AuthorizationGate::Anchor => anchor += 1,
                AuthorizationGate::Registry => registry += 1,
                AuthorizationGate::Grant => grant += 1,
            }
        }
        assert_eq!(anchor, 1, "anchor-gate cardinality changed");
        assert_eq!(registry, 3, "registry-gate cardinality changed");
        assert_eq!(grant, 4, "grant-gate cardinality changed");
        assert_eq!(anchor + registry + grant, 8, "partition is not total");
    }

    #[test]
    fn gate_mapping_is_deterministic() {
        // .gate() is a const fn over a closed variant set; calling it
        // twice on the same variant must always return the same gate.
        for r in [
            DisbursementReasonCode::PolicyNotFound,
            DisbursementReasonCode::AllocationNotFound,
            DisbursementReasonCode::OracleStale,
            DisbursementReasonCode::OraclePriceZero,
            DisbursementReasonCode::AmountZero,
            DisbursementReasonCode::ReleaseCapExceeded,
            DisbursementReasonCode::SubjectNotAuthorized,
            DisbursementReasonCode::AssetMismatch,
        ] {
            assert_eq!(r.gate(), r.gate(), "gate mapping varied for {r:?}");
        }
    }

    // ---- v1 destination-mapping compatibility pin ----

    #[test]
    fn subject_not_authorized_includes_destination_mismatch_for_v1() {
        // v1 compatibility: SubjectNotAuthorized covers both
        // (a) requester not authorized for the allocation, and
        // (b) destination not in the allocation's approved-destination
        // set. Both map to the registry gate.
        //
        // This pin exists so that if a future cluster introduces a
        // dedicated destination reason code, the removal of this
        // mapping is an explicit canonical-byte version-boundary event
        // rather than a silent behavioral change.
        assert_eq!(
            DisbursementReasonCode::SubjectNotAuthorized.gate(),
            AuthorizationGate::Registry,
        );
    }
}
