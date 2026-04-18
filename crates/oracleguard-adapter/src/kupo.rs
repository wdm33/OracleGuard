//! Kupo HTTP client for reading on-chain UTxOs and datums.
//!
//! Owns: the imperative shell that hits a Kupo instance to pull the
//! Charli3 AggState UTxO and its inline datum. Closes the shell gap
//! called out in the demo-readiness audit — previously the adapter
//! could parse an AggState datum (see [`crate::charli3`]) and trigger
//! a fresh aggregation (see [`crate::charli3_aggregate`]) but had no
//! way to actually read the refreshed datum from chain.
//!
//! Does NOT own:
//!
//! - **Parsing AggState CBOR.** That's [`crate::charli3::parse_aggstate_datum`].
//!   This module fetches opaque bytes; the charli3 module interprets them.
//! - **Normalizing into canonical eval/provenance types.** That's
//!   [`crate::charli3::normalize_parse_validate`]. This module hands
//!   raw bytes + a `utxo_ref` to that pipeline.
//! - **Oracle aggregation triggers.** That's [`crate::charli3_aggregate`].
//!
//! ## Kupo endpoint surface
//!
//! Kupo (<https://cardanosolutions.github.io/kupo/>) exposes two HTTP
//! endpoints this module uses:
//!
//! - `GET /matches/{address}?unspent` — JSON array of UTxOs at the
//!   given address, filtered to unspent outputs. Each record carries
//!   `transaction_id`, `output_index`, `value.assets`, `datum_hash`,
//!   and a `created_at.slot_no` that Kupo uses as the default sort
//!   key (most recent first).
//! - `GET /datums/{datum_hash}` — JSON object `{"datum": "<hex>"}`
//!   with the raw CBOR bytes of the datum as lowercase hex.
//!
//! The hackathon's public synced instance is at
//! `http://35.209.192.203:1442` — see
//! <https://github.com/Charli3-Official/hackathon-resources>.
//!
//! ## Two-step fetch pipeline
//!
//! [`fetch_aggstate_datum`] chains two requests:
//!
//! 1. `GET /matches/{oracle_address}?unspent`
//! 2. Find the UTxO whose `value.assets` contains the configured
//!    asset key (the `policy_id.asset_name_hex` combination for the
//!    AggState token — for ADA/USD on Preprod this is
//!    [`CHARLI3_ADA_USD_ASSET_KEY`]). Extract its `datum_hash` and
//!    `transaction_id`.
//! 3. `GET /datums/{datum_hash}` → JSON `{"datum": "<hex>"}`
//! 4. Hex-decode the `datum` field → raw CBOR bytes for the AggState
//!    datum; feed into [`crate::charli3::parse_aggstate_datum`].
//! 5. Hex-decode `transaction_id` → 32-byte `aggregator_utxo_ref`
//!    that becomes `OracleFactProvenanceV1.aggregator_utxo_ref`.
//!
//! The pure parse functions ([`find_aggstate_match`],
//! [`parse_datum_response`], [`utxo_ref_bytes`]) are available
//! individually so tests can exercise them against captured JSON
//! fixtures without hitting the network.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Deserialize;

// --------------------------------------------------------------------
// Well-known constants
// --------------------------------------------------------------------

/// Base URL of the hackathon's public synced Kupo instance on Preprod.
///
/// See <https://github.com/Charli3-Official/hackathon-resources> for
/// the authoritative first-party pointer. Operators can override by
/// constructing a [`KupoEndpoint`] with a different `base_url`.
pub const HACKATHON_PREPROD_BASE_URL: &str = "http://35.209.192.203:1442";

/// Kupo asset key for the Charli3 ADA/USD AggState token on Preprod.
///
/// Format: `<policy_id>.<asset_name_hex>` — matches the `value.assets`
/// map key Kupo returns in `/matches`. Value is taken verbatim from
/// the hackathon-resources `configs/ada-usd-preprod.yml` (`policy_id`
/// field) plus the on-chain asset name `C3AS` (`43334153` in hex).
pub const CHARLI3_ADA_USD_ASSET_KEY: &str =
    "886dcb2363e160c944e63cf544ce6f6265b22ef7c4e2478dd975078e.43334153";

