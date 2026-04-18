//! Cardano disbursement settlement backend — pycardano + Ogmios v6.
//!
//! Owns: the imperative shell that spawns a Python helper
//! (`scripts/cardano_disburse.py`) to build, sign, and submit the
//! Cardano transaction matching an [`AuthorizedEffectV1`].
//!
//! Closes the third and final shell gap from the demo-readiness
//! audit: the adapter now has end-to-end plumbing for the full
//! governed-disbursement loop (Charli3 trigger → Kupo fetch →
//! Ziranity consensus → Cardano tx).
//!
//! ## Why Python + Ogmios
//!
//! The hackathon provides a synced Ogmios v6 instance on Preprod at
//! <http://35.209.192.203:1337>, and the Charli3 client already
//! shelled out from [`crate::charli3_aggregate`] is itself pycardano-
//! backed. Using pycardano for settlement keeps the operator-side
//! toolchain consistent: one Python dependency set
//! (`pip install pycardano ogmios charli3-pull-oracle-client`) covers
//! both aggregation and settlement. No `cardano-node`, no local
//! chain sync, no `cardano-cli` — just a synced hackathon endpoint
//! and two mnemonics.
//!
//! For a production deployment the operator can swap this backend
//! for a different [`SettlementBackend`] implementation without
//! touching OracleGuard's canonical surface; the trait boundary in
//! [`crate::cardano`] is the swap point.
//!
//! ## How the pieces fit
//!
//! Rust side (this module):
//! - [`PyCardanoDisburseBackend`] holds the static operator config
//!   (python binary path, helper script path, Ogmios URL, pool
//!   wallet bech32 address).
//! - [`build_submit_args`] converts a per-call [`SettlementRequest`]
//!   into the argument vector the Python helper consumes. Pure and
//!   golden-fixture pinned.
//! - [`SettlementBackend::submit`] spawns `python3 <helper> <args>`,
//!   inherits `POOL_MNEMONIC` from the environment, waits for the
//!   child to exit, and parses the 64-char hex `tx_id` from stdout
//!   via the existing [`crate::cardano::parse_cli_txid_stdout`].
//!
//! Python side (`scripts/cardano_disburse.py`, 150 LOC):
//! - Reads `POOL_MNEMONIC` from env; derives the Shelley payment
//!   signing key at BIP-44 path `m/1852'/1815'/0'/0/0`.
//! - Opens an [`OgmiosV6ChainContext`] against the supplied URL.
//! - Builds a tx with `add_input_address(pool)` +
//!   `add_output(TransactionOutput(destination, amount))`, attaches
//!   the OracleGuard `intent_id` as metadata label 674, signs,
//!   submits, and prints the tx_id to stdout.
//!
//! ## Wallet roles
//!
//! See `docs/clusters/preprod_wallets.md` for the full three-wallet
//! role map. Summary:
//!
//! - **Pool** (`wallet_1`) — `tx_in` source. `POOL_MNEMONIC` env
//!   var. Also the `change_address` so leftover lovelace returns to
//!   the pool.
//! - **Operator** (`wallet_2`) — signs Charli3 aggregation, NOT this
//!   disbursement tx. `WALLET_MNEMONIC` env var — a distinct env
//!   variable from `POOL_MNEMONIC`. The two never collide.
//! - **Receiver** (`wallet_3`) — the authorized effect's
//!   `destination`. Receives funds only; no mnemonic role.
//!
//! ## What this module does NOT own
//!
//! - **The tx construction logic itself.** That lives in the Python
//!   helper because pycardano is the battle-tested library for it;
//!   re-implementing in Rust would be weeks of UTxO-selection and
//!   fee-calculation work for no operational benefit.
//! - **Chain queries.** Ogmios is queried by pycardano on the Python
//!   side; this module never sees UTxO data directly.
//! - **Metadata semantics.** The `intent_id` in metadata label 674
//!   is attribution-only. OracleGuard's canonical-byte surfaces
//!   (intent, effect, evidence) don't consult tx metadata; it exists
//!   purely so an on-chain auditor can link a tx to its originating
//!   OracleGuard intent.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cardano::{parse_cli_txid_stdout, CardanoTxHashV1, SettlementBackend, SettlementSubmitError};
use crate::settlement::SettlementRequest;

