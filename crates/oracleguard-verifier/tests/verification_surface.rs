//! Cluster 8 Slice 06 — verification-surface integration suite.
//!
//! This file is the integration-scope counterpart to the unit tests
//! inside `oracleguard-verifier`. Where the unit tests pin a single
//! check at a time, this suite exercises the full
//! `verify_bundle` / `verify_evidence` pipeline across every MVP
//! fixture and proves three architectural claims mechanically:
//!
//! 1. **Public determinism.** Repeated verification of the same
//!    bundle yields byte-identical reports.
//! 2. **Replay equivalence.** The public evaluator reproduces the
//!    recorded authorization snapshot's evaluator projection for
//!    every fixture that drove the evaluator.
//! 3. **Tamper detection.** Mutating any authoritative field of a
//!    clean bundle produces at least one typed verifier finding —
//!    no silent acceptance of corrupted evidence.
//!
//! These assertions sit alongside the adapter's
//! `tests/fulfillment_boundary.rs` and
//! `tests/routing_boundary.rs` integration suites so reviewers have
//! one place per cluster to point when auditing the mechanical-proof
//! claim.

use std::path::{Path, PathBuf};

use oracleguard_policy::error::EvaluationResult;
use oracleguard_policy::evaluate::evaluate_disbursement;
use oracleguard_schemas::encoding::intent_id;
use oracleguard_schemas::evidence::{
    decode_evidence, encode_evidence, AuthorizationSnapshotV1, DisbursementEvidenceV1,
    ExecutionOutcomeV1, EVIDENCE_VERSION_V1,
};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::INTENT_VERSION_V1;
use oracleguard_schemas::reason::DisbursementReasonCode;
use oracleguard_verifier::{
    verify_bundle, verify_evidence, AuthorizedEffectField, BundleLoadError, VerifierFinding,
    VerifierReport,
};

const MVP_FIXTURES: &[&str] = &[
    "allow_700_ada_bundle.postcard",
    "deny_900_ada_bundle.postcard",
    "reject_non_ada_bundle.postcard",
    "reject_pending_bundle.postcard",
];

fn fixture_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../fixtures/evidence");
    p.push(name);
    p
}

