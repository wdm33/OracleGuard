//! Cardano settlement shell.
//!
//! Owns:
//!
//! - [`CardanoTxHashV1`] — the canonical 32-byte Cardano transaction
//!   id type with strict hex round-trip. This is the stable settlement
//!   reference surfaced on the allow path and consumed by Cluster 8
//!   evidence.
//! - [`SettlementSubmitError`] — the closed shell error surface
//!   returned by settlement backends.
//! - [`SettlementBackend`] — the submission-boundary trait that the
//!   fulfillment entrypoint (Slice 04) uses to execute an allow-path
//!   settlement.
//! - [`CardanoCliSettlementBackend`] — the RED `cardano-cli`-backed
//!   implementation of [`SettlementBackend`]. It performs process
//!   spawning to capture a transaction id; it does NOT define
//!   semantic meaning and it is NOT reachable from the
//!   [`crate::settlement`] module's pure mapping or guard logic.
//!
//! Does NOT own:
//!
//! - The authorized-effect type (that is
//!   `oracleguard_schemas::effect::AuthorizedEffectV1`).
//! - The fulfillment boundary (that is [`crate::settlement`]).
//! - Evidence rendering (Cluster 8).

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::settlement::SettlementRequest;

// -------------------------------------------------------------------
// Slice 05 — Settlement-backend trait
// -------------------------------------------------------------------

/// Submission boundary used by the fulfillment entrypoint (Slice 04)
/// to execute an allow-path settlement.
///
/// The trait is defined here — next to the shell backend that
/// implements it — so production code and tests can refer to a
/// single, stable surface. Production uses
/// [`CardanoCliSettlementBackend`]; tests use deterministic fixture
/// implementations that record the [`SettlementRequest`] they receive
/// and return a pinned [`CardanoTxHashV1`].
///
/// The trait deliberately takes `&self` so implementations cannot
/// grow mutable state that would hide nondeterminism. Any per-call
/// state (temp files, network handles) lives inside the `submit`
/// body.
pub trait SettlementBackend {
    /// Submit a settlement request and return the captured Cardano
    /// transaction hash on success.
    fn submit(&self, request: &SettlementRequest)
        -> Result<CardanoTxHashV1, SettlementSubmitError>;
}

// -------------------------------------------------------------------
// Slice 05 — Canonical transaction-hash type
// -------------------------------------------------------------------

/// Canonical 32-byte Cardano transaction id.
///
/// Cardano transaction ids are 32-byte BLAKE2b-256 digests. The type
/// is fixed-width and `Copy` so settlement outputs participate in
/// byte-identical replay without heap layout creeping in.
///
/// Use [`CardanoTxHashV1::from_hex`] / [`CardanoTxHashV1::to_hex`] at
/// the shell boundary (the `cardano-cli` `transaction txid` command
/// prints lowercase hex). Internal comparisons use byte identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CardanoTxHashV1(pub [u8; 32]);

/// Errors returned by [`CardanoTxHashV1::from_hex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxHashParseError {
    /// The input length was not exactly 64 hex characters.
    WrongLength,
    /// The input contained a character outside `0-9a-f`. Uppercase is
    /// rejected so byte identity stays unambiguous.
    InvalidChar,
}

impl CardanoTxHashV1 {
    /// Parse a 64-character lowercase-hex string into a tx hash.
    ///
    /// Strict: length must be exactly 64; every character must be in
    /// `0-9a-f`. Uppercase input is rejected — the `cardano-cli`
    /// output is lowercase by convention and we do not canonicalize
    /// silently.
    pub fn from_hex(s: &str) -> Result<Self, TxHashParseError> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(TxHashParseError::WrongLength);
        }
        let mut out = [0u8; 32];
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = decode_nibble(chunk[0])?;
            let lo = decode_nibble(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Ok(CardanoTxHashV1(out))
    }

    /// Render a tx hash as a 64-character lowercase-hex string.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.0 {
            out.push(nibble_char(b >> 4));
            out.push(nibble_char(b & 0x0F));
        }
        out
    }
}

fn decode_nibble(b: u8) -> Result<u8, TxHashParseError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(TxHashParseError::InvalidChar),
    }
}

fn nibble_char(n: u8) -> char {
    match n {
        0..=9 => char::from(b'0' + n),
        10..=15 => char::from(b'a' + n - 10),
        _ => unreachable!(),
    }
}

// -------------------------------------------------------------------
// Slice 05 — Shell error surface
// -------------------------------------------------------------------

