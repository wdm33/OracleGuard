//! Cluster 5 Slice 06 — Deny / no-authorized-effect closure proofs.
//!
//! Cluster 5 Slices 02–05 established the three-gate closure and
//! wired `authorize_disbursement`. This test file proves the closure
//! property that Slice 06 pins:
//!
//! > A denied authorization can never produce an authorized effect.
//! > An authorized result is the only path that carries an effect
//! > payload.
//!
//! The proof has two parts:
//!
//! 1. **Structural.** `AuthorizationResult::Denied` carries no effect
//!    field. Pattern-matching enforces this at compile time.
//! 2. **Behavioral.** Every reachable denial path emits `Denied` with
//!    a reason whose `.gate()` matches the returned `gate`; no path
//!    that denies ever returns `Authorized`. The allow path produces
//!    an `AuthorizedEffectV1` whose fields mirror the intent exactly
//!    and whose amount equals the requested amount.
//!
//! A byte-level golden fixture
//! (`fixtures/authorize/authorized_effect_golden.postcard`) pins the
//! canonical bytes of the demo authorized effect across runs.
//!
//! If any test here fails, Slice 06 is reopened.

use oracleguard_policy::authorize::{
    authorize_disbursement, AllocationFacts, AllocationRegistryView, AuthorizationResult,
    PolicyAnchorView,
};
use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::{DisbursementIntentV1, INTENT_VERSION_V1};
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};
use oracleguard_schemas::reason::DisbursementReasonCode;

// ---- Demo anchors (shared with Cluster 4 goldens) ----

const DEMO_POLICY_REF: [u8; 32] = [0x11; 32];
const DEMO_ALLOCATION_ID: [u8; 32] = [0x22; 32];
const DEMO_REQUESTER_ID: [u8; 32] = [0x33; 32];
const DEMO_BASIS_LOVELACE: u64 = 1_000_000_000;
const DEMO_PRICE_MICROUSD: u64 = 450_000;
const DEMO_REQUEST_LOVELACE: u64 = 700_000_000;
const DEMO_FRESH_NOW_MS: u64 = 1_713_000_000_000;
const DEMO_STALE_NOW_MS: u64 = 1_713_000_300_000;
const DEMO_EXPIRY_MS: u64 = 1_713_000_300_000;

const GOLDEN_AUTHORIZED_EFFECT: &[u8] =
    include_bytes!("../../../fixtures/authorize/authorized_effect_golden.postcard");

fn demo_destination() -> CardanoAddressV1 {
    CardanoAddressV1 {
        bytes: [0x55; 57],
        length: 57,
    }
}

fn demo_intent(requested_amount_lovelace: u64) -> DisbursementIntentV1 {
    DisbursementIntentV1 {
        intent_version: INTENT_VERSION_V1,
        policy_ref: DEMO_POLICY_REF,
        allocation_id: DEMO_ALLOCATION_ID,
        requester_id: DEMO_REQUESTER_ID,
        oracle_fact: OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: DEMO_PRICE_MICROUSD,
            source: SOURCE_CHARLI3,
        },
        oracle_provenance: OracleFactProvenanceV1 {
            timestamp_unix: 1_713_000_000_000,
            expiry_unix: DEMO_EXPIRY_MS,
            aggregator_utxo_ref: [0x44; 32],
        },
        requested_amount_lovelace,
        destination: demo_destination(),
        asset: AssetIdV1::ADA,
    }
}

// ---- Fixture traits ----

struct OneAnchor {
    registered: [u8; 32],
}
impl PolicyAnchorView for OneAnchor {
    fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool {
        *policy_ref == self.registered
    }
}

struct EmptyAnchor;
impl PolicyAnchorView for EmptyAnchor {
    fn is_policy_registered(&self, _policy_ref: &[u8; 32]) -> bool {
        false
    }
}

struct OneRegistry {
    allocation_id: [u8; 32],
    basis_lovelace: u64,
    approved_asset: AssetIdV1,
    authorized_subjects: [[u8; 32]; 1],
    approved_destinations: [CardanoAddressV1; 1],
}

impl OneRegistry {
    fn new() -> Self {
        Self {
            allocation_id: DEMO_ALLOCATION_ID,
            basis_lovelace: DEMO_BASIS_LOVELACE,
            approved_asset: AssetIdV1::ADA,
            authorized_subjects: [DEMO_REQUESTER_ID],
            approved_destinations: [demo_destination()],
        }
    }
}

