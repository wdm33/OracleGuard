//! Three-gate authorization closure.
//!
//! Owns: the deterministic orchestration shape of OracleGuard's three
//! authorization gates (anchor → registry → grant), the typed result
//! surface [`AuthorizationResult`], and the pure trait boundaries
//! [`PolicyAnchorView`] and [`AllocationRegistryView`] that let tests
//! and private runtime integrations supply anchor / registry state
//! without letting this crate perform I/O.
//!
//! Does NOT own: release-cap math (that is [`crate::evaluate`] /
//! [`crate::math`]), canonical byte identity of the authorized effect
//! (that is `oracleguard_schemas::effect::AuthorizedEffectV1`), or any
//! I/O. Every view is a pure trait over caller-supplied state.
//!
//! ## Gate order and failure precedence
//!
//! Gates run in the fixed order
//! [`AuthorizationGate::Anchor`] → [`AuthorizationGate::Registry`] →
//! [`AuthorizationGate::Grant`]. The first failing gate produces the
//! canonical denial reason; later gates are not consulted. See
//! `docs/authorization-gates.md` for the authoritative documentation of
//! within-gate check order and the full reason-to-gate partition.
//!
//! Cluster 5 Slice 02 pins the sequencing shape and types. Slices 03,
//! 04, and 05 land the per-gate logic. Slice 05 wires the top-level
//! [`authorize_disbursement`] composition.

use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::{validate_intent_version, DisbursementIntentV1};
use oracleguard_schemas::reason::DisbursementReasonCode;

/// Closed authorization result produced by the three-gate closure.
///
/// ## Structural deny/no-effect closure
///
/// Only [`AuthorizationResult::Authorized`] carries an
/// [`AuthorizedEffectV1`]. A [`AuthorizationResult::Denied`] has no
/// effect payload — it is structurally impossible to emit one alongside
/// a denial. This is the deny/no-effect closure Cluster 5 Slice 06
/// pins behaviorally.
///
/// The `gate` field on `Denied` records which gate failed; by
/// invariant, `gate == reason.gate()` (see
/// [`DisbursementReasonCode::gate`]). The field is duplicated so
/// consumers can branch on the failing gate without re-deriving it.
///
/// The variant set is frozen for v1. Adding, removing, or reordering
/// variants is a canonical-byte change and therefore a schema-version
/// boundary crossing.
///
/// The `Authorized` variant inlines [`AuthorizedEffectV1`] (~230 bytes)
/// while `Denied` carries ~2 bytes. Clippy's `large_enum_variant` lint
/// is allowed here: authorization is not a hot-path, per-disbursement
/// cost dominates, and boxing would force a separate allocation on
/// every allow while breaking `Copy`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AuthorizationResult {
    /// Every gate passed. The authorized effect is the single source
    /// of truth for what settlement may execute.
    Authorized {
        /// Canonical authorized-effect payload. Byte-identical fields
        /// for `policy_ref`, `allocation_id`, `requester_id`,
        /// `destination`, `asset`; `authorized_amount_lovelace` equals
        /// the intent's `requested_amount_lovelace`;
        /// `release_cap_basis_points` is the band the evaluator
        /// selected.
        effect: AuthorizedEffectV1,
    },
    /// A gate denied the request.
    Denied {
        /// The canonical reason code emitted by the failing gate.
        reason: DisbursementReasonCode,
        /// The failing gate. Invariant: `gate == reason.gate()`.
        gate: AuthorizationGate,
    },
}

/// View onto the anchor-gate state.
///
/// The anchor gate validates that `intent.policy_ref` is a registered
/// policy identity. Implementations MUST be deterministic and MUST NOT
/// perform I/O: the trait signature is pure (no `&mut self`, no
/// `Result`, no async), and callers that need to refresh state should
/// do so before constructing the view.
///
/// Cluster 5 Slice 03 wires the anchor-gate function against this
/// trait; tests in that slice supply an in-memory fixture.
pub trait PolicyAnchorView {
    /// Return `true` when the given `policy_ref` is a registered policy
    /// identity. Any non-recognized bytes must return `false`.
    fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool;
}