fn load_fixture(name: &str) -> DisbursementEvidenceV1 {
    let path = fixture_path(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {name}: {e}"));
    decode_evidence(&bytes).unwrap_or_else(|e| panic!("decode {name}: {e:?}"))
}

// -------------------------------------------------------------------
// Clean fixtures — every MVP bundle verifies without findings.
// -------------------------------------------------------------------

#[test]
fn every_mvp_fixture_verifies_clean() {
    for name in MVP_FIXTURES {
        let report = verify_bundle(&fixture_path(name)).expect("verify");
        assert!(
            report.is_ok(),
            "fixture {name} produced findings: {:?}",
            report.findings
        );
    }
}

#[test]
fn every_mvp_fixture_has_version_pin() {
    for name in MVP_FIXTURES {
        let evidence = load_fixture(name);
        assert_eq!(evidence.evidence_version, EVIDENCE_VERSION_V1);
        assert_eq!(evidence.intent.intent_version, INTENT_VERSION_V1);
    }
}

// -------------------------------------------------------------------
// Determinism — public reports are byte-identically reproducible.
// -------------------------------------------------------------------

#[test]
fn verify_bundle_is_byte_identical_across_runs() {
    for name in MVP_FIXTURES {
        let path = fixture_path(name);
        let mut reports = Vec::new();
        for _ in 0..8 {
            reports.push(verify_bundle(&path).expect("verify"));
        }
        let first = &reports[0];
        for (i, r) in reports.iter().enumerate().skip(1) {
            assert_eq!(
                r, first,
                "run {i} diverged for {name}: {:?} vs {:?}",
                r, first
            );
        }
    }
}

#[test]
fn verify_evidence_matches_verify_bundle_for_every_fixture() {
    for name in MVP_FIXTURES {
        let path = fixture_path(name);
        let from_bundle = verify_bundle(&path).expect("bundle");
        let evidence = load_fixture(name);
        let from_value = verify_evidence(&evidence);
        assert_eq!(from_bundle, from_value, "surface mismatch for {name}");
    }
}

#[test]
fn re_encoding_every_fixture_round_trips_byte_identically() {
    // Pinning the fixtures' canonical byte surface against the
    // schemas encoder proves the bundles on disk are not drifting
    // away from what the current code produces.
    for name in MVP_FIXTURES {
        let path = fixture_path(name);
        let original = std::fs::read(&path).expect("read");
        let evidence = decode_evidence(&original).expect("decode");
        let re_encoded = encode_evidence(&evidence).expect("encode");
        assert_eq!(original, re_encoded, "byte drift on {name}");
    }
}

// -------------------------------------------------------------------
// Replay — pure evaluator reproduces the recorded snapshot.
// -------------------------------------------------------------------

#[test]
fn public_evaluator_reproduces_allow_fixture() {
    let evidence = load_fixture("allow_700_ada_bundle.postcard");
    let result = evaluate_disbursement(&evidence.intent, evidence.allocation_basis_lovelace);
    match (result, evidence.authorization) {
        (
            EvaluationResult::Allow {
                authorized_amount_lovelace,
                release_cap_basis_points,
            },
            AuthorizationSnapshotV1::Authorized { effect },
        ) => {
            assert_eq!(
                authorized_amount_lovelace,
                effect.authorized_amount_lovelace
            );
            assert_eq!(release_cap_basis_points, effect.release_cap_basis_points);
        }
        other => panic!("unexpected pair: {other:?}"),
    }
}

#[test]
fn public_evaluator_reproduces_deny_fixture() {
    let evidence = load_fixture("deny_900_ada_bundle.postcard");
    let result = evaluate_disbursement(&evidence.intent, evidence.allocation_basis_lovelace);
    match (result, evidence.authorization) {
        (
            EvaluationResult::Deny { reason: r_actual },
            AuthorizationSnapshotV1::Denied {
                reason: r_recorded, ..
            },
        ) => {
            assert_eq!(r_actual, r_recorded);
            assert_eq!(r_actual, DisbursementReasonCode::ReleaseCapExceeded);
        }
        other => panic!("unexpected pair: {other:?}"),
    }
}

#[test]
fn recorded_intent_id_equals_recomputed_intent_id_for_every_fixture() {
    for name in MVP_FIXTURES {
        let evidence = load_fixture(name);
        let recomputed = intent_id(&evidence.intent).expect("intent id");
        assert_eq!(evidence.intent_id, recomputed, "intent_id drift on {name}");
    }
}

// -------------------------------------------------------------------
// Tamper sweep — mutating any authoritative field flags findings.
// -------------------------------------------------------------------

fn assert_not_ok(report: &VerifierReport, label: &str) {
    assert!(
        !report.is_ok(),
        "mutation {label} produced no findings: {:?}",
        report.findings
    );
}

#[test]
fn mutating_intent_id_produces_findings() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.intent_id = [0xFF; 32];
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "intent_id flip");
    assert!(report
        .findings
        .iter()
        .any(|f| matches!(f, VerifierFinding::IntentIdMismatch { .. })));
}

#[test]
fn mutating_evidence_version_produces_findings() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.evidence_version = 42;
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "evidence_version flip");
    assert!(report
        .findings
        .iter()
        .any(|f| matches!(f, VerifierFinding::EvidenceVersionUnsupported { .. })));
}

#[test]
fn mutating_intent_version_produces_findings() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.intent.intent_version = 42;
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "intent_version flip");
    assert!(report
        .findings
        .iter()
        .any(|f| matches!(f, VerifierFinding::IntentVersionUnsupported { .. })));
}

#[test]
fn mutating_intent_policy_ref_produces_findings() {
    // Mutating the intent's policy_ref breaks intent_id (the id in
    // the record no longer matches the recomputed id) AND the
    // authorized-effect consistency (effect.policy_ref no longer
    // matches intent.policy_ref). Both findings must appear.
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.intent.policy_ref = [0x00; 32];
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "intent.policy_ref flip");
    assert!(report
        .findings
        .iter()
        .any(|f| matches!(f, VerifierFinding::IntentIdMismatch { .. })));
    assert!(report.findings.iter().any(|f| matches!(
        f,
        VerifierFinding::AuthorizedEffectMismatch {
            field: AuthorizedEffectField::PolicyRef,
        }
    )));
}

#[test]
fn mutating_intent_requested_amount_produces_findings() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.intent.requested_amount_lovelace = 1;
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "intent.requested_amount flip");
    assert!(
        report
            .findings
            .iter()
            .any(|f| matches!(f, VerifierFinding::IntentIdMismatch { .. })),
        "mutation must break intent_id recomputation"
    );
}