impl AllocationRegistryView for OneRegistry {
    fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
        if *allocation_id == self.allocation_id {
            Some(AllocationFacts {
                basis_lovelace: self.basis_lovelace,
                authorized_subjects: &self.authorized_subjects,
                approved_asset: self.approved_asset,
                approved_destinations: &self.approved_destinations,
            })
        } else {
            None
        }
    }
}

struct EmptyRegistry;
impl AllocationRegistryView for EmptyRegistry {
    fn lookup(&self, _allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
        None
    }
}

// ---- Structural closure: Denied carries no effect field ----

// The `AuthorizationResult` enum's `Denied` variant has no effect
// field. This test is a pattern-match destructuring guard: if a
// future refactor silently adds an effect field to `Denied`, this
// match fails to compile.
#[test]
fn denied_variant_has_no_effect_field() {
    let denied = AuthorizationResult::Denied {
        reason: DisbursementReasonCode::ReleaseCapExceeded,
        gate: AuthorizationGate::Grant,
    };
    match denied {
        AuthorizationResult::Authorized { effect: _ } => {
            panic!("Denied did not match the Authorized arm — good; but we expected Denied")
        }
        AuthorizationResult::Denied { reason, gate } => {
            // Destructure: exactly two fields. Any future addition of
            // an effect-shaped field fails to compile here.
            assert_eq!(reason.gate(), gate);
        }
    }
}

// Symmetric pin: Authorized carries exactly one field — the effect.
// If a non-effect field is silently added, this destructuring fails.
#[test]
fn authorized_variant_has_only_effect_field() {
    let sample = authorized_demo_effect();
    let authorized = AuthorizationResult::Authorized { effect: sample };
    match authorized {
        AuthorizationResult::Authorized { effect } => {
            assert_eq!(effect, sample);
        }
        AuthorizationResult::Denied { .. } => {
            panic!("Authorized matched Denied arm")
        }
    }
}

// ---- Behavioral closure: every deny path yields no effect ----

// A table of (description, authorization inputs, expected reason, expected gate).
// Every non-grant reason is reachable through the orchestrator with
// appropriately rigged fixtures.
#[test]
fn every_reachable_deny_reason_emits_no_effect() {
    // Each case produces the named reason through authorize_disbursement
    // and asserts the returned AuthorizationResult is Denied (never
    // Authorized) with the expected reason-gate pair.

    // PolicyNotFound via unregistered policy
    assert_denied(
        &authorize_disbursement(
            &demo_intent(DEMO_REQUEST_LOVELACE),
            &EmptyAnchor,
            &OneRegistry::new(),
            DEMO_FRESH_NOW_MS,
        ),
        DisbursementReasonCode::PolicyNotFound,
        AuthorizationGate::Anchor,
    );

    // PolicyNotFound via non-v1 intent_version
    {
        let mut bad = demo_intent(DEMO_REQUEST_LOVELACE);
        bad.intent_version = 2;
        assert_denied(
            &authorize_disbursement(
                &bad,
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            ),
            DisbursementReasonCode::PolicyNotFound,
            AuthorizationGate::Anchor,
        );
    }

    // AllocationNotFound
    assert_denied(
        &authorize_disbursement(
            &demo_intent(DEMO_REQUEST_LOVELACE),
            &OneAnchor {
                registered: DEMO_POLICY_REF,
            },
            &EmptyRegistry,
            DEMO_FRESH_NOW_MS,
        ),
        DisbursementReasonCode::AllocationNotFound,
        AuthorizationGate::Registry,
    );

    // SubjectNotAuthorized via requester mismatch
    {
        let mut bad = demo_intent(DEMO_REQUEST_LOVELACE);
        bad.requester_id = [0x99; 32];
        assert_denied(
            &authorize_disbursement(
                &bad,
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            ),
            DisbursementReasonCode::SubjectNotAuthorized,
            AuthorizationGate::Registry,
        );
    }

    // SubjectNotAuthorized via destination mismatch (v1 mapping).
    {
        let mut bad = demo_intent(DEMO_REQUEST_LOVELACE);
        bad.destination = CardanoAddressV1 {
            bytes: [0x66; 57],
            length: 57,
        };
        assert_denied(
            &authorize_disbursement(
                &bad,
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            ),
            DisbursementReasonCode::SubjectNotAuthorized,
            AuthorizationGate::Registry,
        );
    }

    // AssetMismatch
    {
        let mut bad = demo_intent(DEMO_REQUEST_LOVELACE);
        bad.asset = AssetIdV1 {
            policy_id: [0xAB; 28],
            asset_name: [0u8; 32],
            asset_name_len: 0,
        };
        assert_denied(
            &authorize_disbursement(
                &bad,
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            ),
            DisbursementReasonCode::AssetMismatch,
            AuthorizationGate::Registry,
        );
    }

    // OracleStale
    assert_denied(
        &authorize_disbursement(
            &demo_intent(DEMO_REQUEST_LOVELACE),
            &OneAnchor {
                registered: DEMO_POLICY_REF,
            },
            &OneRegistry::new(),
            DEMO_STALE_NOW_MS,
        ),
        DisbursementReasonCode::OracleStale,
        AuthorizationGate::Grant,
    );

    // AmountZero
    assert_denied(
        &authorize_disbursement(
            &demo_intent(0),
            &OneAnchor {
                registered: DEMO_POLICY_REF,
            },
            &OneRegistry::new(),
            DEMO_FRESH_NOW_MS,
        ),
        DisbursementReasonCode::AmountZero,
        AuthorizationGate::Grant,
    );

    // OraclePriceZero
    {
        let mut bad = demo_intent(DEMO_REQUEST_LOVELACE);
        bad.oracle_fact.price_microusd = 0;
        assert_denied(
            &authorize_disbursement(
                &bad,
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            ),
            DisbursementReasonCode::OraclePriceZero,
            AuthorizationGate::Grant,
        );
    }

    // ReleaseCapExceeded
    assert_denied(
        &authorize_disbursement(
            &demo_intent(900_000_000),
            &OneAnchor {
                registered: DEMO_POLICY_REF,
            },
            &OneRegistry::new(),
            DEMO_FRESH_NOW_MS,
        ),
        DisbursementReasonCode::ReleaseCapExceeded,
        AuthorizationGate::Grant,
    );
}