/// Charli3 ADA/USD oracle address on Preprod.
///
/// Same value as `oracle_address` in the hackathon's
/// `configs/ada-usd-preprod.yml`. Exposed here so operator tooling
/// can point [`fetch_aggstate_datum`] at the canonical oracle without
/// duplicating the address constant.
pub const CHARLI3_ADA_USD_ORACLE_ADDRESS: &str =
    "addr_test1wq3pacs7jcrlwehpuy3ryj8kwvsqzjp9z6dpmx8txnr0vkq6vqeuu";

/// Default timeout for Kupo HTTP requests.
///
/// Kupo responses for /matches on the Charli3 oracle address return in
/// ~100ms on the public hackathon instance. 10 seconds is generous
/// headroom for transient slowness; production operators should
/// configure shorter if they need tighter SLOs.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

// --------------------------------------------------------------------
// Endpoint configuration
// --------------------------------------------------------------------

/// Kupo HTTP endpoint configuration.
///
/// `base_url` is the scheme + host + port; URL-builders append
/// `/matches/...` and `/datums/...` paths. Trailing slashes on
/// `base_url` are stripped so either `http://host:port` or
/// `http://host:port/` produces identical request URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KupoEndpoint {
    pub base_url: String,
}

impl KupoEndpoint {
    /// Construct an endpoint from any base URL string.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    /// Convenience: the hackathon public Preprod Kupo instance.
    #[must_use]
    pub fn hackathon_preprod() -> Self {
        Self::new(HACKATHON_PREPROD_BASE_URL)
    }
}

/// Build the URL for a `/matches/{address}?unspent` request.
///
/// Pure; trailing slashes on the base URL are stripped so the output
/// has a single-slash separator.
#[must_use]
pub fn matches_url(endpoint: &KupoEndpoint, oracle_address: &str) -> String {
    format!(
        "{}/matches/{}?unspent",
        endpoint.base_url.trim_end_matches('/'),
        oracle_address
    )
}

/// Build the URL for a `/datums/{datum_hash}` request.
///
/// Pure; same trailing-slash handling as [`matches_url`].
#[must_use]
pub fn datum_url(endpoint: &KupoEndpoint, datum_hash: &str) -> String {
    format!(
        "{}/datums/{}",
        endpoint.base_url.trim_end_matches('/'),
        datum_hash
    )
}

// --------------------------------------------------------------------
// Response parsing (pure)
// --------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct MatchRecordRaw {
    transaction_id: String,
    output_index: u32,
    datum_hash: Option<String>,
    value: ValueRaw,
}

#[derive(Debug, Deserialize)]
struct ValueRaw {
    assets: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
struct DatumResponseRaw {
    datum: String,
}

/// Selected UTxO record from a Kupo `/matches` response.
///
/// Kupo returns many per-address fields; this type surfaces only the
/// three downstream consumers need:
/// - `transaction_id` — 64 hex chars, becomes the 32-byte
///   `aggregator_utxo_ref`.
/// - `output_index` — index within the transaction's outputs. Not
///   currently used by OracleGuard's authoritative path but surfaced
///   for operator tooling and for future CIP-31 reference-input use.
/// - `datum_hash` — 64 hex chars, used to fetch the datum body via
///   `/datums/{hash}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchRecord {
    pub transaction_id: String,
    pub output_index: u32,
    pub datum_hash: String,
}

/// Closed error surface for parsing Kupo responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KupoParseError {
    /// JSON body did not decode into the expected shape.
    MalformedJson,
    /// No UTxO in the `/matches` response carried the expected asset
    /// key. Possible causes: wrong `asset_key`, Kupo not synced to a
    /// block carrying the UTxO, or the oracle's AggState token was
    /// burned / moved to a different address.
    NoMatchingUtxo,
    /// Matched UTxO exists but has no `datum_hash`. AggState UTxOs on
    /// Preprod always carry inline datums; seeing this is a feed-side
    /// schema change.
    MissingDatumHash,
    /// `/datums` response lacked the `datum` field.
    DatumFieldMissing,
    /// Hex string (either a `datum` or a `transaction_id`) had an odd
    /// character count or a non-hex character.
    HexInvalid,
    /// Transaction id was a valid hex string but not exactly 32 bytes
    /// (64 hex chars). On Cardano every transaction id is a BLAKE2b-256
    /// digest, so this should be structurally impossible; surfaced for
    /// totality.
    TransactionIdWrongLength,
}