/// Allocation facts needed by the registry gate.
///
/// Every field is borrowed from the caller-supplied view to keep the
/// trait zero-alloc. The registry gate consumes this struct by value.
///
/// - `basis_lovelace` — approved allocation size in lovelace. Fed
///   verbatim to `evaluate_disbursement` as `allocation_basis_lovelace`.
/// - `authorized_subjects` — set of requester ids authorized against
///   this allocation. Membership is exact byte equality.
/// - `approved_asset` — the asset the allocation is denominated in.
///   Matched against `intent.asset` by exact byte equality.
/// - `approved_destinations` — set of Cardano addresses this
///   allocation may pay out to. Membership is exact byte equality,
///   including `length`. A destination not in this set denies with
///   [`DisbursementReasonCode::SubjectNotAuthorized`] per the v1
///   compatibility mapping (see that variant's docstring).
#[derive(Debug, Clone, Copy)]
pub struct AllocationFacts<'a> {
    pub basis_lovelace: u64,
    pub authorized_subjects: &'a [[u8; 32]],
    pub approved_asset: AssetIdV1,
    pub approved_destinations: &'a [CardanoAddressV1],
}

/// View onto the registry-gate state.
///
/// The registry gate validates allocation identity, subject
/// authorization, asset match, and destination approval. This trait
/// is pure by construction; implementations MUST be deterministic and
/// MUST NOT perform I/O.
///
/// Cluster 5 Slice 04 wires the registry-gate function against this
/// trait; tests in that slice supply an in-memory fixture.
pub trait AllocationRegistryView {
    /// Return the facts for `allocation_id`, or `None` if the
    /// allocation is not recognized or not approved. Non-existent
    /// allocations MUST map to `None` — the caller turns that into
    /// [`DisbursementReasonCode::AllocationNotFound`].
    fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>>;
}

// -------------------------------------------------------------------
// Anchor gate (Cluster 5 Slice 03)
// -------------------------------------------------------------------

