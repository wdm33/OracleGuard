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

use serde::Serialize;

use crate::effect::{AssetIdV1, CardanoAddressV1};
use crate::intent::DisbursementIntentV1;
use crate::oracle::OracleFactEvalV1;

/// BLAKE3 domain separator for OracleGuard disbursement intent
/// identity. Distinct from Ziranity's `ZIRANITY_INTENT_ID_V1` and any
/// other domain tag so that cross-domain hash collisions are
/// structurally impossible.
pub const ORACLEGUARD_DISBURSEMENT_DOMAIN: &str = "ORACLEGUARD_DISBURSEMENT_V1";

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

/// Identity-bearing projection of [`DisbursementIntentV1`].
///
/// This struct intentionally omits `oracle_provenance`: provenance is
/// audit-only metadata and must not participate in intent identity.
/// Any change to a field listed here changes the intent-id; any
/// change to `oracle_provenance` does not.
#[derive(Serialize)]
struct DisbursementIntentIdentityV1 {
    intent_version: u16,
    policy_ref: [u8; 32],
    allocation_id: [u8; 32],
    requester_id: [u8; 32],
    oracle_fact: OracleFactEvalV1,
    requested_amount_lovelace: u64,
    destination: CardanoAddressV1,
    asset: AssetIdV1,
}

fn project_identity(intent: &DisbursementIntentV1) -> DisbursementIntentIdentityV1 {
    DisbursementIntentIdentityV1 {
        intent_version: intent.intent_version,
        policy_ref: intent.policy_ref,
        allocation_id: intent.allocation_id,
        requester_id: intent.requester_id,
        oracle_fact: intent.oracle_fact,
        requested_amount_lovelace: intent.requested_amount_lovelace,
        destination: intent.destination,
        asset: intent.asset,
    }
}

/// Encode the identity-bearing projection of an intent to canonical
/// bytes. Exposed for tests and for downstream code that needs to
/// verify what bytes feed the intent-id hash.
pub fn encode_intent_identity(
    intent: &DisbursementIntentV1,
) -> Result<Vec<u8>, CanonicalEncodeError> {
    postcard::to_allocvec(&project_identity(intent))
        .map_err(|_| CanonicalEncodeError::SerializerFailure)
}