/// Configuration for a [`PyCardanoDisburseBackend`].
///
/// All fields are `pub` so operator tooling can construct a backend
/// without a builder. Fields that change per-request (destination,
/// amount, intent_id) come from the [`SettlementRequest`] passed to
/// [`SettlementBackend::submit`] — they are not stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyCardanoDisburseBackend {
    /// Path to the `python3` binary. Can be an absolute path or a
    /// bare name resolved against PATH. For venvs, point directly at
    /// the venv's python (e.g. `.venv/bin/python3`) so the helper
    /// picks up the right pycardano install.
    pub python_bin: PathBuf,
    /// Path to the disbursement helper script, typically
    /// `scripts/cardano_disburse.py` in this repo.
    pub helper_script: PathBuf,
    /// Ogmios v6 endpoint URL (e.g. `http://35.209.192.203:1337`).
    /// Forwarded verbatim to the Python helper as `--ogmios-url`.
    pub ogmios_url: String,
    /// Bech32 address of the pool wallet — the `tx_in` source and
    /// the `change_address`. Forwarded verbatim as `--pool-address`.
    pub pool_address: String,
}

impl PyCardanoDisburseBackend {
    /// Construct a backend with the hackathon Ogmios v6 endpoint
    /// pre-wired. Caller supplies the python binary path, helper
    /// script path, and pool wallet bech32 address.
    #[must_use]
    pub fn new_hackathon_preprod(
        python_bin: PathBuf,
        helper_script: PathBuf,
        pool_address: impl Into<String>,
    ) -> Self {
        Self {
            python_bin,
            helper_script,
            ogmios_url: crate::kupo::HACKATHON_PREPROD_BASE_URL
                .replace(":1442", ":1337"),
            pool_address: pool_address.into(),
        }
    }
}

/// Deterministically build the argument vector passed to
/// `scripts/cardano_disburse.py`.
///
/// Pure: no I/O, no env reads, no globals. Same backend + same
/// request in, same `Vec<String>` out, always. A golden fixture
/// (`fixtures/cardano/sample_disburse_cli_args.golden.txt`) pins a
/// representative invocation byte-for-byte so argument shape drift
/// fails CI rather than shipping silently.
///
/// Flag order is fixed:
///
/// 1. `<helper_script>` (positional — python's first argv after the
///    interpreter)
/// 2. `--ogmios-url <url>`
/// 3. `--pool-address <bech32>`
/// 4. `--destination-bytes-hex <114-hex>`
/// 5. `--destination-length <u8>`
/// 6. `--amount-lovelace <u64>`
/// 7. `--intent-id <64-hex>`
///
/// The destination bytes are emitted as the full 57-byte buffer (not
/// trimmed to `length`) so the helper receives byte-identical input
/// regardless of address width. The helper trims to `length` on its
/// side.
#[must_use]
pub fn build_submit_args(
    backend: &PyCardanoDisburseBackend,
    request: &SettlementRequest,
) -> Vec<String> {
    vec![
        backend.helper_script.to_string_lossy().into_owned(),
        "--ogmios-url".to_string(),
        backend.ogmios_url.clone(),
        "--pool-address".to_string(),
        backend.pool_address.clone(),
        "--destination-bytes-hex".to_string(),
        hex_encode_57(&request.destination.bytes),
        "--destination-length".to_string(),
        request.destination.length.to_string(),
        "--amount-lovelace".to_string(),
        request.amount_lovelace.to_string(),
        "--intent-id".to_string(),
        hex_encode_32(&request.intent_id),
    ]
}

fn hex_encode_57(bytes: &[u8; 57]) -> String {
    let mut s = String::with_capacity(114);
    for b in bytes {
        s.push(nibble_to_hex(b >> 4));
        s.push(nibble_to_hex(b & 0x0F));
    }
    s
}

fn hex_encode_32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push(nibble_to_hex(b >> 4));
        s.push(nibble_to_hex(b & 0x0F));
    }
    s
}

fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => char::from(b'0' + n),
        10..=15 => char::from(b'a' + n - 10),
        _ => unreachable!(),
    }
}