/// Closed error surface returned by a [`SettlementBackend::submit`]
/// implementation.
#[derive(Debug)]
pub enum SettlementSubmitError {
    /// Spawning the `cardano-cli` binary failed (not found on PATH,
    /// permission denied, etc.).
    Spawn(io::Error),
    /// The CLI process ran but exited non-zero. `stdout` and `stderr`
    /// are surfaced verbatim so the caller can diagnose without this
    /// module interpreting them.
    NonZeroExit {
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// The CLI exited successfully but its stdout did not contain a
    /// parseable 64-character lowercase-hex transaction id.
    ParseTxHash(TxHashParseError),
}

impl From<TxHashParseError> for SettlementSubmitError {
    fn from(err: TxHashParseError) -> Self {
        SettlementSubmitError::ParseTxHash(err)
    }
}

// -------------------------------------------------------------------
// Slice 05 — cardano-cli settlement backend skeleton
// -------------------------------------------------------------------

/// RED `cardano-cli`-backed implementation of
/// [`SettlementBackend`].
///
/// The backend spawns `cardano-cli transaction txid --tx-file <path>`
/// against the path in [`Self::signed_tx_path_for`] and parses its
/// stdout into a [`CardanoTxHashV1`]. Transaction build/sign and
/// ledger-state observation are out of scope for Cluster 7; the
/// capture boundary is pinned here so Cluster 8 can depend on a
/// stable `Settled { tx_hash }` surface.
///
/// Fields are `pub` so operator tooling can construct a backend
/// without a builder. No field reaches the Cardano output that is
/// not already present on the authorized effect.
#[derive(Debug, Clone)]
pub struct CardanoCliSettlementBackend {
    /// Path to the `cardano-cli` binary.
    pub cardano_cli_bin: PathBuf,
    /// Network-selection arguments, e.g.
    /// `["--testnet-magic".into(), "2".into()]` for Preprod.
    pub network_args: Vec<String>,
    /// Directory that contains the signed tx file this backend will
    /// read the id from. The per-request path is computed by
    /// [`Self::signed_tx_path_for`].
    pub work_dir: PathBuf,
}

impl CardanoCliSettlementBackend {
    /// Path to the signed transaction file for `request`. Deterministic
    /// from the request's `intent_id` so submissions are idempotent on
    /// the filesystem.
    pub fn signed_tx_path_for(&self, request: &SettlementRequest) -> PathBuf {
        let mut name = String::with_capacity(64 + 8);
        for b in request.intent_id {
            name.push(nibble_char(b >> 4));
            name.push(nibble_char(b & 0x0F));
        }
        name.push_str(".signed");
        self.work_dir.join(name)
    }

    /// Argument vector passed to `cardano-cli` for the txid capture
    /// step. Exposed as a pure helper so tests can pin exact args
    /// without spawning a process.
    pub fn txid_cli_args(&self, request: &SettlementRequest) -> Vec<String> {
        let mut args = Vec::with_capacity(4 + self.network_args.len());
        args.push("transaction".to_string());
        args.push("txid".to_string());
        args.extend(self.network_args.iter().cloned());
        args.push("--tx-file".to_string());
        args.push(
            self.signed_tx_path_for(request)
                .to_string_lossy()
                .into_owned(),
        );
        args
    }

    fn spawn_txid(&self, request: &SettlementRequest) -> Result<Vec<u8>, SettlementSubmitError> {
        let args = self.txid_cli_args(request);
        let output = Command::new(&self.cardano_cli_bin)
            .args(&args)
            .output()
            .map_err(SettlementSubmitError::Spawn)?;
        if !output.status.success() {
            return Err(SettlementSubmitError::NonZeroExit {
                exit_code: output.status.code(),
                stdout: output.stdout,
                stderr: output.stderr,
            });
        }
        Ok(output.stdout)
    }
}

impl SettlementBackend for CardanoCliSettlementBackend {
    fn submit(
        &self,
        request: &SettlementRequest,
    ) -> Result<CardanoTxHashV1, SettlementSubmitError> {
        let stdout = self.spawn_txid(request)?;
        parse_cli_txid_stdout(&stdout)
    }
}

/// Parse the stdout of `cardano-cli transaction txid` into a
/// [`CardanoTxHashV1`].
///
/// The CLI emits the hex transaction id followed by a newline. Any
/// leading or trailing whitespace (including CR on Windows) is
/// trimmed before the strict hex parser runs; non-hex bodies are
/// rejected.
pub fn parse_cli_txid_stdout(stdout: &[u8]) -> Result<CardanoTxHashV1, SettlementSubmitError> {
    let text = core::str::from_utf8(stdout).map_err(|_| TxHashParseError::InvalidChar)?;
    let trimmed = text.trim();
    Ok(CardanoTxHashV1::from_hex(trimmed)?)
}

/// Absolute path of a per-process stub that a test backend can point
/// at; kept out of the production path by living only in tests.
#[allow(dead_code)]
fn _unused_path_helper(_: &Path) {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};