fn assert_denied(
    r: &AuthorizationResult,
    expected_reason: DisbursementReasonCode,
    expected_gate: AuthorizationGate,
) {
    match r {
        AuthorizationResult::Authorized { effect } => {
            panic!(
                "expected Denied {{ {expected_reason:?}, {expected_gate:?} }}, \
                 got Authorized {{ amount={} band={} }}",
                effect.authorized_amount_lovelace, effect.release_cap_basis_points,
            );
        }
        AuthorizationResult::Denied { reason, gate } => {
            assert_eq!(
                *reason, expected_reason,
                "wrong reason for expected_gate={expected_gate:?}"
            );
            assert_eq!(
                *gate, expected_gate,
                "wrong gate for expected_reason={expected_reason:?}"
            );
            // Invariant: gate == reason.gate().
            assert_eq!(*gate, reason.gate(), "gate/reason-gate drift");
        }
    }
}

// ---- Behavioral closure: allow path produces an effect that mirrors the intent ----

fn authorized_demo_effect() -> AuthorizedEffectV1 {
    AuthorizedEffectV1 {
        policy_ref: DEMO_POLICY_REF,
        allocation_id: DEMO_ALLOCATION_ID,
        requester_id: DEMO_REQUESTER_ID,
        destination: demo_destination(),
        asset: AssetIdV1::ADA,
        authorized_amount_lovelace: DEMO_REQUEST_LOVELACE,
        release_cap_basis_points: 7_500,
    }
}

#[test]
fn allow_path_produces_effect_mirroring_intent() {
    let intent = demo_intent(DEMO_REQUEST_LOVELACE);
    let r = authorize_disbursement(
        &intent,
        &OneAnchor {
            registered: DEMO_POLICY_REF,
        },
        &OneRegistry::new(),
        DEMO_FRESH_NOW_MS,
    );
    match r {
        AuthorizationResult::Authorized { effect } => {
            assert_eq!(effect.policy_ref, intent.policy_ref);
            assert_eq!(effect.allocation_id, intent.allocation_id);
            assert_eq!(effect.requester_id, intent.requester_id);
            assert_eq!(effect.destination, intent.destination);
            assert_eq!(effect.asset, intent.asset);
            assert_eq!(
                effect.authorized_amount_lovelace,
                intent.requested_amount_lovelace,
            );
            assert_eq!(effect.release_cap_basis_points, 7_500);
        }
        AuthorizationResult::Denied { reason, gate } => {
            panic!("demo intent must allow, got Denied {{ {reason:?}, {gate:?} }}")
        }
    }
}

