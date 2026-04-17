//! Consensus output intake.
//!
//! Owns: the thin GREEN/RED boundary that reads already-authoritative
//! consensus output returned by the Ziranity CLI after a disbursement
//! intent has been processed. This module parses typed bytes; it does
//! not re-derive, re-validate, or reinterpret the authorization
//! decision.
//!
//! Does NOT own:
//!
//! - The authorization decision itself. That lives in
//!   [`oracleguard_policy::authorize::authorize_disbursement`] and has
//!   already produced the `AuthorizationResult` encoded in the bytes
//!   this module parses.
//! - Canonical byte identity of `AuthorizationResult`. That belongs to
//!   `oracleguard-policy`, which owns
//!   [`AuthorizationResult::decode`] / [`AuthorizationResult::encode`].
//! - Settlement execution. Cluster 7 consumes
//!   [`IntentReceiptV1::output`] as already-decided authoritative data.
//!
//! ## Reading rule
//!
//! Consumers of this module MUST treat the returned
//! [`AuthorizationResult`] as authoritative. They MUST NOT:
//!
//! - call `authorize_disbursement` on the intent after consensus
//! - recompute `release_cap_basis_points` from the oracle price
//! - re-validate destination or subject authorization
//! - translate `Denied { reason, gate }` into a different reason code
//!
//! This module structurally enforces the boundary: the parser does not
//! import any evaluator function, and the only typed value it produces
//! is the already-authoritative [`AuthorizationResult`] wrapped in a
//! receipt envelope.

use oracleguard_policy::authorize::{AuthorizationResult, AuthorizationResultDecodeError};

/// Status envelope surfacing the lifecycle state of a submitted intent.
///
/// Mirrors the Ziranity CLI's receipt surface (Implementation Plan
/// §14). OracleGuard consumes this as typed data; the semantic meaning
/// is authored upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntentStatus {
    /// Consensus reached and authorization evaluated. The
    /// [`IntentReceiptV1::output`] payload carries the decision.
    Committed,
    /// Consensus is still pending. No authorization decision is
    /// available yet; `output` is meaningless in this state and the
    /// caller must re-query before acting.
    Pending,
}

/// Typed consensus receipt returned by `ziranity intent submit`.
///
/// This is the intake-side view of the CLI's post-consensus output.
/// The authoritative decision lives in `output`; every other field is
/// provenance/lifecycle metadata that callers may log but MUST NOT
/// mix into the authorization decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentReceiptV1 {
    /// 32-byte BLAKE3 `intent_id` computed by the Ziranity CLI over
    /// the `ClientIntentV1` envelope. Matches the intent's canonical
    /// identity; used for cross-referencing evidence bundles.
    pub intent_id: [u8; 32],
    /// Lifecycle status — `Committed` once consensus has decided.
    pub status: IntentStatus,
    /// Block height at which consensus committed. `0` when `status`
    /// is [`IntentStatus::Pending`].
    pub committed_height: u64,
    /// The authoritative authorization result. Already decided by the
    /// three-gate closure upstream; OracleGuard reads this, it does
    /// not recompute it.
    pub output: AuthorizationResult,
}

/// Errors returned by [`parse_consensus_output`] and
/// [`parse_cli_receipt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntakeError {
    /// The underlying [`AuthorizationResult`] bytes could not be
    /// decoded. Either the CLI produced something other than a
    /// canonical authorization-result encoding, or the bytes were
    /// corrupted in transit.
    Decode(AuthorizationResultDecodeError),
    /// The CLI-receipt byte layout was malformed — wrong length,
    /// missing fields, or unknown status discriminant.
    MalformedReceipt,
}

impl From<AuthorizationResultDecodeError> for IntakeError {
    fn from(err: AuthorizationResultDecodeError) -> Self {
        IntakeError::Decode(err)
    }
}

/// Parse the canonical postcard bytes of a standalone
/// [`AuthorizationResult`] into typed form.
///
/// This is the lowest intake primitive. It is a strict
/// [`AuthorizationResult::decode`] call plus a typed error conversion
/// so callers have a single intake error surface.
pub fn parse_consensus_output(bytes: &[u8]) -> Result<AuthorizationResult, IntakeError> {
    Ok(AuthorizationResult::decode(bytes)?)
}

