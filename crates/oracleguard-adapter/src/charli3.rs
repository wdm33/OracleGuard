//! Charli3 oracle fetch and normalization shell.
//!
//! Owns: the imperative code that fetches an `AggState` UTxO from a
//! Kupo HTTP endpoint plus the pure CBOR-parsing helpers that reduce
//! a raw Plutus datum into the canonical evaluation- and
//! provenance-domain types from `oracleguard-schemas`.
//!
//! Does NOT own: the canonical oracle observation type (schemas), the
//! aggregation protocol (Python pull-oracle client, invoked out-of-band
//! by the operator or a separate shell step), nor any decision based
//! on the observation (policy).
//!
//! The module is deliberately split: the raw-bytes → typed-values
//! parser and the freshness check are pure functions suitable for
//! inlining into tests and fuzzers. Only the HTTP fetch path in
//! [`fetch_aggstate_datum`] performs I/O.

#![deny(clippy::float_arithmetic)]

use minicbor::data::{Tag, Type};
use minicbor::decode::Decoder;
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};

/// Plutus constructor tags used in the AggState datum.
///
/// Plutus encodes data-constructors as CBOR tags in the range
/// `121..=127` for constructor indices `0..=6`. The AggState datum
/// observed on Preprod uses `Constr 0` (tag 121) at the outer level
/// and `Constr 2` (tag 123) at the inner level.
const CONSTR_0_TAG: u64 = 121;
const CONSTR_2_TAG: u64 = 123;

/// Map keys inside the AggState `price_map`.
///
/// These are the canonical keys Charli3 uses on Preprod and are
/// verified in `fixtures/charli3/aggstate_v1_golden.capture.md`.
const KEY_PRICE: u64 = 0;
const KEY_CREATED_MS: u64 = 1;
const KEY_EXPIRY_MS: u64 = 2;

/// Raw parsed AggState datum.
///
/// The fields mirror the three entries of the on-chain `price_map`.
/// The integer units match the on-chain units exactly; no unit
/// conversion happens in the parser. That is the normalizer's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AggStateDatum {
    /// `price_map[0]` — integer micro-USD as published by the feed.
    pub price_microusd: u64,
    /// `price_map[1]` — posix milliseconds at which the aggregator
    /// produced this datum.
    pub created_unix_ms: u64,
    /// `price_map[2]` — posix milliseconds after which the datum is
    /// stale.
    pub expiry_unix_ms: u64,
}

/// Closed set of reasons a raw AggState datum can be rejected.
///
/// Kept distinct from the semantic `DisbursementReasonCode` so the
/// structural-CBOR failure surface does not leak into the evaluator's
/// reason space. A later slice maps these into the authoritative
/// reason codes at the evaluator boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Charli3ParseError {
    /// Input could not be decoded as CBOR at all, or had an
    /// unexpected CBOR shape (wrong tag, wrong type, missing break,
    /// trailing bytes, etc.).
    MalformedCbor,
    /// CBOR was well-formed but the outer constructor tag was not
    /// `Constr 0` / `Constr 2` as expected for an AggState datum.
    UnexpectedConstructor,
    /// The inner `price_map` did not contain key `0` (price).
    MissingPriceField,
    /// The inner `price_map` did not contain key `1` (created timestamp).
    MissingTimestampField,
    /// The inner `price_map` did not contain key `2` (expiry timestamp).
    MissingExpiryField,
    /// A required integer value was encoded as something other than a
    /// non-negative integer.
    PriceNotInteger,
    /// A required integer value was encoded as something other than a
    /// non-negative integer.
    TimestampNotInteger,
    /// An integer value exceeded `u64::MAX`. `price_map` values on
    /// Charli3 Preprod fit inside `u64`, and OracleGuard's authoritative
    /// integer surface is `u64` throughout.
    IntegerTooLarge,
    /// The `price_map` contained a key outside the expected set
    /// `{0, 1, 2}`, or a duplicate key. Either case would change the
    /// authoritative meaning of the datum silently and is rejected.
    UnexpectedMapKey,
}