#[test]
fn mutating_authorized_effect_amount_produces_findings() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    if let AuthorizationSnapshotV1::Authorized { ref mut effect } = evidence.authorization {
        effect.authorized_amount_lovelace = 1;
    }
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "effect.authorized_amount flip");
    // Both the authorized-effect byte-identity check and the
    // replay-equivalence check must flag this mutation.
    assert!(report.findings.iter().any(|f| matches!(
        f,
        VerifierFinding::AuthorizedEffectMismatch {
            field: AuthorizedEffectField::AuthorizedAmountLovelace,
        }
    )));
    assert!(report
        .findings
        .iter()
        .any(|f| matches!(f, VerifierFinding::ReplayDiverged { .. })));
}

#[test]
fn mutating_tx_hash_produces_bundle_byte_drift_but_no_verifier_finding() {
    // tx_hash is recorded provenance — the verifier does not
    // cross-check it against any other bundle field because the
    // Cardano chain is out of scope. But re-encoding with a mutated
    // tx_hash must produce different canonical bytes, so CI golden
    // tests still catch tampering at the fixture level.
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    let original_bytes = encode_evidence(&evidence).expect("encode");
    evidence.execution = ExecutionOutcomeV1::Settled {
        tx_hash: [0xEE; 32],
    };
    let mutated_bytes = encode_evidence(&evidence).expect("encode");
    assert_ne!(original_bytes, mutated_bytes);
    // Integrity + replay pass because tx_hash doesn't feed either.
    let report = verify_evidence(&evidence);
    assert!(
        report.is_ok(),
        "tx_hash should not flag: {:?}",
        report.findings
    );
}

#[test]
fn switching_execution_variant_to_denied_on_allow_fixture_flags_inconsistency() {
    let mut evidence = load_fixture("allow_700_ada_bundle.postcard");
    evidence.execution = ExecutionOutcomeV1::DeniedUpstream {
        reason: DisbursementReasonCode::ReleaseCapExceeded,
        gate: AuthorizationGate::Grant,
    };
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "Settled → DeniedUpstream");
    assert!(report.findings.iter().any(|f| matches!(
        f,
        VerifierFinding::AuthorizationExecutionInconsistent { .. }
    )));
}

#[test]
fn switching_execution_variant_to_settled_on_deny_fixture_flags_inconsistency() {
    let mut evidence = load_fixture("deny_900_ada_bundle.postcard");
    evidence.execution = ExecutionOutcomeV1::Settled {
        tx_hash: [0xAB; 32],
    };
    let report = verify_evidence(&evidence);
    assert_not_ok(&report, "DeniedUpstream → Settled");
    assert!(report.findings.iter().any(|f| matches!(
        f,
        VerifierFinding::AuthorizationExecutionInconsistent { .. }
    )));
}

// -------------------------------------------------------------------
// Error surfaces — malformed input never panics.
// -------------------------------------------------------------------

#[test]
fn verify_bundle_surfaces_io_error_for_missing_paths() {
    let missing: &Path = Path::new("/tmp/oracleguard/absolutely-not-a-real-bundle.postcard");
    match verify_bundle(missing) {
        Err(BundleLoadError::Io(_)) => {}
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn verify_bundle_surfaces_decode_error_for_garbage_bytes() {
    use std::fs;
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../target/tmp-garbage-bundle.postcard");
    // Write garbage; read back via verify_bundle.
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(&p, b"not an evidence bundle").expect("write garbage");
    match verify_bundle(&p) {
        Err(BundleLoadError::Decode(_)) => {}
        other => panic!("expected Decode, got {other:?}"),
    }
    fs::remove_file(&p).ok();
}

// -------------------------------------------------------------------
// Cross-fixture consistency — allow and deny bundles share
// policy/allocation/requester identity.
// -------------------------------------------------------------------

#[test]
fn allow_and_deny_fixtures_share_policy_allocation_requester_and_oracle_fact() {
    let allow = load_fixture("allow_700_ada_bundle.postcard");
    let deny = load_fixture("deny_900_ada_bundle.postcard");
    assert_eq!(allow.intent.policy_ref, deny.intent.policy_ref);
    assert_eq!(allow.intent.allocation_id, deny.intent.allocation_id);
    assert_eq!(allow.intent.requester_id, deny.intent.requester_id);
    assert_eq!(allow.intent.oracle_fact, deny.intent.oracle_fact);
    assert_eq!(allow.intent.destination, deny.intent.destination);
    assert_eq!(allow.intent.asset, deny.intent.asset);
    assert_ne!(
        allow.intent.requested_amount_lovelace,
        deny.intent.requested_amount_lovelace
    );
}