impl PyCardanoDisburseBackend {
    fn spawn_helper(&self, request: &SettlementRequest) -> Result<Vec<u8>, SettlementSubmitError> {
        let args = build_submit_args(self, request);
        let output = Command::new(&self.python_bin)
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

impl SettlementBackend for PyCardanoDisburseBackend {
    fn submit(
        &self,
        request: &SettlementRequest,
    ) -> Result<CardanoTxHashV1, SettlementSubmitError> {
        let stdout = self.spawn_helper(request)?;
        parse_cli_txid_stdout(&stdout)
    }
}

/// Resolve the Python helper script's path relative to the workspace
/// root. Convenience for operator tooling: given the workspace
/// directory, compute the path to `scripts/cardano_disburse.py`.
///
/// Pure; no filesystem check. Caller is responsible for verifying
/// the file exists before constructing a
/// [`PyCardanoDisburseBackend`].
#[must_use]
pub fn default_helper_script_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join("scripts").join("cardano_disburse.py")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};

    fn sample_backend() -> PyCardanoDisburseBackend {
        PyCardanoDisburseBackend {
            python_bin: PathBuf::from("/usr/bin/python3"),
            helper_script: PathBuf::from("/home/op/oracleguard/scripts/cardano_disburse.py"),
            ogmios_url: "http://35.209.192.203:1337".to_string(),
            pool_address: "addr_test1qz4f2vac8nn7tp802mxj3cu40a7xhhzc3agut6spq6rpz5rgtlvyed9yn3ncuv3fgaadfmvn64d7egjn824t7pj99xfs4y58d0".to_string(),
        }
    }

    fn sample_request() -> SettlementRequest {
        SettlementRequest {
            destination: CardanoAddressV1 {
                // wallet_3 (receiver) raw bytes; full 57 bytes, header 0x00.
                bytes: [
                    0x00, 0x0e, 0xe0, 0x3e, 0x45, 0xb0, 0x5d, 0x62, 0x25, 0xeb, 0x71, 0x43, 0xd2,
                    0xbe, 0x23, 0xa3, 0xb8, 0x62, 0x9b, 0x4d, 0x9e, 0x61, 0x62, 0x1b, 0xb3, 0x0a,
                    0xfc, 0xc5, 0x3a, 0x95, 0x47, 0x4e, 0x79, 0xd1, 0x29, 0x8d, 0x29, 0x1e, 0x20,
                    0x0a, 0x2c, 0x88, 0x9f, 0xe1, 0x3b, 0x97, 0x78, 0x5c, 0xc4, 0x12, 0x79, 0x62,
                    0x9d, 0x99, 0xcf, 0xb8, 0x7d,
                ],
                length: 57,
            },
            asset: AssetIdV1::ADA,
            amount_lovelace: 700_000_000,
            intent_id: [0x77; 32],
        }
    }

    // ---- new_hackathon_preprod ----

    #[test]
    fn hackathon_preprod_constructor_wires_ogmios_port() {
        let b = PyCardanoDisburseBackend::new_hackathon_preprod(
            PathBuf::from("/usr/bin/python3"),
            PathBuf::from("/tmp/cardano_disburse.py"),
            "addr_test1qz...",
        );
        assert_eq!(b.ogmios_url, "http://35.209.192.203:1337");
    }

    // ---- build_submit_args: determinism ----

    #[test]
    fn build_submit_args_is_deterministic() {
        let a = build_submit_args(&sample_backend(), &sample_request());
        let b = build_submit_args(&sample_backend(), &sample_request());
        assert_eq!(a, b);
    }

    // ---- build_submit_args: flag order ----