/// Parse the CLI-level receipt byte frame produced by
/// `ziranity intent submit --wait`.
///
/// The on-wire shape is: `intent_id (32) | status (1) | committed_height (8 LE) | output_len (4 LE) | output (output_len bytes)`.
///
/// This frame is adapter-owned intake glue, not a canonical semantic
/// wire type. The semantic authority is the embedded
/// `AuthorizationResult`, which is parsed via
/// [`parse_consensus_output`]. Changes to the frame layout are
/// operator-visible only and do not require a version-boundary event.
///
/// Status discriminants:
/// - `0x01` → [`IntentStatus::Committed`]
/// - `0x02` → [`IntentStatus::Pending`]
pub fn parse_cli_receipt(raw: &[u8]) -> Result<IntentReceiptV1, IntakeError> {
    const HEADER_LEN: usize = 32 + 1 + 8 + 4;
    if raw.len() < HEADER_LEN {
        return Err(IntakeError::MalformedReceipt);
    }
    let mut intent_id = [0u8; 32];
    intent_id.copy_from_slice(&raw[0..32]);

    let status = match raw[32] {
        0x01 => IntentStatus::Committed,
        0x02 => IntentStatus::Pending,
        _ => return Err(IntakeError::MalformedReceipt),
    };

    let mut committed_bytes = [0u8; 8];
    committed_bytes.copy_from_slice(&raw[33..41]);
    let committed_height = u64::from_le_bytes(committed_bytes);

    let mut output_len_bytes = [0u8; 4];
    output_len_bytes.copy_from_slice(&raw[41..45]);
    let output_len = u32::from_le_bytes(output_len_bytes) as usize;

    if raw.len() != HEADER_LEN + output_len {
        return Err(IntakeError::MalformedReceipt);
    }

    let output_bytes = &raw[HEADER_LEN..HEADER_LEN + output_len];
    let output = parse_consensus_output(output_bytes)?;

    Ok(IntentReceiptV1 {
        intent_id,
        status,
        committed_height,
        output,
    })
}

