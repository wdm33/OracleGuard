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
use std::path::{Path, PathBuf};
use std::process::Command;

use oracleguard_schemas::encoding::{emit_payload, CanonicalEncodeError};
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::routing::DISBURSEMENT_OCU_ID;

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

/// Default `--wait-timeout` seconds passed to the CLI. 30 seconds
/// matches the Implementation Plan §14 convention.
pub const DEFAULT_WAIT_TIMEOUT_SECS: u32 = 30;

/// Default `--key` key id used when callers do not override. The
/// corresponding 32-byte Ed25519 seed must exist at
/// `${ZIRANITY_KEY_DIR}/${key_id}.key` with mode `0o600`.
///
/// A fixed default makes arg construction deterministic in tests; in
/// production the operator always sets this explicitly.
pub const DEFAULT_KEY_ID: &str = "oracleguard-operator";

/// Configuration bundle for one `ziranity intent submit` invocation.
///
/// Fields are `pub` (not `pub(crate)`) so integration tests and
/// operator tooling can assemble a config without needing a builder.
/// Every field that reaches the CLI does so verbatim — there is no
/// derived or hidden argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitConfig {
    /// Path to the file containing canonical `DisbursementIntentV1`
    /// postcard bytes emitted by [`write_payload_to_file`].
    pub payload_path: PathBuf,
    /// Operator Ed25519 key id. The CLI looks up
    /// `${ZIRANITY_KEY_DIR}/${key_id}.key`.
    pub key_id: String,
    /// Target authority OCU. Always
    /// [`oracleguard_schemas::routing::DISBURSEMENT_OCU_ID`] for the
    /// MVP, per the single-authority rule; surfaced as a field so the
    /// wrapper does not silently override caller intent if a future
    /// sharding slice reopens authority routing.
    pub object_id: [u8; 32],
    /// MCP endpoint host for a devnet / production validator.
    pub node_host: String,
    /// MCP endpoint port. Convention: `base_port + 1000` (e.g.
    /// validator 0 on base 9400 exposes MCP on 10400).
    pub node_port: u16,
    /// Path to the bootstrap bundle produced by
    /// `ziranity devnet start` (default `<data_dir>/bootstrap.bundle`)
    /// or the production equivalent.
    pub bootstrap_bundle_path: PathBuf,
    /// If true, pass `--wait` to block until a consensus receipt is
    /// available. The MVP always blocks; the field is surfaced so
    /// async-submission code paths do not need a separate config type.
    pub wait: bool,
    /// `--wait-timeout <seconds>`. Only emitted when `wait` is true.
    pub wait_timeout_secs: u32,
    /// Feature-gated TLS bypass for devnet only. Must NOT be set in
    /// production.
    pub devnet_insecure: bool,
}

impl SubmitConfig {
    /// Build a config with MVP defaults: `object_id` = the fixed
    /// disbursement OCU, `key_id` = [`DEFAULT_KEY_ID`], `wait` = true,
    /// `wait_timeout_secs` = [`DEFAULT_WAIT_TIMEOUT_SECS`],
    /// `devnet_insecure` = false.
    ///
    /// Caller supplies the inputs the default cannot know: payload
    /// path, node endpoint, and bootstrap bundle path.
    pub fn new_mvp(
        payload_path: PathBuf,
        node_host: String,
        node_port: u16,
        bootstrap_bundle_path: PathBuf,
    ) -> Self {
        Self {
            payload_path,
            key_id: DEFAULT_KEY_ID.to_string(),
            object_id: DISBURSEMENT_OCU_ID,
            node_host,
            node_port,
            bootstrap_bundle_path,
            wait: true,
            wait_timeout_secs: DEFAULT_WAIT_TIMEOUT_SECS,
            devnet_insecure: false,
        }
    }
}

/// Render a 32-byte object id as a 64-char lowercase hex string.
fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        let hi = b >> 4;
        let lo = b & 0x0F;
        out.push(hex_nibble(hi));
        out.push(hex_nibble(lo));
    }
    out
}

fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => char::from(b'0' + n),
        10..=15 => char::from(b'a' + n - 10),
        _ => unreachable!(),
    }
}