/// Find the first UTxO in a `/matches` response whose `value.assets`
/// map contains `asset_key`.
///
/// Kupo's default order for `/matches` is `created_at.slot_no`
/// descending, so "first match" is the most-recently-produced UTxO
/// carrying the asset. For the Charli3 AggState token, that's always
/// the freshest aggregation. If Kupo's default order ever changes,
/// this function's contract (take the first match) remains stable
/// but callers may need to sort explicitly.
pub fn find_aggstate_match(
    matches_json: &[u8],
    asset_key: &str,
) -> Result<MatchRecord, KupoParseError> {
    let records: Vec<MatchRecordRaw> =
        serde_json::from_slice(matches_json).map_err(|_| KupoParseError::MalformedJson)?;
    for r in records {
        if r.value.assets.contains_key(asset_key) {
            let datum_hash = r.datum_hash.ok_or(KupoParseError::MissingDatumHash)?;
            return Ok(MatchRecord {
                transaction_id: r.transaction_id,
                output_index: r.output_index,
                datum_hash,
            });
        }
    }
    Err(KupoParseError::NoMatchingUtxo)
}

/// Parse a `/datums/{hash}` response body into raw CBOR bytes.
///
/// Kupo returns `{"datum": "<hex>"}`; this function extracts and
/// hex-decodes the `datum` field. The resulting bytes are fed
/// verbatim into [`crate::charli3::parse_aggstate_datum`].
pub fn parse_datum_response(datum_json: &[u8]) -> Result<Vec<u8>, KupoParseError> {
    let resp: DatumResponseRaw =
        serde_json::from_slice(datum_json).map_err(|_| KupoParseError::MalformedJson)?;
    hex_decode(&resp.datum).ok_or(KupoParseError::HexInvalid)
}