/// Returned by [`check_freshness`] when the AggState datum has expired.
///
/// Kept as its own error type so the freshness check cannot be
/// confused with a CBOR-shape failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OracleStale {
    /// `price_map[2]` — expiry posix ms from the datum.
    pub expiry_unix_ms: u64,
    /// Wall-clock value the freshness check was evaluated against.
    pub now_unix_ms: u64,
}

/// Parse an AggState inline-datum byte sequence into its typed form.
///
/// The parser is strict:
///
/// - the outer value must be `Constr 0` (tag 121) wrapping a one-element
///   array;
/// - that element must be `Constr 2` (tag 123) wrapping a one-element
///   array;
/// - that element must be a three-entry map with integer keys `0`, `1`,
///   `2` (exactly once each);
/// - each value must be a non-negative integer that fits in `u64`;
/// - no trailing bytes may follow the outer structure.
///
/// Any deviation produces a typed [`Charli3ParseError`] — there is no
/// best-effort or partial parse.
pub fn parse_aggstate_datum(bytes: &[u8]) -> Result<AggStateDatum, Charli3ParseError> {
    let mut d = Decoder::new(bytes);
    let datum = decode_outer(&mut d)?;
    if d.position() != bytes.len() {
        return Err(Charli3ParseError::MalformedCbor);
    }
    Ok(datum)
}

fn decode_outer(d: &mut Decoder<'_>) -> Result<AggStateDatum, Charli3ParseError> {
    expect_tag(d, CONSTR_0_TAG, Charli3ParseError::UnexpectedConstructor)?;
    expect_array_with_one_element(d)?;
    let datum = decode_inner(d)?;
    expect_array_break(d)?;
    Ok(datum)
}

fn decode_inner(d: &mut Decoder<'_>) -> Result<AggStateDatum, Charli3ParseError> {
    expect_tag(d, CONSTR_2_TAG, Charli3ParseError::UnexpectedConstructor)?;
    expect_array_with_one_element(d)?;
    let datum = decode_price_map(d)?;
    expect_array_break(d)?;
    Ok(datum)
}

fn decode_price_map(d: &mut Decoder<'_>) -> Result<AggStateDatum, Charli3ParseError> {
    let len = d.map().map_err(|_| Charli3ParseError::MalformedCbor)?;
    let Some(len) = len else {
        // indefinite-length maps are not used on Preprod; rejecting
        // them here keeps the accepted encoding canonical.
        return Err(Charli3ParseError::MalformedCbor);
    };
    if len != 3 {
        return Err(Charli3ParseError::MalformedCbor);
    }

    let mut price: Option<u64> = None;
    let mut created: Option<u64> = None;
    let mut expiry: Option<u64> = None;

    for _ in 0..3 {
        let key = read_u64_or(d, Charli3ParseError::MalformedCbor)?;
        match key {
            KEY_PRICE => {
                if price.is_some() {
                    return Err(Charli3ParseError::UnexpectedMapKey);
                }
                price = Some(read_u64_or(d, Charli3ParseError::PriceNotInteger)?);
            }
            KEY_CREATED_MS => {
                if created.is_some() {
                    return Err(Charli3ParseError::UnexpectedMapKey);
                }
                created = Some(read_u64_or(d, Charli3ParseError::TimestampNotInteger)?);
            }
            KEY_EXPIRY_MS => {
                if expiry.is_some() {
                    return Err(Charli3ParseError::UnexpectedMapKey);
                }
                expiry = Some(read_u64_or(d, Charli3ParseError::TimestampNotInteger)?);
            }
            _ => return Err(Charli3ParseError::UnexpectedMapKey),
        }
    }

    let price_microusd = price.ok_or(Charli3ParseError::MissingPriceField)?;
    let created_unix_ms = created.ok_or(Charli3ParseError::MissingTimestampField)?;
    let expiry_unix_ms = expiry.ok_or(Charli3ParseError::MissingExpiryField)?;

    Ok(AggStateDatum {
        price_microusd,
        created_unix_ms,
        expiry_unix_ms,
    })
}

