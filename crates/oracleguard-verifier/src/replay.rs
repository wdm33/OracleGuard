//! Evaluator replay.
//!
//! Owns: deterministically re-running
//! [`oracleguard_policy::evaluate::evaluate_disbursement`] over the
//! canonical inputs recorded in an evidence bundle and comparing the
//! result to the evaluator projection of the recorded authorization
//! snapshot.
//!
//! Does NOT own: the evaluator itself (policy crate), nor the
//! canonical decision type (schemas). This module only *compares*.
//!
//! ## Replay scope
//!
//! The public evaluator is a pure function of
//! `(DisbursementIntentV1, allocation_basis_lovelace)`. Replay is
//! therefore defined only for authorization snapshots whose outcome
//! the evaluator actually produced:
//!
//! - `Authorized { effect }` — the grant gate delegated to the
//!   evaluator and received
//!   `Allow { authorized_amount_lovelace, release_cap_basis_points }`.
//!   Replay compares `evaluate_disbursement` against
//!   `Allow { auth_amt: effect.authorized_amount_lovelace, bps:
//!   effect.release_cap_basis_points }`.
//! - `Denied { reason }` where `reason` is one of the grant-gate
//!   evaluator-produced reasons (`AmountZero`, `OraclePriceZero`,
//!   `ReleaseCapExceeded`). Replay compares against
//!   `Deny { reason }`.
//!
//! Other deny reasons (`PolicyNotFound`, `AllocationNotFound`,
//! `SubjectNotAuthorized`, `AssetMismatch`, `OracleStale`) come from
//! gates that run before the evaluator. The evaluator was never
//! called in those cases, so replay is a no-op — the verifier does
//! not invent a mismatch it cannot reproduce.

use oracleguard_policy::error::EvaluationResult;
use oracleguard_policy::evaluate::evaluate_disbursement;
use oracleguard_schemas::evidence::{AuthorizationSnapshotV1, DisbursementEvidenceV1};
use oracleguard_schemas::intent::INTENT_VERSION_V1;
use oracleguard_schemas::reason::DisbursementReasonCode;

use crate::report::{VerifierFinding, VerifierReport};

/// Evaluator-side projection of an authorization snapshot, when one
/// exists.
///
/// Returns `Some(expected)` when the evaluator definitely ran;
/// `None` when the snapshot's `Denied` variant names a pre-evaluator
/// gate failure and replay has nothing to compare against.
fn expected_from_snapshot(snapshot: &AuthorizationSnapshotV1) -> Option<EvaluationResult> {
    match snapshot {
        AuthorizationSnapshotV1::Authorized { effect } => Some(EvaluationResult::Allow {
            authorized_amount_lovelace: effect.authorized_amount_lovelace,
            release_cap_basis_points: effect.release_cap_basis_points,
        }),
        AuthorizationSnapshotV1::Denied { reason, .. } => match reason {
            DisbursementReasonCode::AmountZero
            | DisbursementReasonCode::OraclePriceZero
            | DisbursementReasonCode::ReleaseCapExceeded => {
                Some(EvaluationResult::Deny { reason: *reason })
            }
            DisbursementReasonCode::PolicyNotFound
            | DisbursementReasonCode::AllocationNotFound
            | DisbursementReasonCode::SubjectNotAuthorized
            | DisbursementReasonCode::AssetMismatch
            | DisbursementReasonCode::OracleStale => None,
        },
    }
}