    const SAMPLE_HEX: &str = include_str!("../../../fixtures/cardano/tx_hash_sample.hex");

    fn sample_request() -> SettlementRequest {
        SettlementRequest {
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
            amount_lovelace: 700_000_000,
            intent_id: [0xAA; 32],
        }
    }

    // ---- CardanoTxHashV1 hex round-trip ----

    #[test]
    fn tx_hash_round_trips_via_hex_for_boundary_values() {
        for bytes in [[0u8; 32], [0xFF; 32], [0x12; 32], [0xAB; 32]] {
            let h = CardanoTxHashV1(bytes);
            let hex = h.to_hex();
            assert_eq!(hex.len(), 64);
            let parsed = CardanoTxHashV1::from_hex(&hex).expect("round-trip");
            assert_eq!(parsed, h);
        }
    }

    #[test]
    fn tx_hash_round_trips_for_sample_fixture() {
        let hex = SAMPLE_HEX.trim();
        assert_eq!(hex.len(), 64);
        let parsed = CardanoTxHashV1::from_hex(hex).expect("parse fixture");
        assert_eq!(parsed.to_hex(), hex);
    }

    #[test]
    fn tx_hash_from_hex_rejects_odd_length() {
        let err = CardanoTxHashV1::from_hex("a").expect_err("reject");
        assert_eq!(err, TxHashParseError::WrongLength);
    }

    #[test]
    fn tx_hash_from_hex_rejects_short_input() {
        let err = CardanoTxHashV1::from_hex(&"a".repeat(63)).expect_err("reject");
        assert_eq!(err, TxHashParseError::WrongLength);
    }

    #[test]
    fn tx_hash_from_hex_rejects_long_input() {
        let err = CardanoTxHashV1::from_hex(&"a".repeat(65)).expect_err("reject");
        assert_eq!(err, TxHashParseError::WrongLength);
    }

    #[test]
    fn tx_hash_from_hex_rejects_empty_input() {
        let err = CardanoTxHashV1::from_hex("").expect_err("reject");
        assert_eq!(err, TxHashParseError::WrongLength);
    }

    #[test]
    fn tx_hash_from_hex_rejects_non_hex_char() {
        let mut bad = String::from("g");
        bad.push_str(&"0".repeat(63));
        let err = CardanoTxHashV1::from_hex(&bad).expect_err("reject");
        assert_eq!(err, TxHashParseError::InvalidChar);
    }

    #[test]
    fn tx_hash_from_hex_rejects_uppercase() {
        let err = CardanoTxHashV1::from_hex(&"A".repeat(64)).expect_err("reject");
        assert_eq!(err, TxHashParseError::InvalidChar);
    }