fn expect_tag(
    d: &mut Decoder<'_>,
    expected: u64,
    on_mismatch: Charli3ParseError,
) -> Result<(), Charli3ParseError> {
    let tag: Tag = d.tag().map_err(|_| Charli3ParseError::MalformedCbor)?;
    if u64::from(tag) != expected {
        return Err(on_mismatch);
    }
    Ok(())
}

fn expect_array_with_one_element(d: &mut Decoder<'_>) -> Result<(), Charli3ParseError> {
    let header = d.array().map_err(|_| Charli3ParseError::MalformedCbor)?;
    match header {
        Some(1) => Ok(()),
        Some(_) => Err(Charli3ParseError::MalformedCbor),
        // Indefinite-length: the on-chain datum uses indefinite arrays
        // (0x9f...0xff). We accept this variant and the caller is
        // expected to close it with [`expect_array_break`].
        None => Ok(()),
    }
}

fn expect_array_break(d: &mut Decoder<'_>) -> Result<(), Charli3ParseError> {
    let ty = d.datatype().map_err(|_| Charli3ParseError::MalformedCbor)?;
    match ty {
        Type::Break => {
            d.skip().map_err(|_| Charli3ParseError::MalformedCbor)?;
            Ok(())
        }
        // A definite-length array of one element that has been fully
        // consumed has no trailing break; in that case we must not
        // consume anything else here.
        _ => Ok(()),
    }
}

fn read_u64_or(
    d: &mut Decoder<'_>,
    on_failure: Charli3ParseError,
) -> Result<u64, Charli3ParseError> {
    match d.datatype().map_err(|_| Charli3ParseError::MalformedCbor)? {
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
            d.u64().map_err(|_| Charli3ParseError::IntegerTooLarge)
        }
        _ => Err(on_failure),
    }
}

/// Normalize a raw AggState datum into the canonical eval- and
/// provenance-domain oracle types.
///
/// The caller supplies the `aggregator_utxo_ref` (the 32-byte UTxO
/// reference, typically the transaction id of the UTxO holding the
/// AggState token) because the datum bytes alone do not tell us which
/// UTxO they were fetched from. Provenance-only; never influences
/// authorization.
///
/// The asset-pair and source labels are pinned to the MVP canonical
/// identifiers:
/// - `asset_pair` = [`ASSET_PAIR_ADA_USD`]
/// - `source`     = [`SOURCE_CHARLI3`]
pub fn normalize_aggstate_datum(
    bytes: &[u8],
    aggregator_utxo_ref: [u8; 32],
) -> Result<(OracleFactEvalV1, OracleFactProvenanceV1), Charli3ParseError> {
    let datum = parse_aggstate_datum(bytes)?;
    let eval = OracleFactEvalV1 {
        asset_pair: ASSET_PAIR_ADA_USD,
        price_microusd: datum.price_microusd,
        source: SOURCE_CHARLI3,
    };
    let provenance = OracleFactProvenanceV1 {
        timestamp_unix: datum.created_unix_ms,
        expiry_unix: datum.expiry_unix_ms,
        aggregator_utxo_ref,
    };
    Ok((eval, provenance))
}