/// Re-run the pure evaluator over the canonical inputs in `evidence`
/// and append a [`VerifierFinding::ReplayDiverged`] finding to
/// `report` if the result disagrees with the evaluator projection of
/// the recorded authorization snapshot.
///
/// Determinism: the evaluator is pure; repeated calls on identical
/// inputs produce byte-identical `EvaluationResult` values. Any
/// divergence is a bundle-tampering or crate-regression signal.
pub fn check_replay_equivalence(evidence: &DisbursementEvidenceV1, report: &mut VerifierReport) {
    // The evaluator has a v1-intent precondition (the anchor gate
    // rejects off-version intents in production). Skipping replay
    // when the intent is not v1 keeps the verifier from panicking on
    // malformed bundles; the version mismatch is already surfaced by
    // `check_integrity`.
    if evidence.intent.intent_version != INTENT_VERSION_V1 {
        return;
    }
    let expected = match expected_from_snapshot(&evidence.authorization) {
        Some(e) => e,
        None => return, // Pre-evaluator gate failure — replay is a no-op.
    };
    let actual = evaluate_disbursement(&evidence.intent, evidence.allocation_basis_lovelace);
    if actual != expected {
        report.push(VerifierFinding::ReplayDiverged { expected, actual });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use oracleguard_schemas::gate::AuthorizationGate;

    use crate::bundle::load_bundle;

    fn fixture(name: &str) -> DisbursementEvidenceV1 {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("../../fixtures/evidence");
        p.push(name);
        load_bundle(&p).expect("load fixture")
    }

    fn run(evidence: &DisbursementEvidenceV1) -> VerifierReport {
        let mut report = VerifierReport::new();
        check_replay_equivalence(evidence, &mut report);
        report
    }

    #[test]
    fn allow_fixture_replay_matches() {
        let report = run(&fixture("allow_700_ada_bundle.postcard"));
        assert!(
            report.is_ok(),
            "allow replay diverged: {:?}",
            report.findings
        );
    }

    #[test]
    fn deny_fixture_replay_matches() {
        // ReleaseCapExceeded is a grant-gate evaluator-produced
        // reason; replay is defined and should match.
        let report = run(&fixture("deny_900_ada_bundle.postcard"));
        assert!(
            report.is_ok(),
            "deny replay diverged: {:?}",
            report.findings
        );
    }

    #[test]
    fn refusal_fixtures_replay_matches_or_is_noop() {
        // Both refusal bundles carry Authorized snapshots, so replay
        // runs against Allow projections. They must match because the
        // snapshots are byte-identical to what the evaluator produced
        // at assembly time.
        for name in [
            "reject_non_ada_bundle.postcard",
            "reject_pending_bundle.postcard",
        ] {
            let report = run(&fixture(name));
            assert!(
                report.is_ok(),
                "refusal bundle {name} replay diverged: {:?}",
                report.findings
            );
        }
    }

    #[test]
    fn replay_flags_divergence_when_amount_mutated() {
        // Construct a bundle whose Authorized effect claims a
        // different amount than the evaluator would return. The
        // authorization-consistency check (integrity.rs) also flags
        // this, but replay independently proves the evaluator was
        // consulted.
        let mut evidence = fixture("allow_700_ada_bundle.postcard");
        if let AuthorizationSnapshotV1::Authorized { ref mut effect } = evidence.authorization {
            effect.authorized_amount_lovelace = 1;
        }
        let report = run(&evidence);
        assert!(
            report
                .findings
                .iter()
                .any(|f| matches!(f, VerifierFinding::ReplayDiverged { .. })),
            "missing ReplayDiverged finding: {:?}",
            report.findings
        );
    }

    #[test]
    fn replay_is_noop_for_pre_evaluator_gate_denials() {
        // Evidence where authorization snapshot denies with a
        // pre-evaluator gate reason (PolicyNotFound / AllocationNotFound
        // / SubjectNotAuthorized / AssetMismatch / OracleStale) — the
        // evaluator was never called, so replay has nothing to
        // compare. We fabricate a synthetic PolicyNotFound bundle
        // from the deny fixture and assert replay contributes no
        // findings even though the bundle's data doesn't match what
        // the evaluator would produce.
        let mut evidence = fixture("deny_900_ada_bundle.postcard");
        evidence.authorization = AuthorizationSnapshotV1::Denied {
            reason: DisbursementReasonCode::PolicyNotFound,
            gate: AuthorizationGate::Anchor,
        };
        let report = run(&evidence);
        assert!(
            report.is_ok(),
            "pre-evaluator deny should yield no replay findings: {:?}",
            report.findings
        );
    }

    #[test]
    fn replay_is_deterministic_across_calls() {
        let evidence = fixture("allow_700_ada_bundle.postcard");
        let a = run(&evidence);
        let b = run(&evidence);
        assert_eq!(a, b);
    }
}