/// Deterministically build the `ziranity intent submit` argument vector
/// for the given config.
///
/// The argument order is fixed (it mirrors the Implementation Plan
/// §14 example and the Cluster 6 Slice 04 slice doc). Same config in,
/// same `Vec<String>` out, always — the golden fixture
/// `fixtures/routing/sample_cli_args.golden.txt` pins this output for
/// a representative config.
///
/// This function is pure GREEN glue: it does no I/O and touches no
/// global state.
pub fn build_cli_args(config: &SubmitConfig) -> Vec<String> {
    let mut args = Vec::with_capacity(16);
    args.push("intent".to_string());
    args.push("submit".to_string());
    args.push("--payload".to_string());
    args.push(config.payload_path.to_string_lossy().into_owned());
    args.push("--key".to_string());
    args.push(config.key_id.clone());
    args.push("--object_id".to_string());
    args.push(hex_encode(&config.object_id));
    args.push("--node".to_string());
    args.push(format!("{}:{}", config.node_host, config.node_port));
    args.push("--bootstrap-bundle".to_string());
    args.push(config.bootstrap_bundle_path.to_string_lossy().into_owned());
    if config.wait {
        args.push("--wait".to_string());
        args.push("--wait-timeout".to_string());
        args.push(config.wait_timeout_secs.to_string());
    }
    if config.devnet_insecure {
        args.push("--devnet-insecure".to_string());
    }
    args
}