/// Hex-decode a [`MatchRecord::transaction_id`] into the 32-byte
/// `aggregator_utxo_ref` that `OracleFactProvenanceV1` carries.
pub fn utxo_ref_bytes(record: &MatchRecord) -> Result<[u8; 32], KupoParseError> {
    let bytes = hex_decode(&record.transaction_id).ok_or(KupoParseError::HexInvalid)?;
    if bytes.len() != 32 {
        return Err(KupoParseError::TransactionIdWrongLength);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks_exact(2) {
        let hi = decode_nibble(chunk[0])?;
        let lo = decode_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Some(out)
}

fn decode_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

// --------------------------------------------------------------------
// HTTP fetch (imperative shell)
// --------------------------------------------------------------------

/// Result of a successful [`fetch_aggstate_datum`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AggStateFetch {
    /// Raw CBOR bytes of the AggState datum. Fed into
    /// [`crate::charli3::parse_aggstate_datum`] /
    /// [`crate::charli3::normalize_parse_validate`].
    pub cbor_bytes: Vec<u8>,
    /// 32-byte aggregator UTxO reference (the transaction id of the
    /// UTxO holding the AggState token). Becomes
    /// `OracleFactProvenanceV1.aggregator_utxo_ref`.
    pub aggregator_utxo_ref: [u8; 32],
}

/// Closed error surface for [`fetch_aggstate_datum`].
#[derive(Debug)]
pub enum KupoFetchError {
    /// An HTTP request failed (connection refused, non-2xx status,
    /// TLS handshake, etc.). Wrapped so the caller can inspect.
    Http(Box<ureq::Error>),
    /// An HTTP request succeeded but the body could not be read as
    /// bytes (I/O during body streaming).
    ReadBody(std::io::Error),
    /// A successful response failed structural parsing — see
    /// [`KupoParseError`] for variants.
    Parse(KupoParseError),
}

impl From<KupoParseError> for KupoFetchError {
    fn from(err: KupoParseError) -> Self {
        Self::Parse(err)
    }
}

impl From<ureq::Error> for KupoFetchError {
    fn from(err: ureq::Error) -> Self {
        Self::Http(Box::new(err))
    }
}

impl From<std::io::Error> for KupoFetchError {
    fn from(err: std::io::Error) -> Self {
        Self::ReadBody(err)
    }
}

/// Fetch the AggState datum for an oracle address via a two-step
/// Kupo pipeline: `/matches/{address}?unspent` → find UTxO →
/// `/datums/{hash}` → hex-decode.
///
/// On success returns the raw CBOR bytes of the AggState datum plus
/// the 32-byte `aggregator_utxo_ref`. On failure, surfaces an HTTP
/// error, body-read error, or parse error without interpretation.
///
/// `timeout` is applied to each individual HTTP request, not to the
/// pipeline as a whole — a slow `/matches` followed by a fast
/// `/datums` can still complete even if the combined wall time
/// exceeds `timeout`. Use [`DEFAULT_TIMEOUT`] for hackathon-friendly
/// values.
pub fn fetch_aggstate_datum(
    endpoint: &KupoEndpoint,
    oracle_address: &str,
    asset_key: &str,
    timeout: Duration,
) -> Result<AggStateFetch, KupoFetchError> {
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();

    let matches_body = agent
        .get(&matches_url(endpoint, oracle_address))
        .call()?
        .into_string()?;
    let record = find_aggstate_match(matches_body.as_bytes(), asset_key)?;

    let datum_body = agent
        .get(&datum_url(endpoint, &record.datum_hash))
        .call()?
        .into_string()?;
    let cbor_bytes = parse_datum_response(datum_body.as_bytes())?;
    let aggregator_utxo_ref = utxo_ref_bytes(&record)?;

    Ok(AggStateFetch {
        cbor_bytes,
        aggregator_utxo_ref,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // ---- Fixtures ----

    const MATCHES_SAMPLE: &[u8] =
        include_bytes!("../../../fixtures/kupo/matches_ada_usd_sample.json");
    const DATUM_SAMPLE: &[u8] = include_bytes!("../../../fixtures/kupo/datum_sample.json");

    // The first-C3AS record in the captured /matches fixture. Taken
    // from the fixture itself, not re-derived; if the fixture is
    // regenerated this constant must be regenerated alongside.
    const FIXTURE_FIRST_TX_ID: &str =
        "94b6d8ad06e3c6cdaa8f347ae8a57c826c561eaac8f9c85ed7099af2b18ac3cd";
    const FIXTURE_FIRST_DATUM_HASH: &str =
        "e99349afd009242f69ae5eedb812e4c1b904e4ba379cf4c1bfc691d8b0f22c78";
    const FIXTURE_FIRST_OUTPUT_INDEX: u32 = 1;

    // ---- URL builders ----

    #[test]
    fn matches_url_concatenates_base_and_path() {
        let e = KupoEndpoint::new("http://example.test:1442");
        let url = matches_url(&e, "addr_test1abc");
        assert_eq!(url, "http://example.test:1442/matches/addr_test1abc?unspent");
    }

    #[test]
    fn matches_url_strips_trailing_slash() {
        let e = KupoEndpoint::new("http://example.test:1442/");
        let url = matches_url(&e, "addr_test1abc");
        assert_eq!(url, "http://example.test:1442/matches/addr_test1abc?unspent");
    }

    #[test]
    fn datum_url_concatenates_base_and_path() {
        let e = KupoEndpoint::new("http://example.test:1442");
        let url = datum_url(&e, "deadbeef");
        assert_eq!(url, "http://example.test:1442/datums/deadbeef");
    }

    #[test]
    fn datum_url_strips_trailing_slash() {
        let e = KupoEndpoint::new("http://example.test:1442/");
        let url = datum_url(&e, "deadbeef");
        assert_eq!(url, "http://example.test:1442/datums/deadbeef");
    }

    #[test]
    fn hackathon_preprod_endpoint_matches_documented_url() {
        let e = KupoEndpoint::hackathon_preprod();
        assert_eq!(e.base_url, "http://35.209.192.203:1442");
    }

    // ---- find_aggstate_match ----

    #[test]
    fn find_aggstate_match_returns_first_c3as_record() {
        let record = find_aggstate_match(MATCHES_SAMPLE, CHARLI3_ADA_USD_ASSET_KEY)
            .expect("fixture contains a C3AS UTxO");
        assert_eq!(record.transaction_id, FIXTURE_FIRST_TX_ID);
        assert_eq!(record.output_index, FIXTURE_FIRST_OUTPUT_INDEX);
        assert_eq!(record.datum_hash, FIXTURE_FIRST_DATUM_HASH);
    }

    #[test]
    fn find_aggstate_match_returns_no_matching_utxo_for_wrong_asset() {
        // A real-looking but wrong asset key: correct policy id,
        // wrong asset name.
        let wrong_key = "886dcb2363e160c944e63cf544ce6f6265b22ef7c4e2478dd975078e.deadbeef";
        let err = find_aggstate_match(MATCHES_SAMPLE, wrong_key).expect_err("must reject");
        assert_eq!(err, KupoParseError::NoMatchingUtxo);
    }

    #[test]
    fn find_aggstate_match_returns_malformed_for_bad_json() {
        let err = find_aggstate_match(b"not json", CHARLI3_ADA_USD_ASSET_KEY)
            .expect_err("must reject");
        assert_eq!(err, KupoParseError::MalformedJson);
    }

    #[test]
    fn find_aggstate_match_returns_no_matching_utxo_for_empty_array() {
        let err = find_aggstate_match(b"[]", CHARLI3_ADA_USD_ASSET_KEY).expect_err("must reject");
        assert_eq!(err, KupoParseError::NoMatchingUtxo);
    }

    #[test]
    fn find_aggstate_match_is_deterministic_across_calls() {
        let a = find_aggstate_match(MATCHES_SAMPLE, CHARLI3_ADA_USD_ASSET_KEY).expect("a");
        let b = find_aggstate_match(MATCHES_SAMPLE, CHARLI3_ADA_USD_ASSET_KEY).expect("b");
        assert_eq!(a, b);
    }

    // ---- parse_datum_response ----

    #[test]
    fn parse_datum_response_returns_hex_decoded_bytes() {
        let bytes = parse_datum_response(DATUM_SAMPLE).expect("fixture parses");
        // First bytes of the captured Preprod datum: d8 79 9f d8 7b ...
        // Matches the Constr 0 / indefinite-array / Constr 2 frame
        // our charli3 parser expects.
        assert_eq!(&bytes[..5], &[0xd8, 0x79, 0x9f, 0xd8, 0x7b]);
    }

    #[test]
    fn parse_datum_response_round_trips_through_charli3_parser() {
        let bytes = parse_datum_response(DATUM_SAMPLE).expect("datum parses");
        let datum = crate::charli3::parse_aggstate_datum(&bytes).expect("CBOR parses");
        // Sanity: the captured live datum carries non-zero price.
        assert!(datum.price_microusd > 0);
        // Live Preprod prices are sub-dollar — several hundred thousand
        // microusd range. Check the value is in a plausible band.
        assert!(datum.price_microusd < 10_000_000);
        // Creation and expiry are posix ms (billions).
        assert!(datum.created_unix_ms > 1_000_000_000_000);
        assert!(datum.expiry_unix_ms > datum.created_unix_ms);
        // Expiry is within the documented 600_000 ms window of creation
        // (validity window for Charli3 Preprod).
        assert_eq!(datum.expiry_unix_ms - datum.created_unix_ms, 600_000);
    }

    #[test]
    fn parse_datum_response_rejects_malformed_json() {
        let err = parse_datum_response(b"not json").expect_err("must reject");
        assert_eq!(err, KupoParseError::MalformedJson);
    }

    #[test]
    fn parse_datum_response_rejects_odd_hex() {
        let err = parse_datum_response(br#"{"datum": "abc"}"#).expect_err("must reject");
        assert_eq!(err, KupoParseError::HexInvalid);
    }

    #[test]
    fn parse_datum_response_rejects_non_hex_char() {
        let err = parse_datum_response(br#"{"datum": "xx"}"#).expect_err("must reject");
        assert_eq!(err, KupoParseError::HexInvalid);
    }

    // ---- utxo_ref_bytes ----

    #[test]
    fn utxo_ref_bytes_decodes_32_bytes() {
        let record = MatchRecord {
            transaction_id: FIXTURE_FIRST_TX_ID.to_string(),
            output_index: FIXTURE_FIRST_OUTPUT_INDEX,
            datum_hash: FIXTURE_FIRST_DATUM_HASH.to_string(),
        };
        let bytes = utxo_ref_bytes(&record).expect("32-byte tx id decodes");
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[0], 0x94);
        assert_eq!(bytes[31], 0xcd);
    }

    #[test]
    fn utxo_ref_bytes_rejects_wrong_length_hex() {
        let record = MatchRecord {
            transaction_id: "dead".to_string(),
            output_index: 0,
            datum_hash: "aa".repeat(32),
        };
        let err = utxo_ref_bytes(&record).expect_err("short tx id rejects");
        assert_eq!(err, KupoParseError::TransactionIdWrongLength);
    }

    #[test]
    fn utxo_ref_bytes_rejects_non_hex() {
        let record = MatchRecord {
            transaction_id: "xx".repeat(32),
            output_index: 0,
            datum_hash: "aa".repeat(32),
        };
        let err = utxo_ref_bytes(&record).expect_err("non-hex tx id rejects");
        assert_eq!(err, KupoParseError::HexInvalid);
    }

    // ---- Full pipeline from fixtures ----

    #[test]
    fn full_fixture_pipeline_produces_oracleguard_provenance_shape() {
        // Mirrors fetch_aggstate_datum's parse half: run the match
        // filter + datum parse against checked-in fixtures and assert
        // the outputs have the shape OracleFactProvenanceV1 expects
        // (32-byte utxo_ref, non-empty CBOR bytes that parse through
        // the charli3 module).
        let record = find_aggstate_match(MATCHES_SAMPLE, CHARLI3_ADA_USD_ASSET_KEY).expect("match");
        let cbor = parse_datum_response(DATUM_SAMPLE).expect("datum");
        let utxo_ref = utxo_ref_bytes(&record).expect("utxo_ref");

        // utxo_ref is exactly 32 bytes and matches the captured fixture.
        assert_eq!(utxo_ref.len(), 32);
        assert_eq!(
            &utxo_ref[..4],
            &[0x94, 0xb6, 0xd8, 0xad],
            "first 4 bytes match fixture tx id prefix"
        );

        // CBOR parses and normalizes through the charli3 pipeline.
        let (eval, provenance) =
            crate::charli3::normalize_aggstate_datum(&cbor, utxo_ref).expect("normalize");
        assert_eq!(
            eval.asset_pair,
            oracleguard_schemas::oracle::ASSET_PAIR_ADA_USD,
            "asset pair is canonical ADA/USD"
        );
        assert_eq!(
            eval.source,
            oracleguard_schemas::oracle::SOURCE_CHARLI3,
            "source is canonical charli3"
        );
        assert_eq!(
            provenance.aggregator_utxo_ref, utxo_ref,
            "utxo_ref round-trips into provenance verbatim"
        );
    }

    // ---- Live integration test (ignored by default) ----
    //
    // Run with:
    //   cargo test -p oracleguard-adapter --lib kupo -- --ignored live
    //
    // Requires internet access to the hackathon's public Kupo instance.
    // Not run in CI.

    #[test]
    #[ignore = "requires network access to the hackathon Kupo instance"]
    fn live_fetch_against_hackathon_endpoint() {
        let result = fetch_aggstate_datum(
            &KupoEndpoint::hackathon_preprod(),
            CHARLI3_ADA_USD_ORACLE_ADDRESS,
            CHARLI3_ADA_USD_ASSET_KEY,
            DEFAULT_TIMEOUT,
        )
        .expect("live fetch succeeds");
        assert_eq!(result.aggregator_utxo_ref.len(), 32);
        assert_ne!(result.aggregator_utxo_ref, [0u8; 32]);
        let datum = crate::charli3::parse_aggstate_datum(&result.cbor_bytes).expect("CBOR parses");
        assert!(datum.price_microusd > 0);
        assert!(datum.expiry_unix_ms > datum.created_unix_ms);
    }
}