    #[test]
    fn tx_hash_to_hex_is_lowercase() {
        let hex = CardanoTxHashV1([0xAB; 32]).to_hex();
        assert!(hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert_eq!(hex, "ab".repeat(32));
    }

    #[test]
    fn tx_hash_equality_is_byte_wise() {
        let a = CardanoTxHashV1([0x10; 32]);
        let b = CardanoTxHashV1([0x10; 32]);
        assert_eq!(a, b);
        let mut c_bytes = [0x10u8; 32];
        c_bytes[31] = 0x11;
        let c = CardanoTxHashV1(c_bytes);
        assert_ne!(a, c);
    }

    #[test]
    fn tx_hash_is_copy() {
        let a = CardanoTxHashV1([0x01; 32]);
        let b = a;
        assert_eq!(a, b);
    }

    // ---- parse_cli_txid_stdout ----

    #[test]
    fn parse_cli_txid_stdout_accepts_trailing_newline() {
        let line = format!("{}\n", "ab".repeat(32));
        let parsed = parse_cli_txid_stdout(line.as_bytes()).expect("parse");
        assert_eq!(parsed, CardanoTxHashV1([0xAB; 32]));
    }

    #[test]
    fn parse_cli_txid_stdout_accepts_leading_and_trailing_whitespace() {
        let line = format!("   {}  \r\n", "ab".repeat(32));
        let parsed = parse_cli_txid_stdout(line.as_bytes()).expect("parse");
        assert_eq!(parsed, CardanoTxHashV1([0xAB; 32]));
    }

    #[test]
    fn parse_cli_txid_stdout_rejects_short_body() {
        let err = parse_cli_txid_stdout(b"ab\n").expect_err("reject");
        assert!(matches!(
            err,
            SettlementSubmitError::ParseTxHash(TxHashParseError::WrongLength)
        ));
    }

    #[test]
    fn parse_cli_txid_stdout_rejects_non_hex_body() {
        let line = format!("{}\n", "g".repeat(64));
        let err = parse_cli_txid_stdout(line.as_bytes()).expect_err("reject");
        assert!(matches!(
            err,
            SettlementSubmitError::ParseTxHash(TxHashParseError::InvalidChar)
        ));
    }

    #[test]
    fn parse_cli_txid_stdout_rejects_non_utf8() {
        let err = parse_cli_txid_stdout(&[0xFF, 0xFE]).expect_err("reject");
        assert!(matches!(
            err,
            SettlementSubmitError::ParseTxHash(TxHashParseError::InvalidChar)
        ));
    }

    // ---- CardanoCliSettlementBackend helpers ----

    fn sample_backend() -> CardanoCliSettlementBackend {
        CardanoCliSettlementBackend {
            cardano_cli_bin: PathBuf::from("/usr/local/bin/cardano-cli"),
            network_args: vec!["--testnet-magic".to_string(), "2".to_string()],
            work_dir: PathBuf::from("/tmp/og/cardano"),
        }
    }

    #[test]
    fn signed_tx_path_is_deterministic_by_intent_id() {
        let backend = sample_backend();
        let mut a = sample_request();
        let mut b = sample_request();
        a.intent_id = [0x11; 32];
        b.intent_id = [0x11; 32];
        assert_eq!(
            backend.signed_tx_path_for(&a),
            backend.signed_tx_path_for(&b)
        );
    }

    #[test]
    fn signed_tx_path_differs_for_distinct_intent_ids() {
        let backend = sample_backend();
        let mut a = sample_request();
        let mut b = sample_request();
        a.intent_id = [0x11; 32];
        b.intent_id = [0x22; 32];
        assert_ne!(
            backend.signed_tx_path_for(&a),
            backend.signed_tx_path_for(&b)
        );
    }

    #[test]
    fn signed_tx_path_uses_lowercase_hex_intent_id() {
        let backend = sample_backend();
        let mut req = sample_request();
        req.intent_id = [0xAB; 32];
        let p = backend.signed_tx_path_for(&req);
        let expected = PathBuf::from("/tmp/og/cardano").join(format!("{}.signed", "ab".repeat(32)));
        assert_eq!(p, expected);
    }

    #[test]
    fn txid_cli_args_are_deterministic() {
        let backend = sample_backend();
        let req = sample_request();
        assert_eq!(backend.txid_cli_args(&req), backend.txid_cli_args(&req));
    }

    #[test]
    fn txid_cli_args_have_fixed_leading_tokens() {
        let args = sample_backend().txid_cli_args(&sample_request());
        assert_eq!(args[0], "transaction");
        assert_eq!(args[1], "txid");
        // Network args follow in order.
        assert_eq!(args[2], "--testnet-magic");
        assert_eq!(args[3], "2");
        assert_eq!(args[4], "--tx-file");
        assert!(args[5].ends_with(".signed"));
    }

    #[test]
    fn submit_surfaces_spawn_error_for_missing_binary() {
        let backend = CardanoCliSettlementBackend {
            cardano_cli_bin: PathBuf::from("/nonexistent/cardano/cli/binary"),
            network_args: vec![],
            work_dir: PathBuf::from("/tmp/og/cardano"),
        };
        let err = backend
            .submit(&sample_request())
            .expect_err("spawn must fail");
        match err {
            SettlementSubmitError::Spawn(_) => {}
            SettlementSubmitError::NonZeroExit { .. } => {
                panic!("expected Spawn, got NonZeroExit")
            }
            SettlementSubmitError::ParseTxHash(e) => {
                panic!("expected Spawn, got ParseTxHash({e:?})")
            }
        }
    }
}