#[test]
fn allow_path_authorized_amount_equals_requested_amount_across_requests() {
    // Three within-cap requests. The evaluator never silently reduces.
    for req in [1_u64, 500_000_000, DEMO_REQUEST_LOVELACE, 750_000_000] {
        let r = authorize_disbursement(
            &demo_intent(req),
            &OneAnchor {
                registered: DEMO_POLICY_REF,
            },
            &OneRegistry::new(),
            DEMO_FRESH_NOW_MS,
        );
        match r {
            AuthorizationResult::Authorized { effect } => {
                assert_eq!(effect.authorized_amount_lovelace, req);
            }
            AuthorizationResult::Denied { reason, gate } => {
                panic!("req={req} must allow, got Denied {{ {reason:?}, {gate:?} }}")
            }
        }
    }
}

// ---- Golden fixture: AuthorizedEffectV1 canonical bytes ----

#[test]
fn authorized_effect_golden_matches_demo_authorization() {
    let r = authorize_disbursement(
        &demo_intent(DEMO_REQUEST_LOVELACE),
        &OneAnchor {
            registered: DEMO_POLICY_REF,
        },
        &OneRegistry::new(),
        DEMO_FRESH_NOW_MS,
    );
    let effect = match r {
        AuthorizationResult::Authorized { effect } => effect,
        AuthorizationResult::Denied { reason, gate } => {
            panic!("demo intent must allow, got Denied {{ {reason:?}, {gate:?} }}")
        }
    };
    let bytes = match postcard::to_allocvec(&effect) {
        Ok(b) => b,
        Err(e) => panic!("encode failed: {e:?}"),
    };
    assert_eq!(bytes.as_slice(), GOLDEN_AUTHORIZED_EFFECT);
}

#[test]
fn authorized_effect_golden_replay_is_byte_identical() {
    let runs: Vec<Vec<u8>> = (0..8)
        .map(|_| {
            let r = authorize_disbursement(
                &demo_intent(DEMO_REQUEST_LOVELACE),
                &OneAnchor {
                    registered: DEMO_POLICY_REF,
                },
                &OneRegistry::new(),
                DEMO_FRESH_NOW_MS,
            );
            let effect = match r {
                AuthorizationResult::Authorized { effect } => effect,
                AuthorizationResult::Denied { .. } => panic!("expected Authorized"),
            };
            match postcard::to_allocvec(&effect) {
                Ok(b) => b,
                Err(e) => panic!("encode failed: {e:?}"),
            }
        })
        .collect();
    for bytes in &runs {
        assert_eq!(bytes.as_slice(), GOLDEN_AUTHORIZED_EFFECT);
    }
    for window in runs.windows(2) {
        assert_eq!(window[0], window[1]);
    }
}

#[test]
fn authorized_effect_golden_decodes_back_to_same_effect() {
    let (decoded, remainder): (AuthorizedEffectV1, _) =
        match postcard::take_from_bytes(GOLDEN_AUTHORIZED_EFFECT) {
            Ok(v) => v,
            Err(e) => panic!("decode failed: {e:?}"),
        };
    assert!(remainder.is_empty(), "golden had trailing data");
    assert_eq!(decoded, authorized_demo_effect());
}

// ---- Manual fixture regeneration ----
//
// Run with:
//     cargo test -p oracleguard-policy --test closure_proofs -- \
//         --ignored regenerate
//
// This writes fresh bytes over
// `fixtures/authorize/authorized_effect_golden.postcard`. Do this only
// when the fixture shape itself changes deliberately; any commit that
// touches this fixture must explain why the canonical bytes moved.

#[test]
#[ignore = "regenerates golden fixture; run manually only"]
fn regenerate_authorized_effect_golden() {
    let effect = authorized_demo_effect();
    let bytes = match postcard::to_allocvec(&effect) {
        Ok(b) => b,
        Err(e) => panic!("encode failed: {e:?}"),
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/authorize/authorized_effect_golden.postcard"
    );
    std::fs::write(path, bytes).expect("write golden");
}