/// Error returned by [`submit_intent`].
#[derive(Debug)]
pub enum SubmitError {
    /// Spawning the `ziranity` binary failed (not found on PATH, etc).
    Spawn(io::Error),
    /// The CLI process ran but exited non-zero. `stdout` and `stderr`
    /// are surfaced verbatim so the caller can diagnose without this
    /// module trying to interpret them.
    NonZeroExit {
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

/// Raw stdout bytes returned by a successful `ziranity intent submit`
/// invocation.
///
/// Intake/parse of these bytes into a typed consensus receipt is the
/// job of Cluster 6 Slice 05, not this slice. Returning opaque bytes
/// here keeps the submission-wrapper boundary strictly shell-level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCliStdout {
    pub bytes: Vec<u8>,
}

/// Spawn `ziranity intent submit` with the arguments derived from
/// `config` and wait for it to exit.
///
/// This is the RED invocation boundary: everything above it is pure
/// argument construction, everything below it is the Ziranity CLI and
/// the MCP transport. On success returns the CLI's raw stdout bytes
/// for later intake parsing (Slice 05). On non-zero exit surfaces the
/// exit code plus stdout/stderr without interpretation.
///
/// `ziranity_bin` is the path to the `ziranity` binary. Passed
/// explicitly so tests can point at a fake binary and production can
/// resolve against PATH or an absolute path.
pub fn submit_intent(
    ziranity_bin: &Path,
    config: &SubmitConfig,
) -> Result<RawCliStdout, SubmitError> {
    let args = build_cli_args(config);
    let output = Command::new(ziranity_bin)
        .args(&args)
        .output()
        .map_err(SubmitError::Spawn)?;
    if !output.status.success() {
        return Err(SubmitError::NonZeroExit {
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    Ok(RawCliStdout {
        bytes: output.stdout,
    })
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
        let err =
            write_payload_to_file(&intent, path).expect_err("missing parent directory must error");
        match err {
            WritePayloadError::Io(_) => {}
            WritePayloadError::Encode(e) => panic!("expected Io error, got Encode({e:?})"),
        }
    }

    // ---- Cluster 6 Slice 04 — CLI wrapper boundary ----

    fn sample_config() -> SubmitConfig {
        SubmitConfig::new_mvp(
            PathBuf::from("/tmp/og/intent.payload"),
            "127.0.0.1".to_string(),
            10_400,
            PathBuf::from("/tmp/og/devnet/bootstrap.bundle"),
        )
    }

    #[test]
    fn mvp_defaults_are_pinned() {
        let c = sample_config();
        assert_eq!(c.object_id, DISBURSEMENT_OCU_ID);
        assert_eq!(c.key_id, DEFAULT_KEY_ID);
        assert!(c.wait);
        assert_eq!(c.wait_timeout_secs, DEFAULT_WAIT_TIMEOUT_SECS);
        assert!(!c.devnet_insecure);
    }

    #[test]
    fn build_cli_args_is_deterministic() {
        let c = sample_config();
        let a = build_cli_args(&c);
        let b = build_cli_args(&c);
        assert_eq!(a, b);
    }

    #[test]
    fn build_cli_args_contains_expected_flag_order() {
        let args = build_cli_args(&sample_config());
        // Leading subcommand tokens are fixed.
        assert_eq!(&args[0..2], &["intent".to_string(), "submit".to_string()]);
        // Core flags appear in fixed positions so the golden fixture is
        // checkable and reviewers can audit shape at a glance.
        assert_eq!(args[2], "--payload");
        assert_eq!(args[4], "--key");
        assert_eq!(args[6], "--object_id");
        assert_eq!(args[8], "--node");
        assert_eq!(args[10], "--bootstrap-bundle");
        assert_eq!(args[12], "--wait");
        assert_eq!(args[13], "--wait-timeout");
    }

    #[test]
    fn build_cli_args_encodes_object_id_as_64_char_lowercase_hex() {
        let args = build_cli_args(&sample_config());
        let hex = &args[7];
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|ch| matches!(ch, '0'..='9' | 'a'..='f')));
        assert_eq!(hex, &hex_encode(&DISBURSEMENT_OCU_ID));
    }

    #[test]
    fn build_cli_args_node_is_host_colon_port() {
        let args = build_cli_args(&sample_config());
        assert_eq!(args[9], "127.0.0.1:10400");
    }

    #[test]
    fn build_cli_args_omits_wait_flags_when_wait_false() {
        let mut c = sample_config();
        c.wait = false;
        let args = build_cli_args(&c);
        assert!(!args.iter().any(|a| a == "--wait"));
        assert!(!args.iter().any(|a| a == "--wait-timeout"));
    }

    #[test]
    fn build_cli_args_emits_devnet_insecure_only_when_set() {
        let mut c = sample_config();
        assert!(!build_cli_args(&c).iter().any(|a| a == "--devnet-insecure"));
        c.devnet_insecure = true;
        assert!(build_cli_args(&c).iter().any(|a| a == "--devnet-insecure"));
    }

    #[test]
    fn build_cli_args_payload_change_isolates_to_payload_field() {
        let a = build_cli_args(&sample_config());
        let mut c2 = sample_config();
        c2.payload_path = PathBuf::from("/tmp/og/OTHER.payload");
        let b = build_cli_args(&c2);
        // Only the position-3 value (the --payload argument value) may
        // change between the two arg vectors.
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 3 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only payload changes");
            }
        }
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn build_cli_args_object_id_change_isolates_to_object_id_field() {
        let a = build_cli_args(&sample_config());
        let mut c2 = sample_config();
        c2.object_id = [0xAB; 32];
        let b = build_cli_args(&c2);
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 7 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only object_id changes");
            }
        }
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn build_cli_args_key_change_isolates_to_key_field() {
        let a = build_cli_args(&sample_config());
        let mut c2 = sample_config();
        c2.key_id = "other-operator".to_string();
        let b = build_cli_args(&c2);
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 5 {
                assert_ne!(x, y);
            } else {
                assert_eq!(x, y, "arg {i} must not change when only key changes");
            }
        }
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn build_cli_args_matches_golden_fixture() {
        // Pins the full argument vector for a representative config to
        // fixtures/routing/sample_cli_args.golden.txt. One line per
        // argument token.
        const GOLDEN: &str = include_str!("../../../fixtures/routing/sample_cli_args.golden.txt");
        let args = build_cli_args(&sample_config());
        let rendered = args.join("\n");
        assert_eq!(rendered.trim_end(), GOLDEN.trim_end());
    }

    #[test]
    fn hex_encode_round_trips_zero_and_all_ones() {
        assert_eq!(hex_encode(&[0u8; 32]), "0".repeat(64));
        assert_eq!(hex_encode(&[0xFF; 32]), "f".repeat(64));
    }

    #[test]
    fn submit_intent_surfaces_spawn_error_for_missing_binary() {
        // Pointing the wrapper at a path that does not exist must
        // surface SubmitError::Spawn rather than panicking or silently
        // succeeding.
        let bin = Path::new("/nonexistent/ziranity/binary/does_not_exist");
        let err = submit_intent(bin, &sample_config()).expect_err("missing binary must error");
        match err {
            SubmitError::Spawn(_) => {}
            SubmitError::NonZeroExit { .. } => panic!("expected Spawn, got NonZeroExit"),
        }
    }
}
