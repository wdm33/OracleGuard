//! Ziranity CLI submission shell.
//!
//! Owns: the thin RED boundary that moves canonical
//! `DisbursementIntentV1` payload bytes into the Ziranity CLI's
//! submission surface. This module is a shell wrapper; it does not
//! define semantic meaning, envelope framing, or transport protocol.
//!
//! Does NOT own (boundaries):
//!
//! - **Canonical payload bytes.** Produced by
//!   `oracleguard_schemas::encoding::emit_payload` (Cluster 6 Slice 03
//!   named alias of `encode_intent`). This module treats the payload as
//!   opaque bytes once emitted.
//! - **Envelope construction / signing.** The Ziranity CLI builds the
//!   `ClientIntentV1` envelope, signs it with the configured Ed25519
//!   operator key, computes the BLAKE3 `intent_id`, and drives the
//!   MCP transport protocol (BindObject → BindAck → SubmitIntent →
//!   SubmitAck).
//! - **Authority derivation.** The routing target is the fixed
//!   `oracleguard_schemas::routing::DISBURSEMENT_OCU_ID` constant. See
//!   `docs/authority-routing.md`.
//! - **Authorization.** The three-gate closure
//!   (`oracleguard_policy::authorize::authorize_disbursement`) decides
//!   allow/deny upstream of this module.
//!
//! ## Payload-only boundary
//!
//! The submission boundary OracleGuard owns is exactly these bytes:
//!
//! ```text
//! emit_payload(&intent) → canonical DisbursementIntentV1 postcard bytes
//! ```
//!
//! Everything else — envelope, signature, MCP framing, consensus
//! projection — is Ziranity-side. If submission glue in this module
//! ever starts wrapping the payload in a private structure, signing
//! it locally, or redefining canonical bytes, the boundary has been
//! violated.
//!
//! Cluster 6 Slice 04 extends this module with the CLI argument
//! construction wrapper; this slice (03) lands only the payload-byte
//! boundary and the disk-write helper the wrapper consumes.

use std::io;
use std::path::Path;

use oracleguard_schemas::encoding::{emit_payload, CanonicalEncodeError};
use oracleguard_schemas::intent::DisbursementIntentV1;

/// Error returned by [`write_payload_to_file`].
#[derive(Debug)]
pub enum WritePayloadError {
    /// The canonical encoder rejected the intent. Should not happen
    /// for well-formed fixed-width intents; surfaced rather than
    /// panicked so callers can log and re-try or escalate.
    Encode(CanonicalEncodeError),
    /// Filesystem write failed. The payload bytes were produced
    /// successfully but could not be persisted.
    Io(io::Error),
}

impl From<CanonicalEncodeError> for WritePayloadError {
    fn from(err: CanonicalEncodeError) -> Self {
        WritePayloadError::Encode(err)
    }
}

impl From<io::Error> for WritePayloadError {
    fn from(err: io::Error) -> Self {
        WritePayloadError::Io(err)
    }
}

/// Emit the canonical payload bytes for `intent` and write them to
/// `path`.
///
/// This is the RED handoff from OracleGuard to the Ziranity CLI: the
/// CLI is then invoked with `--payload <path>` (Cluster 6 Slice 04).
/// The function does no framing, no signing, and no transport; the
/// file contains exactly the canonical `DisbursementIntentV1` bytes
/// that `oracleguard_schemas::encoding::emit_payload` would return.
///
/// The write is a plain `std::fs::write` — atomic rename and file
/// permissions are the operator's responsibility, matched to the
/// deployment's secrets-handling posture.
pub fn write_payload_to_file(
    intent: &DisbursementIntentV1,
    path: &Path,
) -> Result<(), WritePayloadError> {
    let bytes = emit_payload(intent)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Emit the canonical payload bytes for `intent` without writing.
///
/// Thin wrapper around
/// `oracleguard_schemas::encoding::emit_payload` re-exported from the
/// adapter so shell code can consume a single submission surface.
/// Canonical bytes are owned by schemas; this re-export is a
/// convenience alias, not a second semantic source.
pub fn payload_bytes(intent: &DisbursementIntentV1) -> Result<Vec<u8>, CanonicalEncodeError> {
    emit_payload(intent)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
    use oracleguard_schemas::encoding::encode_intent;
    use oracleguard_schemas::intent::INTENT_VERSION_V1;
    use oracleguard_schemas::oracle::{
        OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
    };

    fn sample_intent() -> DisbursementIntentV1 {
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
            requested_amount_lovelace: 1_000_000_000,
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
        }
    }

    #[test]
    fn payload_bytes_equals_canonical_encoding() {
        // The adapter re-export must be byte-identical to the schemas
        // canonical encoder — there is only one source of truth.
        let intent = sample_intent();
        let via_emit = payload_bytes(&intent).expect("emit");
        let via_encode = encode_intent(&intent).expect("encode");
        assert_eq!(via_emit, via_encode);
    }

    #[test]
    fn payload_bytes_is_stable_across_calls() {
        let intent = sample_intent();
        let a = payload_bytes(&intent).expect("emit a");
        let b = payload_bytes(&intent).expect("emit b");
        assert_eq!(a, b);
    }

    #[test]
    fn write_payload_to_file_writes_canonical_bytes_only() {
        // Write to a temp path, read back, check byte-for-byte equality
        // with the canonical encoding. No envelope, signature, or
        // framing is permitted in the file contents.
        let intent = sample_intent();
        let expected = encode_intent(&intent).expect("encode");

        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "oracleguard_payload_test_{}_{}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));

        write_payload_to_file(&intent, &path).expect("write");
        let read_back = std::fs::read(&path).expect("read");
        std::fs::remove_file(&path).ok();

        assert_eq!(read_back, expected);
    }

    #[test]
    fn write_payload_to_file_surfaces_io_error() {
        // Writing to a directory that does not exist must surface an
        // Io error rather than silently succeeding or panicking.
        let intent = sample_intent();
        let path = Path::new("/nonexistent/oracleguard/payload/test.bin");
        let err = write_payload_to_file(&intent, path)
            .expect_err("missing parent directory must error");
        match err {
            WritePayloadError::Io(_) => {}
            WritePayloadError::Encode(e) => panic!("expected Io error, got Encode({e:?})"),
        }
    }
}