/// Render an [`IntentReceiptV1`] back into its CLI-frame byte form.
///
/// Inverse of [`parse_cli_receipt`]. Exposed so tests can pin the
/// round-trip and so private-side integration can produce the exact
/// bytes OracleGuard parses without copying byte-layout knowledge.
pub fn render_cli_receipt(receipt: &IntentReceiptV1) -> Result<Vec<u8>, IntakeError> {
    let output_bytes = receipt.output.encode()?;
    let output_len =
        u32::try_from(output_bytes.len()).map_err(|_| IntakeError::MalformedReceipt)?;
    let mut out = Vec::with_capacity(32 + 1 + 8 + 4 + output_bytes.len());
    out.extend_from_slice(&receipt.intent_id);
    out.push(match receipt.status {
        IntentStatus::Committed => 0x01,
        IntentStatus::Pending => 0x02,
    });
    out.extend_from_slice(&receipt.committed_height.to_le_bytes());
    out.extend_from_slice(&output_len.to_le_bytes());
    out.extend_from_slice(&output_bytes);
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_policy::authorize::AuthorizationResult;
    use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
    use oracleguard_schemas::gate::AuthorizationGate;
    use oracleguard_schemas::reason::DisbursementReasonCode;

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

    fn sample_authorized() -> AuthorizationResult {
        AuthorizationResult::Authorized {
            effect: sample_effect(),
        }
    }

    fn sample_denied() -> AuthorizationResult {
        AuthorizationResult::Denied {
            reason: DisbursementReasonCode::OracleStale,
            gate: AuthorizationGate::Grant,
        }
    }

    #[test]
    fn parse_consensus_output_decodes_authorized() {
        let bytes = sample_authorized().encode().expect("encode");
        let parsed = parse_consensus_output(&bytes).expect("decode");
        assert_eq!(parsed, sample_authorized());
    }

    #[test]
    fn parse_consensus_output_decodes_denied() {
        let bytes = sample_denied().encode().expect("encode");
        let parsed = parse_consensus_output(&bytes).expect("decode");
        assert_eq!(parsed, sample_denied());
    }

    #[test]
    fn parse_consensus_output_rejects_trailing_bytes() {
        let mut bytes = sample_authorized().encode().expect("encode");
        bytes.push(0x00);
        let err = parse_consensus_output(&bytes).expect_err("must reject");
        assert_eq!(
            err,
            IntakeError::Decode(AuthorizationResultDecodeError::TrailingBytes)
        );
    }

    #[test]
    fn parse_consensus_output_rejects_empty_input() {
        let err = parse_consensus_output(&[]).expect_err("must reject");
        assert_eq!(
            err,
            IntakeError::Decode(AuthorizationResultDecodeError::Malformed)
        );
    }

    #[test]
    fn cli_receipt_round_trip_committed_authorized() {
        let receipt = IntentReceiptV1 {
            intent_id: [0xAA; 32],
            status: IntentStatus::Committed,
            committed_height: 42,
            output: sample_authorized(),
        };
        let bytes = render_cli_receipt(&receipt).expect("render");
        let parsed = parse_cli_receipt(&bytes).expect("parse");
        assert_eq!(parsed, receipt);
    }

    #[test]
    fn cli_receipt_round_trip_committed_denied() {
        let receipt = IntentReceiptV1 {
            intent_id: [0xBB; 32],
            status: IntentStatus::Committed,
            committed_height: 1_234_567,
            output: sample_denied(),
        };
        let bytes = render_cli_receipt(&receipt).expect("render");
        let parsed = parse_cli_receipt(&bytes).expect("parse");
        assert_eq!(parsed, receipt);
    }

    #[test]
    fn cli_receipt_round_trip_pending() {
        let receipt = IntentReceiptV1 {
            intent_id: [0xCC; 32],
            status: IntentStatus::Pending,
            committed_height: 0,
            output: sample_denied(),
        };
        let bytes = render_cli_receipt(&receipt).expect("render");
        let parsed = parse_cli_receipt(&bytes).expect("parse");
        assert_eq!(parsed, receipt);
    }

    #[test]
    fn parse_cli_receipt_rejects_truncated_header() {
        let bytes = [0u8; 40];
        let err = parse_cli_receipt(&bytes).expect_err("must reject");
        assert_eq!(err, IntakeError::MalformedReceipt);
    }

    #[test]
    fn parse_cli_receipt_rejects_unknown_status() {
        let receipt = IntentReceiptV1 {
            intent_id: [0xDD; 32],
            status: IntentStatus::Committed,
            committed_height: 7,
            output: sample_authorized(),
        };
        let mut bytes = render_cli_receipt(&receipt).expect("render");
        bytes[32] = 0xFF; // corrupt status discriminant
        let err = parse_cli_receipt(&bytes).expect_err("must reject");
        assert_eq!(err, IntakeError::MalformedReceipt);
    }

    #[test]
    fn parse_cli_receipt_rejects_output_length_mismatch() {
        let receipt = IntentReceiptV1 {
            intent_id: [0xEE; 32],
            status: IntentStatus::Committed,
            committed_height: 7,
            output: sample_authorized(),
        };
        let mut bytes = render_cli_receipt(&receipt).expect("render");
        bytes.push(0x00); // append one trailing byte — length field now lies
        let err = parse_cli_receipt(&bytes).expect_err("must reject");
        assert_eq!(err, IntakeError::MalformedReceipt);
    }

    #[test]
    fn parse_cli_receipt_rejects_inner_trailing_bytes() {
        // Length header says 1 byte beyond actual; embedded
        // AuthorizationResult would decode but with trailing bytes.
        let receipt = IntentReceiptV1 {
            intent_id: [0xFF; 32],
            status: IntentStatus::Committed,
            committed_height: 7,
            output: sample_denied(),
        };
        let mut bytes = render_cli_receipt(&receipt).expect("render");
        // Bump the embedded output_len by +1 and append a stray byte
        // at the tail so the outer length matches. The inner decoder
        // then sees a trailing-byte violation.
        let output_bytes_start = 32 + 1 + 8 + 4;
        let output_len_pos = 32 + 1 + 8;
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&bytes[output_len_pos..output_len_pos + 4]);
        let new_len = u32::from_le_bytes(len_bytes) + 1;
        bytes[output_len_pos..output_len_pos + 4].copy_from_slice(&new_len.to_le_bytes());
        bytes.push(0x00);
        // Sanity: start index still valid
        assert!(bytes.len() > output_bytes_start);
        let err = parse_cli_receipt(&bytes).expect_err("must reject");
        // Inner trailing-byte maps to Decode::TrailingBytes since the
        // inner decoder rejects the appended null.
        assert_eq!(
            err,
            IntakeError::Decode(AuthorizationResultDecodeError::TrailingBytes)
        );
    }

    #[test]
    fn parse_cli_receipt_is_stable_across_calls() {
        let receipt = IntentReceiptV1 {
            intent_id: [0x77; 32],
            status: IntentStatus::Committed,
            committed_height: 99,
            output: sample_authorized(),
        };
        let bytes = render_cli_receipt(&receipt).expect("render");
        let a = parse_cli_receipt(&bytes).expect("a");
        let b = parse_cli_receipt(&bytes).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn intake_does_not_recompute_authorization() {
        // Structural / audit test: the intake module's non-test source
        // must not call any evaluator or authorization-orchestrator
        // symbol. This pins the Cluster 6 Slice 05 "no reinterpretation"
        // promise mechanically.
        //
        // We split the source at `#[cfg(test)]` and inspect only the
        // production half, so the test's own prose and assertions
        // don't trigger self-matches.
        const THIS_FILE: &str = include_str!("intake.rs");
        // Built from pieces so the literal doesn't match itself.
        let split_marker = format!("{}[cfg(test)]", '#');
        let (prod, _) = THIS_FILE
            .split_once(split_marker.as_str())
            .expect("production/test split marker must exist");
        // Build forbidden patterns at runtime so the literals don't
        // themselves live in the file.
        let call = |s: &str| format!("{s}(");
        for sym in [
            "authorize_disbursement",
            "evaluate_disbursement",
            "run_anchor_gate",
            "run_registry_gate",
            "run_grant_gate",
        ] {
            let pat = call(sym);
            assert!(
                !prod.contains(&pat),
                "intake production source must not call {sym}"
            );
        }
    }
}