/// Pure freshness check.
///
/// `now_unix_ms` is taken as an explicit argument so this function
/// stays deterministic and testable. The caller — a shell — is
/// responsible for reading the wall clock. An expired datum must not
/// reach the evaluator; the adapter is expected to treat
/// [`OracleStale`] as the typed rejection for that case and map it
/// onto the authoritative reason-code surface at the policy boundary.
pub fn check_freshness(
    provenance: &OracleFactProvenanceV1,
    now_unix_ms: u64,
) -> Result<(), OracleStale> {
    if now_unix_ms < provenance.expiry_unix {
        Ok(())
    } else {
        Err(OracleStale {
            expiry_unix_ms: provenance.expiry_unix,
            now_unix_ms,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Golden on-chain datum, captured from Preprod Charli3 ADA/USD
    /// AggState UTxO on 2026-04-17. See
    /// `fixtures/charli3/aggstate_v1_golden.capture.md` for source.
    const GOLDEN: &[u8] = include_bytes!("../../../fixtures/charli3/aggstate_v1_golden.cbor");

    const GOLDEN_PRICE_MICROUSD: u64 = 258_000;
    const GOLDEN_CREATED_MS: u64 = 1_776_428_823_000;
    const GOLDEN_EXPIRY_MS: u64 = 1_776_429_423_000;
    const GOLDEN_UTXO_REF: [u8; 32] = [0xAA; 32];

    #[test]
    fn parse_golden_extracts_expected_integers() {
        let datum = parse_aggstate_datum(GOLDEN).expect("golden should parse");
        assert_eq!(datum.price_microusd, GOLDEN_PRICE_MICROUSD);
        assert_eq!(datum.created_unix_ms, GOLDEN_CREATED_MS);
        assert_eq!(datum.expiry_unix_ms, GOLDEN_EXPIRY_MS);
    }

    #[test]
    fn parse_is_stable_across_runs() {
        let a = parse_aggstate_datum(GOLDEN).expect("parse a");
        let b = parse_aggstate_datum(GOLDEN).expect("parse b");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_produces_canonical_asset_pair_and_source() {
        let (eval, _) = normalize_aggstate_datum(GOLDEN, GOLDEN_UTXO_REF).expect("normalize");
        assert_eq!(eval.asset_pair, ASSET_PAIR_ADA_USD);
        assert_eq!(eval.source, SOURCE_CHARLI3);
    }

    #[test]
    fn normalize_carries_integer_microusd_unchanged() {
        let (eval, _) = normalize_aggstate_datum(GOLDEN, GOLDEN_UTXO_REF).expect("normalize");
        assert_eq!(eval.price_microusd, GOLDEN_PRICE_MICROUSD);
    }

    #[test]
    fn normalize_carries_provenance_without_mutating_eval_fields() {
        let (eval, provenance) =
            normalize_aggstate_datum(GOLDEN, GOLDEN_UTXO_REF).expect("normalize");
        assert_eq!(provenance.timestamp_unix, GOLDEN_CREATED_MS);
        assert_eq!(provenance.expiry_unix, GOLDEN_EXPIRY_MS);
        assert_eq!(provenance.aggregator_utxo_ref, GOLDEN_UTXO_REF);
        // Eval fields are unchanged relative to the direct parse.
        let raw = parse_aggstate_datum(GOLDEN).expect("parse");
        assert_eq!(eval.price_microusd, raw.price_microusd);
    }

    #[test]
    fn normalize_same_bytes_yields_identical_eval_bytes() {
        // Replay determinism: same canonical input → byte-identical
        // eval-domain representation via postcard.
        let (a, _) = normalize_aggstate_datum(GOLDEN, GOLDEN_UTXO_REF).expect("a");
        let (b, _) = normalize_aggstate_datum(GOLDEN, GOLDEN_UTXO_REF).expect("b");
        let ea = postcard::to_allocvec(&a).expect("encode a");
        let eb = postcard::to_allocvec(&b).expect("encode b");
        assert_eq!(ea, eb);
    }

    #[test]
    fn parse_rejects_trailing_bytes() {
        let mut extended: Vec<u8> = GOLDEN.to_vec();
        extended.push(0x00);
        let err = parse_aggstate_datum(&extended).expect_err("trailing byte must be rejected");
        assert_eq!(err, Charli3ParseError::MalformedCbor);
    }

    #[test]
    fn parse_rejects_truncated_input() {
        let truncated = &GOLDEN[..GOLDEN.len() - 1];
        let err = parse_aggstate_datum(truncated).expect_err("truncated input must be rejected");
        assert_eq!(err, Charli3ParseError::MalformedCbor);
    }

    #[test]
    fn parse_rejects_empty_input() {
        let err = parse_aggstate_datum(&[]).expect_err("empty input must be rejected");
        assert_eq!(err, Charli3ParseError::MalformedCbor);
    }

    #[test]
    fn parse_rejects_unknown_outer_constructor() {
        // Constr 1 (tag 122) instead of Constr 0: swap the second
        // byte. Byte 0 is 0xd8 (tag header with 1-byte value), byte 1
        // is 0x79 (= 121). Flipping to 0x7a gives 122 = Constr 1.
        let mut tampered: Vec<u8> = GOLDEN.to_vec();
        tampered[1] = 0x7a;
        let err = parse_aggstate_datum(&tampered).expect_err("bad constructor must be rejected");
        assert_eq!(err, Charli3ParseError::UnexpectedConstructor);
    }

    #[test]
    fn parse_rejects_bad_inner_constructor() {
        // Bytes layout of the outer header: 0xd8 0x79 0x9f 0xd8 0x7b ...
        //                                     ^^^^ ^^^^      ^^^^ ^^^^
        //                                     tag  121       tag  123
        // Byte 4 is the inner tag value (0x7b = 123). Flipping it to
        // 0x7d (= 125) keeps the CBOR well-formed but makes the inner
        // constructor Constr 4 instead of Constr 2.
        let mut tampered: Vec<u8> = GOLDEN.to_vec();
        tampered[4] = 0x7d;
        let err = parse_aggstate_datum(&tampered).expect_err("bad inner constructor rejected");
        assert_eq!(err, Charli3ParseError::UnexpectedConstructor);
    }

    #[test]
    fn normalize_rejects_empty_input_as_malformed() {
        let err = normalize_aggstate_datum(&[], GOLDEN_UTXO_REF)
            .expect_err("empty must be rejected");
        assert_eq!(err, Charli3ParseError::MalformedCbor);
    }

    #[test]
    fn check_freshness_accepts_before_expiry() {
        let provenance = OracleFactProvenanceV1 {
            timestamp_unix: GOLDEN_CREATED_MS,
            expiry_unix: GOLDEN_EXPIRY_MS,
            aggregator_utxo_ref: GOLDEN_UTXO_REF,
        };
        assert_eq!(check_freshness(&provenance, GOLDEN_EXPIRY_MS - 1), Ok(()));
    }

    #[test]
    fn check_freshness_rejects_at_or_after_expiry() {
        let provenance = OracleFactProvenanceV1 {
            timestamp_unix: GOLDEN_CREATED_MS,
            expiry_unix: GOLDEN_EXPIRY_MS,
            aggregator_utxo_ref: GOLDEN_UTXO_REF,
        };
        let err = check_freshness(&provenance, GOLDEN_EXPIRY_MS).expect_err("at expiry is stale");
        assert_eq!(err.expiry_unix_ms, GOLDEN_EXPIRY_MS);
        assert_eq!(err.now_unix_ms, GOLDEN_EXPIRY_MS);

        let err2 = check_freshness(&provenance, GOLDEN_EXPIRY_MS + 60_000)
            .expect_err("past expiry is stale");
        assert_eq!(err2.now_unix_ms, GOLDEN_EXPIRY_MS + 60_000);
    }

    #[test]
    fn check_freshness_is_pure_wrt_now_argument() {
        // Two calls with the same provenance and the same now produce
        // the same result. Replay-safe: no hidden wall-clock read.
        let provenance = OracleFactProvenanceV1 {
            timestamp_unix: GOLDEN_CREATED_MS,
            expiry_unix: GOLDEN_EXPIRY_MS,
            aggregator_utxo_ref: GOLDEN_UTXO_REF,
        };
        let r1 = check_freshness(&provenance, 1);
        let r2 = check_freshness(&provenance, 1);
        assert_eq!(r1, r2);
    }
}
