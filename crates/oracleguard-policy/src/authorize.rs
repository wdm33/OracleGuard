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
use oracleguard_schemas::oracle::check_freshness;
use oracleguard_schemas::reason::DisbursementReasonCode;

use crate::evaluate::evaluate_disbursement;
use crate::error::EvaluationResult;

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
// Registry gate (Cluster 5 Slice 04)
// -------------------------------------------------------------------

/// Run the registry gate against a caller-supplied
/// [`AllocationRegistryView`] and return the approved allocation basis.
///
/// The registry gate is the second gate in the three-gate closure. It
/// validates that the allocation exists, the requester is authorized
/// against it, the intent's asset matches the allocation's approved
/// asset, and the intent's destination is in the allocation's
/// approved-destination set.
///
/// ## Check order (pinned by `docs/authorization-gates.md`)
///
/// 1. `registry.lookup(&intent.allocation_id) == None` →
///    [`DisbursementReasonCode::AllocationNotFound`].
/// 2. `intent.requester_id` not in `authorized_subjects` →
///    [`DisbursementReasonCode::SubjectNotAuthorized`].
/// 3. `intent.asset != approved_asset` (byte-identical) →
///    [`DisbursementReasonCode::AssetMismatch`].
/// 4. `intent.destination` not in `approved_destinations`
///    (byte-identical, including `length`) →
///    [`DisbursementReasonCode::SubjectNotAuthorized`] per the v1
///    destination-mapping compatibility (see the variant docstring
///    and `docs/authorization-gates.md §Registry gate`).
/// 5. `Ok(basis_lovelace)` — registry gate passes. The grant gate may
///    run and will receive `basis_lovelace` as the evaluator's
///    allocation-basis argument.
///
/// ## Purity
///
/// This function is pure. It performs no I/O, reads no wall clock,
/// and does not allocate. The trait argument is the single mediator
/// through which allocation state reaches the gate.
pub fn run_registry_gate(
    intent: &DisbursementIntentV1,
    registry: &impl AllocationRegistryView,
) -> Result<u64, DisbursementReasonCode> {
    let facts = match registry.lookup(&intent.allocation_id) {
        Some(f) => f,
        None => return Err(DisbursementReasonCode::AllocationNotFound),
    };

    if !facts.authorized_subjects.contains(&intent.requester_id) {
        return Err(DisbursementReasonCode::SubjectNotAuthorized);
    }

    if intent.asset != facts.approved_asset {
        return Err(DisbursementReasonCode::AssetMismatch);
    }

    if !facts.approved_destinations.contains(&intent.destination) {
        // v1 destination-mapping compatibility: destination mismatch
        // emits SubjectNotAuthorized. See the variant docstring and
        // `docs/authorization-gates.md §Registry gate` for the
        // rationale.
        return Err(DisbursementReasonCode::SubjectNotAuthorized);
    }

    Ok(facts.basis_lovelace)
}

// -------------------------------------------------------------------
// Grant gate (Cluster 5 Slice 05)
// -------------------------------------------------------------------

/// Run the grant gate for an already-anchored, already-registry-valid
/// intent and return the canonical authorized effect on success.
///
/// The grant gate is the third gate in the three-gate closure. It
/// validates oracle freshness and then delegates the release-cap
/// decision to the pure evaluator
/// [`crate::evaluate::evaluate_disbursement`]. It does NOT re-derive
/// any band math, threshold table, or cap arithmetic — all of that is
/// the evaluator's responsibility.
///
/// ## Check order (pinned by `docs/authorization-gates.md`)
///
/// 1. [`check_freshness`] against `intent.oracle_provenance` with the
///    caller-supplied `now_unix_ms` → on error,
///    [`DisbursementReasonCode::OracleStale`].
/// 2. [`evaluate_disbursement`] with `basis_lovelace`:
///    - `Allow { authorized_amount_lovelace, release_cap_basis_points }`
///      → `Ok(AuthorizedEffectV1 { ... })` with fields copied
///      verbatim from the intent.
///    - `Deny { reason }` → `Err(reason)` (one of `AmountZero`,
///      `OraclePriceZero`, `ReleaseCapExceeded`).
///
/// ## Purity
///
/// This function is pure with respect to its arguments. `now_unix_ms`
/// is taken explicitly so the wall clock does not leak into the
/// gate; the caller reads the clock and passes it in.
///
/// ## Delegation to the evaluator
///
/// The `AmountZero → OraclePriceZero → ReleaseCapExceeded` suffix is
/// the evaluator's internal order; the grant gate inherits it by
/// construction. Freshness runs first so a stale datum cannot feed
/// the evaluator.
pub fn run_grant_gate(
    intent: &DisbursementIntentV1,
    basis_lovelace: u64,
    now_unix_ms: u64,
) -> Result<AuthorizedEffectV1, DisbursementReasonCode> {
    if check_freshness(&intent.oracle_provenance, now_unix_ms).is_err() {
        return Err(DisbursementReasonCode::OracleStale);
    }
    match evaluate_disbursement(intent, basis_lovelace) {
        EvaluationResult::Allow {
            authorized_amount_lovelace,
            release_cap_basis_points,
        } => Ok(AuthorizedEffectV1 {
            policy_ref: intent.policy_ref,
            allocation_id: intent.allocation_id,
            requester_id: intent.requester_id,
            destination: intent.destination,
            asset: intent.asset,
            authorized_amount_lovelace,
            release_cap_basis_points,
        }),
        EvaluationResult::Deny { reason } => Err(reason),
    }
}