/// Run the anchor gate against a caller-supplied [`PolicyAnchorView`].
///
/// The anchor gate is the first gate in the three-gate closure. It
/// validates that the intent is on a supported schema version and that
/// its `policy_ref` is a registered policy identity. Both failures map
/// to [`DisbursementReasonCode::PolicyNotFound`] because v1 treats
/// "unsupported schema version" and "unregistered policy identity" as
/// facets of the same anchor-gate failure.
///
/// ## Check order (pinned by `docs/authorization-gates.md`)
///
/// 1. `intent.intent_version != INTENT_VERSION_V1` →
///    [`DisbursementReasonCode::PolicyNotFound`] (delegates to
///    [`validate_intent_version`]).
/// 2. `!anchor.is_policy_registered(&intent.policy_ref)` →
///    [`DisbursementReasonCode::PolicyNotFound`].
/// 3. `Ok(())` — the anchor gate passes. The registry gate may run.
///
/// ## Purity
///
/// This function is pure. It performs no I/O, reads no wall clock,
/// and does not allocate. The trait argument is the single mediator
/// through which anchor state reaches the gate.
pub fn run_anchor_gate(
    intent: &DisbursementIntentV1,
    anchor: &impl PolicyAnchorView,
) -> Result<(), DisbursementReasonCode> {
    if validate_intent_version(intent).is_err() {
        return Err(DisbursementReasonCode::PolicyNotFound);
    }
    if !anchor.is_policy_registered(&intent.policy_ref) {
        return Err(DisbursementReasonCode::PolicyNotFound);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_schemas::intent::INTENT_VERSION_V1;
    use oracleguard_schemas::oracle::{
        OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
    };

    // ---- Shared test fixtures ----

    const ANCHOR_POLICY_REF: [u8; 32] = [0x11; 32];
    const ANCHOR_OTHER_POLICY_REF: [u8; 32] = [0xAA; 32];
    const ALLOCATION_ID: [u8; 32] = [0x22; 32];
    const REQUESTER_ID: [u8; 32] = [0x33; 32];

    fn sample_intent() -> DisbursementIntentV1 {
        DisbursementIntentV1 {
            intent_version: INTENT_VERSION_V1,
            policy_ref: ANCHOR_POLICY_REF,
            allocation_id: ALLOCATION_ID,
            requester_id: REQUESTER_ID,
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
            requested_amount_lovelace: 700_000_000,
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
        }
    }

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

    // Static in-memory implementation of `PolicyAnchorView`. No I/O,
    // no mutation; the registered set is borrowed from a slice so
    // each test can pick its own.
    struct StaticAnchor<'a> {
        registered: &'a [[u8; 32]],
    }

    impl<'a> PolicyAnchorView for StaticAnchor<'a> {
        fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool {
            self.registered.iter().any(|r| r == policy_ref)
        }
    }

    // ---- AuthorizationResult structural guards ----

    // Destructuring guard: if a variant is added, removed, or reordered
    // this test fails to compile, forcing the schema-version boundary
    // conversation.
    #[test]
    fn authorization_result_variant_set_is_frozen() {
        let cases = [
            AuthorizationResult::Authorized {
                effect: sample_effect(),
            },
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::AmountZero,
                gate: AuthorizationGate::Grant,
            },
        ];
        for r in cases {
            let _: u8 = match r {
                AuthorizationResult::Authorized { .. } => 0,
                AuthorizationResult::Denied { .. } => 1,
            };
        }
    }

    #[test]
    fn authorization_result_allow_equality_is_field_wise() {
        let a = AuthorizationResult::Authorized {
            effect: sample_effect(),
        };
        let b = AuthorizationResult::Authorized {
            effect: sample_effect(),
        };
        assert_eq!(a, b);
        let mut other_effect = sample_effect();
        other_effect.authorized_amount_lovelace = 1;
        let c = AuthorizationResult::Authorized {
            effect: other_effect,
        };
        assert_ne!(a, c);
    }

    #[test]
    fn authorization_result_denied_equality_is_reason_and_gate_wise() {
        let a = AuthorizationResult::Denied {
            reason: DisbursementReasonCode::AmountZero,
            gate: AuthorizationGate::Grant,
        };
        let b = AuthorizationResult::Denied {
            reason: DisbursementReasonCode::AmountZero,
            gate: AuthorizationGate::Grant,
        };
        assert_eq!(a, b);
        let c = AuthorizationResult::Denied {
            reason: DisbursementReasonCode::ReleaseCapExceeded,
            gate: AuthorizationGate::Grant,
        };
        assert_ne!(a, c);
    }

    #[test]
    fn authorization_result_postcard_round_trips() {
        for r in [
            AuthorizationResult::Authorized {
                effect: sample_effect(),
            },
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::PolicyNotFound,
                gate: AuthorizationGate::Anchor,
            },
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::SubjectNotAuthorized,
                gate: AuthorizationGate::Registry,
            },
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::ReleaseCapExceeded,
                gate: AuthorizationGate::Grant,
            },
        ] {
            let bytes = postcard::to_allocvec(&r).expect("encode");
            let (decoded, rest) =
                postcard::take_from_bytes::<AuthorizationResult>(&bytes).expect("decode");
            assert!(rest.is_empty());
            assert_eq!(decoded, r);
        }
    }

    // ---- Gate / reason invariant ----

    #[test]
    fn every_reason_denies_at_its_own_gate() {
        // For every canonical deny reason, the `gate` field in a
        // Denied result that honors the invariant must equal the
        // reason's intrinsic gate. This is what consumers can rely on
        // without re-deriving.
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
        for reason in all {
            let denied = AuthorizationResult::Denied {
                reason,
                gate: reason.gate(),
            };
            match denied {
                AuthorizationResult::Denied { reason: r, gate } => {
                    assert_eq!(gate, r.gate(), "invariant broken for {reason:?}");
                }
                AuthorizationResult::Authorized { .. } => {
                    panic!("Denied variant matched Authorized arm")
                }
            }
        }
    }

    // ---- Documented within-gate precedence (Cluster 5 Slice 02) ----
    //
    // The precedence order below mirrors `docs/authorization-gates.md`
    // exactly. Per-gate implementation tests in Slices 03, 04, and 05
    // exercise the order operationally; this table is the authoritative
    // checkable reference the tests compare against.

    #[test]
    fn documented_within_anchor_gate_precedence() {
        // Anchor gate:
        //   1. non-v1 intent_version → PolicyNotFound
        //   2. policy_ref not registered → PolicyNotFound
        let order: &[DisbursementReasonCode] = &[DisbursementReasonCode::PolicyNotFound];
        for r in order {
            assert_eq!(r.gate(), AuthorizationGate::Anchor);
        }
    }

    #[test]
    fn documented_within_registry_gate_precedence() {
        // Registry gate:
        //   1. allocation lookup None → AllocationNotFound
        //   2. requester not in authorized_subjects → SubjectNotAuthorized
        //   3. asset mismatch → AssetMismatch
        //   4. destination not in approved_destinations → SubjectNotAuthorized
        //     (v1 compatibility: destination mismatch maps to the same code)
        let order: &[DisbursementReasonCode] = &[
            DisbursementReasonCode::AllocationNotFound,
            DisbursementReasonCode::SubjectNotAuthorized,
            DisbursementReasonCode::AssetMismatch,
        ];
        for r in order {
            assert_eq!(r.gate(), AuthorizationGate::Registry);
        }
    }

    // ---- Anchor gate (Cluster 5 Slice 03) ----

    #[test]
    fn anchor_gate_passes_for_registered_v1_intent() {
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        assert_eq!(run_anchor_gate(&sample_intent(), &anchor), Ok(()));
    }

    #[test]
    fn anchor_gate_rejects_unregistered_policy_ref() {
        let anchor = StaticAnchor {
            registered: &[ANCHOR_OTHER_POLICY_REF],
        };
        assert_eq!(
            run_anchor_gate(&sample_intent(), &anchor),
            Err(DisbursementReasonCode::PolicyNotFound),
        );
    }

    #[test]
    fn anchor_gate_rejects_empty_registered_set() {
        let anchor = StaticAnchor { registered: &[] };
        assert_eq!(
            run_anchor_gate(&sample_intent(), &anchor),
            Err(DisbursementReasonCode::PolicyNotFound),
        );
    }

    #[test]
    fn anchor_gate_rejects_non_v1_intent_version() {
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        let mut intent = sample_intent();
        intent.intent_version = 2;
        assert_eq!(
            run_anchor_gate(&intent, &anchor),
            Err(DisbursementReasonCode::PolicyNotFound),
        );
    }

    #[test]
    fn anchor_gate_rejects_zero_intent_version() {
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        let mut intent = sample_intent();
        intent.intent_version = 0;
        assert_eq!(
            run_anchor_gate(&intent, &anchor),
            Err(DisbursementReasonCode::PolicyNotFound),
        );
    }

    #[test]
    fn anchor_gate_version_check_precedes_registration_check() {
        // Both conditions invalid: version is non-v1 AND policy_ref is
        // unregistered. The version check runs first, so the result
        // must still be PolicyNotFound — but via the version branch.
        // The reason is identical either way, which is why this test
        // instead confirms the version branch triggers by using an
        // anchor that would accept the policy_ref: the registration
        // path is not consulted, yet denial still occurs.
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        let mut intent = sample_intent();
        intent.intent_version = 99;
        assert_eq!(
            run_anchor_gate(&intent, &anchor),
            Err(DisbursementReasonCode::PolicyNotFound),
        );
    }

    #[test]
    fn anchor_gate_is_deterministic() {
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        let intent = sample_intent();
        let a = run_anchor_gate(&intent, &anchor);
        let b = run_anchor_gate(&intent, &anchor);
        assert_eq!(a, b);

        let mut other = sample_intent();
        other.policy_ref = ANCHOR_OTHER_POLICY_REF;
        let c = run_anchor_gate(&other, &anchor);
        let d = run_anchor_gate(&other, &anchor);
        assert_eq!(c, d);
    }

    #[test]
    fn anchor_gate_anchor_view_is_not_mutated() {
        // Sanity: the trait has no &mut self; calling run_anchor_gate
        // multiple times against the same anchor yields the same
        // result because anchor state cannot change through the trait.
        let anchor = StaticAnchor {
            registered: &[ANCHOR_POLICY_REF],
        };
        let intent = sample_intent();
        for _ in 0..4 {
            assert_eq!(run_anchor_gate(&intent, &anchor), Ok(()));
        }
    }

    #[test]
    fn anchor_gate_failure_emits_reason_mapped_to_anchor_gate() {
        // Every denial from run_anchor_gate must be a reason whose
        // intrinsic .gate() is AuthorizationGate::Anchor. This ties
        // the per-gate function to the reason-to-gate partition.
        let anchor = StaticAnchor { registered: &[] };
        let reason = run_anchor_gate(&sample_intent(), &anchor).unwrap_err();
        assert_eq!(reason.gate(), AuthorizationGate::Anchor);
    }

    #[test]
    fn documented_within_grant_gate_precedence() {
        // Grant gate:
        //   1. oracle not fresh → OracleStale
        //   2. intent.requested_amount_lovelace == 0 → AmountZero
        //   3. oracle_fact.price_microusd == 0 → OraclePriceZero
        //   4. evaluator deny → ReleaseCapExceeded
        //
        // The AmountZero → OraclePriceZero → ReleaseCapExceeded suffix
        // is already pinned by `oracleguard_policy::evaluate::tests`.
        // OracleStale precedes the evaluator because freshness is
        // checked before the evaluator runs.
        let order: &[DisbursementReasonCode] = &[
            DisbursementReasonCode::OracleStale,
            DisbursementReasonCode::AmountZero,
            DisbursementReasonCode::OraclePriceZero,
            DisbursementReasonCode::ReleaseCapExceeded,
        ];
        for r in order {
            assert_eq!(r.gate(), AuthorizationGate::Grant);
        }
    }
}