    #[test]
    fn build_submit_args_emits_expected_flag_order() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        // args[0] is the helper-script path (python's first argv).
        assert_eq!(args[1], "--ogmios-url");
        assert_eq!(args[3], "--pool-address");
        assert_eq!(args[5], "--destination-bytes-hex");
        assert_eq!(args[7], "--destination-length");
        assert_eq!(args[9], "--amount-lovelace");
        assert_eq!(args[11], "--intent-id");
    }

    #[test]
    fn build_submit_args_helper_script_is_first_positional() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        assert_eq!(args[0], "/home/op/oracleguard/scripts/cardano_disburse.py");
    }

    // ---- build_submit_args: per-field encoding ----

    #[test]
    fn build_submit_args_encodes_destination_bytes_as_114_char_hex() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        let hex = &args[6];
        assert_eq!(hex.len(), 114);
        assert!(hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        // First four bytes match wallet_3's header + prefix.
        assert!(hex.starts_with("000ee03e"));
    }

    #[test]
    fn build_submit_args_destination_bytes_are_full_57_regardless_of_length() {
        // Even if `length < 57`, the full 57-byte buffer is emitted.
        // The Python helper trims to `length` on its side. This keeps
        // the CLI shape stable across address widths.
        let mut req = sample_request();
        req.destination.length = 29; // enterprise-address length
        let args = build_submit_args(&sample_backend(), &req);
        let hex = &args[6];
        assert_eq!(hex.len(), 114);
    }

    #[test]
    fn build_submit_args_encodes_intent_id_as_64_char_hex() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        let hex = &args[12];
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, &"77".repeat(32));
    }

    #[test]
    fn build_submit_args_stringifies_amount_lovelace_as_decimal() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        assert_eq!(args[10], "700000000");
    }

    #[test]
    fn build_submit_args_stringifies_length_as_decimal() {
        let args = build_submit_args(&sample_backend(), &sample_request());
        assert_eq!(args[8], "57");
    }

    // ---- build_submit_args: per-field isolation ----

    #[test]
    fn build_submit_args_amount_change_isolates_to_amount_position() {
        let a = build_submit_args(&sample_backend(), &sample_request());
        let mut req2 = sample_request();
        req2.amount_lovelace = 900_000_000;
        let b = build_submit_args(&sample_backend(), &req2);
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 10 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only amount changes");
            }
        }
    }

    #[test]
    fn build_submit_args_intent_id_change_isolates_to_intent_id_position() {
        let a = build_submit_args(&sample_backend(), &sample_request());
        let mut req2 = sample_request();
        req2.intent_id = [0xAA; 32];
        let b = build_submit_args(&sample_backend(), &req2);
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 12 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only intent_id changes");
            }
        }
    }

    #[test]
    fn build_submit_args_ogmios_url_change_isolates_to_url_position() {
        let a = build_submit_args(&sample_backend(), &sample_request());
        let mut b2 = sample_backend();
        b2.ogmios_url = "http://localhost:1337".to_string();
        let b = build_submit_args(&b2, &sample_request());
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 2 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only ogmios_url changes");
            }
        }
    }

    // ---- Golden fixture ----

    #[test]
    fn build_submit_args_matches_golden_fixture() {
        const GOLDEN: &str =
            include_str!("../../../fixtures/cardano/sample_disburse_cli_args.golden.txt");
        let args = build_submit_args(&sample_backend(), &sample_request());
        let rendered = args.join("\n");
        assert_eq!(rendered.trim_end(), GOLDEN.trim_end());
    }

    // ---- Spawn-failure surfacing ----

    #[test]
    fn submit_surfaces_spawn_error_for_missing_python_bin() {
        let backend = PyCardanoDisburseBackend {
            python_bin: PathBuf::from("/nonexistent/python/binary/does_not_exist"),
            helper_script: PathBuf::from("/tmp/cardano_disburse.py"),
            ogmios_url: "http://35.209.192.203:1337".to_string(),
            pool_address: "addr_test1qz...".to_string(),
        };
        let err = backend
            .submit(&sample_request())
            .expect_err("missing python must error");
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

    // ---- Path helper ----

    #[test]
    fn default_helper_script_path_builds_scripts_subpath() {
        let p = default_helper_script_path(Path::new("/home/op/oracleguard"));
        assert_eq!(
            p,
            PathBuf::from("/home/op/oracleguard/scripts/cardano_disburse.py")
        );
    }

    // ---- Hex encoders ----

    #[test]
    fn hex_encode_57_handles_all_zero_and_all_ff() {
        assert_eq!(hex_encode_57(&[0u8; 57]), "0".repeat(114));
        assert_eq!(hex_encode_57(&[0xFFu8; 57]), "f".repeat(114));
    }

    #[test]
    fn hex_encode_32_handles_all_zero_and_all_ff() {
        assert_eq!(hex_encode_32(&[0u8; 32]), "0".repeat(64));
        assert_eq!(hex_encode_32(&[0xFFu8; 32]), "f".repeat(64));
    }
}