// -------------------------------------------------------------------
// Top-level three-gate composition (Cluster 5 Slice 05)
// -------------------------------------------------------------------

/// Run the full three-gate authorization closure and return a typed
/// [`AuthorizationResult`].
///
/// This is the single public entry point for OracleGuard's
/// authorization surface. Gates run in the fixed order
/// anchor → registry → grant; the first failing gate produces the
/// canonical denial reason and later gates are not consulted.
///
/// ## Contract
///
/// - `intent` — the canonical disbursement intent under authorization.
///   The anchor gate validates its `intent_version`.
/// - `anchor` — a pure view onto registered policy identities. The
///   anchor gate calls `anchor.is_policy_registered(&intent.policy_ref)`.
/// - `registry` — a pure view onto allocations. The registry gate
///   calls `registry.lookup(&intent.allocation_id)` and validates
///   subject, asset, and destination against the returned facts.
/// - `now_unix_ms` — wall-clock value for the freshness check. The
///   caller reads the clock; this function is pure with respect to
///   its arguments.
///
/// ## Return values
///
/// - `AuthorizationResult::Authorized { effect }` — every gate passed.
///   The `effect` carries the canonical bytes settlement will execute.
/// - `AuthorizationResult::Denied { reason, gate }` — a gate denied.
///   `gate` always equals `reason.gate()`.
///
/// ## Purity and determinism
///
/// Identical inputs produce byte-identical outputs across runs. No
/// I/O, no internal wall-clock reads, no mutable state.
pub fn authorize_disbursement(
    intent: &DisbursementIntentV1,
    anchor: &impl PolicyAnchorView,
    registry: &impl AllocationRegistryView,
    now_unix_ms: u64,
) -> AuthorizationResult {
    if let Err(reason) = run_anchor_gate(intent, anchor) {
        return AuthorizationResult::Denied {
            reason,
            gate: AuthorizationGate::Anchor,
        };
    }

    let basis_lovelace = match run_registry_gate(intent, registry) {
        Ok(b) => b,
        Err(reason) => {
            return AuthorizationResult::Denied {
                reason,
                gate: AuthorizationGate::Registry,
            };
        }
    };

    match run_grant_gate(intent, basis_lovelace, now_unix_ms) {
        Ok(effect) => AuthorizationResult::Authorized { effect },
        Err(reason) => AuthorizationResult::Denied {
            reason,
            gate: AuthorizationGate::Grant,
        },
    }
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

    // Static in-memory implementation of `AllocationRegistryView`. The
    // allocation id, basis, subject set, asset, and destination set
    // are all borrowed from the test so the trait stays zero-alloc.
    struct StaticRegistry<'a> {
        allocation_id: [u8; 32],
        basis_lovelace: u64,
        authorized_subjects: &'a [[u8; 32]],
        approved_asset: AssetIdV1,
        approved_destinations: &'a [CardanoAddressV1],
    }

    impl<'a> AllocationRegistryView for StaticRegistry<'a> {
        fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
            if *allocation_id == self.allocation_id {
                Some(AllocationFacts {
                    basis_lovelace: self.basis_lovelace,
                    authorized_subjects: self.authorized_subjects,
                    approved_asset: self.approved_asset,
                    approved_destinations: self.approved_destinations,
                })
            } else {
                None
            }
        }
    }

    fn sample_destination() -> CardanoAddressV1 {
        CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        }
    }

    fn other_destination() -> CardanoAddressV1 {
        CardanoAddressV1 {
            bytes: [0x66; 57],
            length: 57,
        }
    }

    fn ok_registry<'a>(
        subjects: &'a [[u8; 32]],
        destinations: &'a [CardanoAddressV1],
    ) -> StaticRegistry<'a> {
        StaticRegistry {
            allocation_id: ALLOCATION_ID,
            basis_lovelace: 1_000_000_000,
            authorized_subjects: subjects,
            approved_asset: AssetIdV1::ADA,
            approved_destinations: destinations,
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

    // ---- Registry gate (Cluster 5 Slice 04) ----

    #[test]
    fn registry_gate_passes_when_all_checks_match() {
        let subjects = [REQUESTER_ID];
        let destinations = [sample_destination()];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Ok(1_000_000_000),
        );
    }

    #[test]
    fn registry_gate_returns_basis_verbatim() {
        // The registry gate passes basis_lovelace through unchanged
        // for the grant gate to consume.
        let subjects = [REQUESTER_ID];
        let destinations = [sample_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.basis_lovelace = 7_777_777;
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Ok(7_777_777),
        );
    }

    #[test]
    fn registry_gate_missing_allocation_emits_allocation_not_found() {
        let subjects = [REQUESTER_ID];
        let destinations = [sample_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.allocation_id = [0xDE; 32];
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::AllocationNotFound),
        );
    }

    #[test]
    fn registry_gate_unauthorized_subject_emits_subject_not_authorized() {
        let subjects = [[0x99; 32]];
        let destinations = [sample_destination()];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_asset_mismatch_emits_asset_mismatch() {
        let subjects = [REQUESTER_ID];
        let destinations = [sample_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.approved_asset = AssetIdV1 {
            policy_id: [0xAB; 28],
            asset_name: *b"OTHER\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
            asset_name_len: 5,
        };
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::AssetMismatch),
        );
    }

    #[test]
    fn registry_gate_destination_mismatch_emits_subject_not_authorized_v1_mapping() {
        // v1 compatibility pin: destination mismatch maps to
        // SubjectNotAuthorized. A dedicated destination reason code is
        // deferred to a future canonical-byte version-boundary change.
        // See the DisbursementReasonCode::SubjectNotAuthorized
        // docstring.
        let subjects = [REQUESTER_ID];
        let destinations = [other_destination()];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_empty_destination_set_denies_destination() {
        let subjects = [REQUESTER_ID];
        let destinations: [CardanoAddressV1; 0] = [];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_empty_subject_set_denies_subject() {
        let subjects: [[u8; 32]; 0] = [];
        let destinations = [sample_destination()];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_allocation_precedes_subject() {
        // Both allocation and subject are invalid. Allocation lookup
        // runs first, so AllocationNotFound wins.
        let subjects = [[0x99; 32]];
        let destinations = [other_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.allocation_id = [0xDE; 32];
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::AllocationNotFound),
        );
    }

    #[test]
    fn registry_gate_subject_precedes_asset() {
        // Subject invalid AND asset invalid → SubjectNotAuthorized.
        let subjects = [[0x99; 32]];
        let destinations = [sample_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.approved_asset = AssetIdV1 {
            policy_id: [0xAB; 28],
            asset_name: [0u8; 32],
            asset_name_len: 0,
        };
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_asset_precedes_destination() {
        // Asset invalid AND destination invalid → AssetMismatch.
        let subjects = [REQUESTER_ID];
        let destinations = [other_destination()];
        let mut registry = ok_registry(&subjects, &destinations);
        registry.approved_asset = AssetIdV1 {
            policy_id: [0xAB; 28],
            asset_name: [0u8; 32],
            asset_name_len: 0,
        };
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::AssetMismatch),
        );
    }

    #[test]
    fn registry_gate_destination_address_length_matters() {
        // Two destinations with the same bytes but different lengths
        // must NOT compare equal. The approved destination set uses
        // exact byte-identical comparison.
        let subjects = [REQUESTER_ID];
        let mut approved = sample_destination();
        approved.length = 56;
        let destinations = [approved];
        let registry = ok_registry(&subjects, &destinations);
        assert_eq!(
            run_registry_gate(&sample_intent(), &registry),
            Err(DisbursementReasonCode::SubjectNotAuthorized),
        );
    }

    #[test]
    fn registry_gate_is_deterministic() {
        let subjects = [REQUESTER_ID];
        let destinations = [sample_destination()];
        let registry = ok_registry(&subjects, &destinations);
        let intent = sample_intent();
        for _ in 0..4 {
            assert_eq!(run_registry_gate(&intent, &registry), Ok(1_000_000_000));
        }
    }

    #[test]
    fn registry_gate_failure_emits_reason_mapped_to_registry_gate() {
        // Every denial from run_registry_gate must be a reason whose
        // intrinsic .gate() is AuthorizationGate::Registry.
        let subjects: [[u8; 32]; 0] = [];
        let destinations: [CardanoAddressV1; 0] = [];
        let registry = ok_registry(&subjects, &destinations);
        let reason = run_registry_gate(&sample_intent(), &registry).unwrap_err();
        assert_eq!(reason.gate(), AuthorizationGate::Registry);

        let mut reg2 = registry;
        reg2.allocation_id = [0xDE; 32];
        let reason2 = run_registry_gate(&sample_intent(), &reg2).unwrap_err();
        assert_eq!(reason2.gate(), AuthorizationGate::Registry);
    }

    // ---- Grant gate (Cluster 5 Slice 05) ----

    // Demo clock values (Implementation Plan §16 anchors).
    // Intent expiry_unix = 1_713_000_300_000. Fresh clock must be
    // strictly less than that.
    const FRESH_NOW_MS: u64 = 1_713_000_000_000;
    const STALE_NOW_MS: u64 = 1_713_000_300_000; // equal to expiry → stale
    const DEMO_BASIS_LOVELACE: u64 = 1_000_000_000;

    #[test]
    fn grant_gate_allows_within_cap_with_fresh_oracle() {
        let effect = run_grant_gate(&sample_intent(), DEMO_BASIS_LOVELACE, FRESH_NOW_MS)
            .expect("demo intent is within mid-band cap");
        assert_eq!(effect.policy_ref, sample_intent().policy_ref);
        assert_eq!(effect.allocation_id, sample_intent().allocation_id);
        assert_eq!(effect.requester_id, sample_intent().requester_id);
        assert_eq!(effect.destination, sample_intent().destination);
        assert_eq!(effect.asset, sample_intent().asset);
        assert_eq!(
            effect.authorized_amount_lovelace,
            sample_intent().requested_amount_lovelace,
        );
        assert_eq!(effect.release_cap_basis_points, 7_500);
    }

    #[test]
    fn grant_gate_rejects_stale_oracle_with_oracle_stale() {
        assert_eq!(
            run_grant_gate(&sample_intent(), DEMO_BASIS_LOVELACE, STALE_NOW_MS),
            Err(DisbursementReasonCode::OracleStale),
        );
    }

    #[test]
    fn grant_gate_freshness_precedes_evaluator() {
        // Intent has requested_amount_lovelace == 0, which alone would
        // trigger AmountZero in the evaluator. But the oracle is stale,
        // so freshness must fire first and win.
        let mut intent = sample_intent();
        intent.requested_amount_lovelace = 0;
        assert_eq!(
            run_grant_gate(&intent, DEMO_BASIS_LOVELACE, STALE_NOW_MS),
            Err(DisbursementReasonCode::OracleStale),
        );
    }

    #[test]
    fn grant_gate_propagates_amount_zero_from_evaluator() {
        let mut intent = sample_intent();
        intent.requested_amount_lovelace = 0;
        assert_eq!(
            run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS),
            Err(DisbursementReasonCode::AmountZero),
        );
    }

    #[test]
    fn grant_gate_propagates_oracle_price_zero_from_evaluator() {
        let mut intent = sample_intent();
        intent.oracle_fact.price_microusd = 0;
        assert_eq!(
            run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS),
            Err(DisbursementReasonCode::OraclePriceZero),
        );
    }

    #[test]
    fn grant_gate_propagates_release_cap_exceeded_from_evaluator() {
        // 900 ADA request against mid-band cap of 750 ADA denies.
        let mut intent = sample_intent();
        intent.requested_amount_lovelace = 900_000_000;
        assert_eq!(
            run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS),
            Err(DisbursementReasonCode::ReleaseCapExceeded),
        );
    }

    #[test]
    fn grant_gate_authorized_amount_equals_requested_amount() {
        // The evaluator must not silently reduce the request; the
        // authorized effect's amount equals the intent's requested
        // amount for every allow outcome.
        let mut intent = sample_intent();
        intent.requested_amount_lovelace = 500_000_000;
        let effect =
            run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS).expect("within cap");
        assert_eq!(effect.authorized_amount_lovelace, 500_000_000);
    }

    #[test]
    fn grant_gate_does_not_duplicate_cap_math() {
        // Structural: the grant gate depends only on evaluator outputs,
        // so the authorized band (release_cap_basis_points) must match
        // what the evaluator produced for the same intent. Two prices
        // and two bands, one test: high band vs. low band.
        let mut high = sample_intent();
        high.oracle_fact.price_microusd = 500_000;
        let eff_high = run_grant_gate(&high, DEMO_BASIS_LOVELACE, FRESH_NOW_MS)
            .expect("high band 100% allows 700m");
        assert_eq!(eff_high.release_cap_basis_points, 10_000);

        let mut low = sample_intent();
        low.oracle_fact.price_microusd = 100_000;
        low.requested_amount_lovelace = 400_000_000;
        let eff_low =
            run_grant_gate(&low, DEMO_BASIS_LOVELACE, FRESH_NOW_MS).expect("low band allows 400m");
        assert_eq!(eff_low.release_cap_basis_points, 5_000);
    }

    #[test]
    fn grant_gate_is_deterministic() {
        let intent = sample_intent();
        for _ in 0..4 {
            let a = run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS);
            let b = run_grant_gate(&intent, DEMO_BASIS_LOVELACE, FRESH_NOW_MS);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn grant_gate_failure_emits_reason_mapped_to_grant_gate() {
        // Every denial from run_grant_gate must be a reason whose
        // intrinsic .gate() is AuthorizationGate::Grant.
        for (intent, now) in [
            (sample_intent(), STALE_NOW_MS),
            ({ let mut i = sample_intent(); i.requested_amount_lovelace = 0; i }, FRESH_NOW_MS),
            ({ let mut i = sample_intent(); i.oracle_fact.price_microusd = 0; i }, FRESH_NOW_MS),
            ({ let mut i = sample_intent(); i.requested_amount_lovelace = 900_000_000; i }, FRESH_NOW_MS),
        ] {
            let reason = run_grant_gate(&intent, DEMO_BASIS_LOVELACE, now).unwrap_err();
            assert_eq!(reason.gate(), AuthorizationGate::Grant);
        }
    }

    // ---- Top-level composition (Cluster 5 Slice 05) ----

    fn ok_anchor() -> StaticAnchor<'static> {
        static REGISTERED: [[u8; 32]; 1] = [[0x11; 32]];
        StaticAnchor {
            registered: &REGISTERED,
        }
    }

    fn full_ok_registry() -> StaticRegistry<'static> {
        static SUBJECTS: [[u8; 32]; 1] = [[0x33; 32]];
        static DESTINATIONS: [CardanoAddressV1; 1] = [CardanoAddressV1 {
            bytes: [0x55; 57],
            length: 57,
        }];
        StaticRegistry {
            allocation_id: ALLOCATION_ID,
            basis_lovelace: DEMO_BASIS_LOVELACE,
            authorized_subjects: &SUBJECTS,
            approved_asset: AssetIdV1::ADA,
            approved_destinations: &DESTINATIONS,
        }
    }

    #[test]
    fn authorize_disbursement_allows_happy_path() {
        let r = authorize_disbursement(
            &sample_intent(),
            &ok_anchor(),
            &full_ok_registry(),
            FRESH_NOW_MS,
        );
        match r {
            AuthorizationResult::Authorized { effect } => {
                assert_eq!(effect.authorized_amount_lovelace, 700_000_000);
                assert_eq!(effect.release_cap_basis_points, 7_500);
            }
            AuthorizationResult::Denied { reason, gate } => {
                panic!("expected Authorized, got Denied {{ {reason:?}, {gate:?} }}");
            }
        }
    }

    #[test]
    fn authorize_disbursement_denies_at_anchor_with_unregistered_policy() {
        let anchor = StaticAnchor { registered: &[] };
        let r = authorize_disbursement(
            &sample_intent(),
            &anchor,
            &full_ok_registry(),
            FRESH_NOW_MS,
        );
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::PolicyNotFound,
                gate: AuthorizationGate::Anchor,
            },
        );
    }

    #[test]
    fn authorize_disbursement_denies_at_registry_with_missing_allocation() {
        let mut registry = full_ok_registry();
        registry.allocation_id = [0xDE; 32];
        let r = authorize_disbursement(&sample_intent(), &ok_anchor(), &registry, FRESH_NOW_MS);
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::AllocationNotFound,
                gate: AuthorizationGate::Registry,
            },
        );
    }

    #[test]
    fn authorize_disbursement_denies_at_grant_with_stale_oracle() {
        let r = authorize_disbursement(
            &sample_intent(),
            &ok_anchor(),
            &full_ok_registry(),
            STALE_NOW_MS,
        );
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::OracleStale,
                gate: AuthorizationGate::Grant,
            },
        );
    }

    #[test]
    fn authorize_disbursement_anchor_failure_precedes_registry_failure() {
        // Both anchor and registry are invalid; anchor must win.
        let anchor = StaticAnchor { registered: &[] };
        let mut registry = full_ok_registry();
        registry.allocation_id = [0xDE; 32];
        let r = authorize_disbursement(&sample_intent(), &anchor, &registry, FRESH_NOW_MS);
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::PolicyNotFound,
                gate: AuthorizationGate::Anchor,
            },
        );
    }

    #[test]
    fn authorize_disbursement_registry_failure_precedes_grant_failure() {
        // Registry invalid AND oracle stale; registry must win.
        let mut registry = full_ok_registry();
        registry.allocation_id = [0xDE; 32];
        let r = authorize_disbursement(&sample_intent(), &ok_anchor(), &registry, STALE_NOW_MS);
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::AllocationNotFound,
                gate: AuthorizationGate::Registry,
            },
        );
    }

    #[test]
    fn authorize_disbursement_anchor_failure_precedes_grant_failure() {
        // Anchor invalid AND oracle stale; anchor must win.
        let anchor = StaticAnchor { registered: &[] };
        let r = authorize_disbursement(
            &sample_intent(),
            &anchor,
            &full_ok_registry(),
            STALE_NOW_MS,
        );
        assert_eq!(
            r,
            AuthorizationResult::Denied {
                reason: DisbursementReasonCode::PolicyNotFound,
                gate: AuthorizationGate::Anchor,
            },
        );
    }

    #[test]
    fn authorize_disbursement_is_deterministic() {
        let intent = sample_intent();
        for _ in 0..4 {
            let a = authorize_disbursement(
                &intent,
                &ok_anchor(),
                &full_ok_registry(),
                FRESH_NOW_MS,
            );
            let b = authorize_disbursement(
                &intent,
                &ok_anchor(),
                &full_ok_registry(),
                FRESH_NOW_MS,
            );
            assert_eq!(a, b);
        }
    }

    #[test]
    fn authorize_disbursement_denied_gate_equals_reason_gate() {
        // Invariant: every Denied result has gate == reason.gate().
        let cases: &[(AuthorizationResult, DisbursementReasonCode)] = &[
            (
                authorize_disbursement(
                    &sample_intent(),
                    &StaticAnchor { registered: &[] },
                    &full_ok_registry(),
                    FRESH_NOW_MS,
                ),
                DisbursementReasonCode::PolicyNotFound,
            ),
            (
                {
                    let mut r = full_ok_registry();
                    r.allocation_id = [0xDE; 32];
                    authorize_disbursement(&sample_intent(), &ok_anchor(), &r, FRESH_NOW_MS)
                },
                DisbursementReasonCode::AllocationNotFound,
            ),
            (
                authorize_disbursement(
                    &sample_intent(),
                    &ok_anchor(),
                    &full_ok_registry(),
                    STALE_NOW_MS,
                ),
                DisbursementReasonCode::OracleStale,
            ),
        ];
        for (result, expected_reason) in cases {
            match result {
                AuthorizationResult::Denied { reason, gate } => {
                    assert_eq!(*reason, *expected_reason);
                    assert_eq!(*gate, reason.gate());
                }
                AuthorizationResult::Authorized { .. } => {
                    panic!("expected Denied result, got Authorized");
                }
            }
        }
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
