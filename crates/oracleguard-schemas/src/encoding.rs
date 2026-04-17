//! Canonical byte encoding for OracleGuard domain payloads.
//!
//! Owns: the single authoritative encoding for [`DisbursementIntentV1`]
//! and its supporting types. The encoding is `postcard` at a pinned
//! major version (see `Cargo.toml`); the major version is part of the
//! encoding contract, because a major-version bump would be a
//! canonical-byte change.
//!
//! Does NOT own: envelope/signing/transport bytes (those are the
//! Ziranity submission path's concern; see `oracleguard-adapter`),
//! JSON or human-readable renderings (those are not authoritative).

use crate::intent::DisbursementIntentV1;

/// Errors produced by [`encode_intent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalEncodeError {
    /// The underlying serializer rejected the value. Should not happen
    /// for well-formed fixed-width types; surfaced rather than panicked.
    SerializerFailure,
}

/// Errors produced by [`decode_intent`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalDecodeError {
    /// Input could not be decoded as a valid [`DisbursementIntentV1`].
    Malformed,
    /// Input decoded as a valid intent but had unconsumed trailing
    /// bytes, which is ambiguous and therefore rejected.
    TrailingBytes,
}

/// Encode a [`DisbursementIntentV1`] to its canonical byte form.
///
/// The output bytes are the authoritative identity of the intent for
/// every downstream check: submission payload, intent-id hashing
/// (slice 05), evidence replay, and verifier re-encoding.
pub fn encode_intent(intent: &DisbursementIntentV1) -> Result<Vec<u8>, CanonicalEncodeError> {
    postcard::to_allocvec(intent).map_err(|_| CanonicalEncodeError::SerializerFailure)
}

/// Decode canonical bytes back into a [`DisbursementIntentV1`].
///
/// The decoder is strict: trailing bytes after a successful decode are
/// rejected, so one byte sequence cannot decode to two distinct
/// meanings.
pub fn decode_intent(bytes: &[u8]) -> Result<DisbursementIntentV1, CanonicalDecodeError> {
    let (intent, remainder) = postcard::take_from_bytes::<DisbursementIntentV1>(bytes)
        .map_err(|_| CanonicalDecodeError::Malformed)?;
    if !remainder.is_empty() {
        return Err(CanonicalDecodeError::TrailingBytes);
    }
    Ok(intent)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::effect::{AssetIdV1, CardanoAddressV1};
    use crate::intent::INTENT_VERSION_V1;
    use crate::oracle::{OracleFactEvalV1, OracleFactProvenanceV1};

    const INTENT_GOLDEN: &[u8] = include_bytes!("../../../fixtures/intent_v1_golden.postcard");

    fn fixture_intent() -> DisbursementIntentV1 {
        DisbursementIntentV1 {
            intent_version: INTENT_VERSION_V1,
            policy_ref: [0x11; 32],
            allocation_id: [0x22; 32],
            requester_id: [0x33; 32],
            oracle_fact: OracleFactEvalV1 {
                asset_pair: *b"ADA/USD\0\0\0\0\0\0\0\0\0",
                price_microusd: 450_000,
                source: *b"charli3\0\0\0\0\0\0\0\0\0",
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

    fn encode(intent: &DisbursementIntentV1) -> Vec<u8> {
        match encode_intent(intent) {
            Ok(bytes) => bytes,
            Err(e) => panic!("encode failed: {e:?}"),
        }
    }

    fn decode(bytes: &[u8]) -> DisbursementIntentV1 {
        match decode_intent(bytes) {
            Ok(i) => i,
            Err(e) => panic!("decode failed: {e:?}"),
        }
    }

    #[test]
    fn encode_is_stable_across_calls() {
        let a = encode(&fixture_intent());
        let b = encode(&fixture_intent());
        assert_eq!(a, b);
    }

    #[test]
    fn encode_decode_encode_round_trip_is_identity() {
        let intent = fixture_intent();
        let bytes = encode(&intent);
        let decoded = decode(&bytes);
        let re_encoded = encode(&decoded);
        assert_eq!(bytes, re_encoded);
        assert_eq!(intent, decoded);
    }

    #[test]
    fn encoded_bytes_match_golden() {
        let bytes = encode(&fixture_intent());
        assert_eq!(bytes.as_slice(), INTENT_GOLDEN);
    }

    #[test]
    fn decoder_rejects_trailing_bytes() {
        let mut bytes = encode(&fixture_intent());
        bytes.push(0x00);
        let err = decode_intent(&bytes).expect_err("expected trailing-byte rejection");
        assert_eq!(err, CanonicalDecodeError::TrailingBytes);
    }

    #[test]
    fn decoder_rejects_truncated_input() {
        let bytes = encode(&fixture_intent());
        let truncated = &bytes[..bytes.len() - 1];
        let err = decode_intent(truncated).expect_err("expected malformed rejection");
        assert_eq!(err, CanonicalDecodeError::Malformed);
    }

    #[test]
    fn decoder_rejects_empty_input() {
        let err = decode_intent(&[]).expect_err("expected malformed rejection");
        assert_eq!(err, CanonicalDecodeError::Malformed);
    }

    #[test]
    fn field_mutation_changes_canonical_bytes() {
        let base = encode(&fixture_intent());
        let mut mutated = fixture_intent();
        mutated.requested_amount_lovelace = 2_000_000_000;
        let mutated_bytes = encode(&mutated);
        assert_ne!(base, mutated_bytes);
    }

    // Regenerate `fixtures/intent_v1_golden.postcard` when the fixture
    // shape itself changes deliberately. Not run in CI:
    //   cargo test -p oracleguard-schemas -- --ignored regenerate
    #[test]
    #[ignore = "regenerates golden fixture; run manually only"]
    fn regenerate_golden_intent_fixture() {
        let bytes = encode(&fixture_intent());
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/intent_v1_golden.postcard"
        );
        std::fs::write(path, bytes).expect("write golden fixture");
    }
}