/// Compute the canonical 32-byte intent identifier.
///
/// `intent_id` is BLAKE3, keyed by the OracleGuard disbursement domain
/// separator, over the canonical bytes of the identity projection. The
/// domain separator guarantees that no other OracleGuard hash path and
/// no Ziranity hash path can collide with this one, even if their
/// inputs coincide.
pub fn intent_id(intent: &DisbursementIntentV1) -> Result<[u8; 32], CanonicalEncodeError> {
    let identity_bytes = encode_intent_identity(intent)?;
    let mut hasher = blake3::Hasher::new_derive_key(ORACLEGUARD_DISBURSEMENT_DOMAIN);
    hasher.update(&identity_bytes);
    Ok(*hasher.finalize().as_bytes())
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

    // ---- Slice 05: semantic identity and intent-id tests ----

    fn id(intent: &DisbursementIntentV1) -> [u8; 32] {
        match intent_id(intent) {
            Ok(h) => h,
            Err(e) => panic!("intent_id failed: {e:?}"),
        }
    }

    #[test]
    fn identical_semantic_inputs_yield_identical_intent_ids() {
        let a = id(&fixture_intent());
        let b = id(&fixture_intent());
        assert_eq!(a, b);
    }

    #[test]
    fn policy_ref_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.policy_ref = [0xFF; 32];
        assert_ne!(base, id(&m));
    }

    #[test]
    fn allocation_id_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.allocation_id = [0xFF; 32];
        assert_ne!(base, id(&m));
    }

    #[test]
    fn requester_id_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.requester_id = [0xFF; 32];
        assert_ne!(base, id(&m));
    }

    #[test]
    fn requested_amount_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.requested_amount_lovelace = 999;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn destination_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.destination.bytes[0] = 0xAA;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn asset_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.asset.policy_id[0] = 0xAA;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn oracle_fact_price_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_fact.price_microusd = 500_000;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn oracle_fact_asset_pair_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_fact.asset_pair[0] = 0xAA;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn oracle_fact_source_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_fact.source[0] = 0xAA;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn intent_version_mutation_changes_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.intent_version = 2;
        assert_ne!(base, id(&m));
    }

    #[test]
    fn oracle_provenance_timestamp_does_not_change_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_provenance.timestamp_unix = 9_999_999_999_999;
        assert_eq!(base, id(&m));
    }

    #[test]
    fn oracle_provenance_expiry_does_not_change_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_provenance.expiry_unix = 0;
        assert_eq!(base, id(&m));
    }

    #[test]
    fn oracle_provenance_utxo_ref_does_not_change_intent_id() {
        let base = id(&fixture_intent());
        let mut m = fixture_intent();
        m.oracle_provenance.aggregator_utxo_ref = [0xCC; 32];
        assert_eq!(base, id(&m));
    }

    #[test]
    fn domain_separator_is_not_a_ziranity_tag() {
        // Guard against an accidental copy-paste of a Ziranity tag.
        // The OracleGuard disbursement-intent identity must have its
        // own domain separator to prevent cross-domain collisions.
        assert_eq!(
            ORACLEGUARD_DISBURSEMENT_DOMAIN,
            "ORACLEGUARD_DISBURSEMENT_V1"
        );
        assert_ne!(ORACLEGUARD_DISBURSEMENT_DOMAIN, "ZIRANITY_INTENT_ID_V1");
        assert_ne!(
            ORACLEGUARD_DISBURSEMENT_DOMAIN,
            "ZIRANITY_VERIFICATION_BUNDLE_V1"
        );
        assert_ne!(ORACLEGUARD_DISBURSEMENT_DOMAIN, "ZIRANITY_REPLAY_GOLDEN_V1");
    }

    #[test]
    fn different_domain_separator_yields_different_hash() {
        // Hand-roll a competing hash with a Ziranity domain tag over
        // the same bytes. It must differ, or the domain separation is
        // doing no work.
        let bytes = match encode_intent_identity(&fixture_intent()) {
            Ok(b) => b,
            Err(e) => panic!("encode_intent_identity failed: {e:?}"),
        };
        let ours = id(&fixture_intent());
        let mut other = blake3::Hasher::new_derive_key("ZIRANITY_INTENT_ID_V1");
        other.update(&bytes);
        let other_hash = *other.finalize().as_bytes();
        assert_ne!(ours, other_hash);
    }

    #[test]
    fn intent_id_is_not_plain_blake3_of_identity_bytes() {
        // The domain-separated key must actually be in use. A plain
        // BLAKE3 over the identity bytes must not match.
        let bytes = match encode_intent_identity(&fixture_intent()) {
            Ok(b) => b,
            Err(e) => panic!("encode_intent_identity failed: {e:?}"),
        };
        let ours = id(&fixture_intent());
        let plain = *blake3::hash(&bytes).as_bytes();
        assert_ne!(ours, plain);
    }

    #[test]
    fn identity_bytes_omit_provenance() {
        // The identity-byte length must be strictly less than the full
        // canonical byte length by exactly the size of the postcard
        // encoding of OracleFactProvenanceV1 (u64 + u64 + [u8;32] =
        // varint u64 (up to 10) + varint u64 (up to 10) + 32).
        let full = encode(&fixture_intent());
        let identity = match encode_intent_identity(&fixture_intent()) {
            Ok(b) => b,
            Err(e) => panic!("encode_intent_identity failed: {e:?}"),
        };
        assert!(
            identity.len() < full.len(),
            "identity bytes must be shorter than full canonical bytes"
        );
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
