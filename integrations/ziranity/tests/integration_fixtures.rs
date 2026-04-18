#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! SD_01_D acceptance fixtures — canonical byte pairs.
//!
//! Each scenario ships two postcard files alongside this test:
//!
//! - `{scenario}_intent.postcard` — canonical `DisbursementIntentV1`
//!   bytes (what Ziranity commits as `IntentBodyV1::SubstrateCall.call`).
//! - `{scenario}_result.postcard` — canonical `AuthorizationResult`
//!   bytes the handler must produce when invoked with the intent
//!   above under the scenario's `CommitContext`.
//!
//! These byte pairs are the artifacts Ziranity embeds in
//! `crates/ion_runtime/tests/sd_01_*` (or equivalent) on their side
//! to assert that the full `submit → commit → dispatch → receipt`
//! path produces the expected output on every validator's replay.
//!
//! On the OracleGuard side this test file pins that:
//!
//! 1. The `DisbursementHandler` invoked against each scenario's intent
//!    and `CommitContext` produces **byte-identical** output to the
//!    checked-in result fixture.
//! 2. Repeat invocation produces byte-identical output every time
//!    (replay determinism at the handler boundary).
//!
//! Regeneration is gated behind `#[ignore]` tests — if the demo
//! intents change deliberately, run:
//!
//! ```text
//! cargo test --test integration_fixtures -- --ignored regenerate
//! ```
//!
//! and commit the regenerated `.postcard` files.

use std::path::PathBuf;

use ion_substrate_abi::{CommitContext, DomainHandler};

use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
use oracleguard_schemas::encoding::encode_intent;
use oracleguard_schemas::intent::{DisbursementIntentV1, INTENT_VERSION_V1};
use oracleguard_schemas::oracle::{
    OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
};

use oracleguard_ziranity_handler::{ConfigSnapshot, ConfigView, DisbursementHandler};

// ---- Demo scenario constants ----
//
// These align with oracleguard-adapter's evidence fixtures
// (allow_700_ada, deny_900_ada), Cluster 4's golden evaluator
// fixtures, and Cluster 5's closure-proofs. Same constants; different
// surface.

const POLICY_REF: [u8; 32] = [0x11; 32];
const ALLOCATION_ID: [u8; 32] = [0x22; 32];
const REQUESTER_ID: [u8; 32] = [0x33; 32];
const DESTINATION: CardanoAddressV1 = CardanoAddressV1 {
    bytes: [0x55; 57],
    length: 57,
};
const CREATION_MS: u64 = 1_713_000_000_000;
const EXPIRY_MS: u64 = 1_713_000_300_000;
const FRESH_NOW_MS: u64 = 1_713_000_100_000; // 100s into a 300s window

const ALLOW_AMOUNT: u64 = 700_000_000; // 700 ADA → within 75% cap
const DENY_AMOUNT: u64 = 900_000_000; // 900 ADA → exceeds 75% cap on 1000 ADA

const BLOCK_HEIGHT: u64 = 42;
const BLOCK_ID: [u8; 32] = [0x77; 32];

// ---- Test-only config wiring the sentinel ids ----
//
// The operator-facing `oracleguard-node.sample.toml` registers the
// real fixture `policy_ref` (56a7bb97…). These integration tests need
// the sentinel ids ([0x11; 32] etc.) that the evidence / evaluator
// fixtures use, so they construct their own config inline rather than
// reusing the operator sample.

fn test_config_toml() -> String {
    format!(
        r#"
[anchor]
registered_policy_refs = [ "{}" ]

[[registry.allocations]]
id                    = "{}"
basis_lovelace        = 1_000_000_000
authorized_subjects   = [ "{}" ]
approved_asset        = "ADA"
approved_destinations = [ {{ bytes_hex = "{}", length = 57 }} ]
"#,
        hex_32(&POLICY_REF),
        hex_32(&ALLOCATION_ID),
        hex_32(&REQUESTER_ID),
        hex_57(&DESTINATION.bytes),
    )
}

fn hex_32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_57(bytes: &[u8; 57]) -> String {
    let mut s = String::with_capacity(114);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn build_test_handler() -> DisbursementHandler<ConfigView, ConfigView> {
    let snap = ConfigSnapshot::from_toml_str(&test_config_toml()).expect("test config parses");
    let view = ConfigView::new(snap);
    DisbursementHandler::new(view.clone(), view)
}

// ---- Demo-intent builder ----

fn demo_intent(requested_amount_lovelace: u64) -> DisbursementIntentV1 {
    DisbursementIntentV1 {
        intent_version: INTENT_VERSION_V1,
        policy_ref: POLICY_REF,
        allocation_id: ALLOCATION_ID,
        requester_id: REQUESTER_ID,
        oracle_fact: OracleFactEvalV1 {
            asset_pair: ASSET_PAIR_ADA_USD,
            price_microusd: 450_000, // mid band
            source: SOURCE_CHARLI3,
        },
        oracle_provenance: OracleFactProvenanceV1 {
            timestamp_unix: CREATION_MS,
            expiry_unix: EXPIRY_MS,
            aggregator_utxo_ref: [0x44; 32],
        },
        requested_amount_lovelace,
        destination: DESTINATION,
        asset: AssetIdV1::ADA,
    }
}

fn fresh_ctx() -> CommitContext {
    CommitContext {
        now_unix_ms: FRESH_NOW_MS,
        block_height: BLOCK_HEIGHT,
        block_id: BLOCK_ID,
    }
}

fn stale_ctx() -> CommitContext {
    CommitContext {
        now_unix_ms: EXPIRY_MS, // at-expiry is stale
        block_height: BLOCK_HEIGHT,
        block_id: BLOCK_ID,
    }
}

// ---- Fixture paths ----

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("integration")
}

// ---- Checked-in fixtures ----

const ALLOW_700_ADA_INTENT: &[u8] =
    include_bytes!("../fixtures/integration/allow_700_ada_intent.postcard");
const ALLOW_700_ADA_RESULT: &[u8] =
    include_bytes!("../fixtures/integration/allow_700_ada_result.postcard");
const DENY_900_ADA_INTENT: &[u8] =
    include_bytes!("../fixtures/integration/deny_900_ada_intent.postcard");
const DENY_900_ADA_RESULT: &[u8] =
    include_bytes!("../fixtures/integration/deny_900_ada_result.postcard");
const STALE_ORACLE_INTENT: &[u8] =
    include_bytes!("../fixtures/integration/stale_oracle_intent.postcard");
const STALE_ORACLE_RESULT: &[u8] =
    include_bytes!("../fixtures/integration/stale_oracle_result.postcard");

// ---- Allow path ----

#[test]
fn allow_700_ada_intent_fixture_matches_encoded_demo_intent() {
    let encoded = encode_intent(&demo_intent(ALLOW_AMOUNT)).expect("encode intent");
    assert_eq!(encoded.as_slice(), ALLOW_700_ADA_INTENT);
}

#[test]
fn allow_700_ada_handler_output_matches_result_fixture() {
    let handler = build_test_handler();
    let out = handler
        .handle(ALLOW_700_ADA_INTENT, &fresh_ctx())
        .expect("handler succeeds on allow");
    assert_eq!(out.as_slice(), ALLOW_700_ADA_RESULT);
}

#[test]
fn allow_700_ada_result_decodes_to_authorized_700_ada_at_mid_band() {
    let decoded = AuthorizationResult::decode(ALLOW_700_ADA_RESULT).expect("decode result");
    match decoded {
        AuthorizationResult::Authorized { effect } => {
            assert_eq!(effect.authorized_amount_lovelace, ALLOW_AMOUNT);
            assert_eq!(effect.release_cap_basis_points, 7_500);
            assert_eq!(effect.destination, DESTINATION);
            assert_eq!(effect.asset, AssetIdV1::ADA);
            assert_eq!(effect.policy_ref, POLICY_REF);
            assert_eq!(effect.allocation_id, ALLOCATION_ID);
            assert_eq!(effect.requester_id, REQUESTER_ID);
        }
        AuthorizationResult::Denied { reason, gate } => {
            panic!("expected Authorized, got Denied({reason:?}, {gate:?})");
        }
    }
}

// ---- Deny path ----

#[test]
fn deny_900_ada_intent_fixture_matches_encoded_demo_intent() {
    let encoded = encode_intent(&demo_intent(DENY_AMOUNT)).expect("encode intent");
    assert_eq!(encoded.as_slice(), DENY_900_ADA_INTENT);
}

#[test]
fn deny_900_ada_handler_output_matches_result_fixture() {
    let handler = build_test_handler();
    let out = handler
        .handle(DENY_900_ADA_INTENT, &fresh_ctx())
        .expect("handler succeeds on deny");
    assert_eq!(out.as_slice(), DENY_900_ADA_RESULT);
}

#[test]
fn deny_900_ada_result_decodes_to_release_cap_exceeded_at_grant_gate() {
    use oracleguard_schemas::gate::AuthorizationGate;
    use oracleguard_schemas::reason::DisbursementReasonCode;

    let decoded = AuthorizationResult::decode(DENY_900_ADA_RESULT).expect("decode result");
    match decoded {
        AuthorizationResult::Denied { reason, gate } => {
            assert_eq!(reason, DisbursementReasonCode::ReleaseCapExceeded);
            assert_eq!(gate, AuthorizationGate::Grant);
        }
        AuthorizationResult::Authorized { .. } => {
            panic!("expected Denied, got Authorized");
        }
    }
}

// ---- Stale oracle path ----

#[test]
fn stale_oracle_intent_fixture_matches_encoded_demo_intent() {
    // Same intent as allow_700_ada — the difference is the CommitContext,
    // not the committed bytes.
    let encoded = encode_intent(&demo_intent(ALLOW_AMOUNT)).expect("encode intent");
    assert_eq!(encoded.as_slice(), STALE_ORACLE_INTENT);
    // Sanity: the stale-oracle intent is byte-identical to the allow
    // intent. The stale outcome is driven entirely by the ctx.
    assert_eq!(STALE_ORACLE_INTENT, ALLOW_700_ADA_INTENT);
}

#[test]
fn stale_oracle_handler_output_matches_result_fixture() {
    let handler = build_test_handler();
    let out = handler
        .handle(STALE_ORACLE_INTENT, &stale_ctx())
        .expect("handler succeeds on stale-oracle deny");
    assert_eq!(out.as_slice(), STALE_ORACLE_RESULT);
}

#[test]
fn stale_oracle_result_decodes_to_oracle_stale_at_grant_gate() {
    use oracleguard_schemas::gate::AuthorizationGate;
    use oracleguard_schemas::reason::DisbursementReasonCode;

    let decoded = AuthorizationResult::decode(STALE_ORACLE_RESULT).expect("decode result");
    match decoded {
        AuthorizationResult::Denied { reason, gate } => {
            assert_eq!(reason, DisbursementReasonCode::OracleStale);
            assert_eq!(gate, AuthorizationGate::Grant);
        }
        AuthorizationResult::Authorized { .. } => {
            panic!("expected Denied, got Authorized");
        }
    }
}

// ---- Replay determinism ----

#[test]
fn handler_output_is_byte_identical_across_repeated_invocations() {
    let handler = build_test_handler();
    let cases: &[(&[u8], CommitContext, &[u8])] = &[
        (ALLOW_700_ADA_INTENT, fresh_ctx(), ALLOW_700_ADA_RESULT),
        (DENY_900_ADA_INTENT, fresh_ctx(), DENY_900_ADA_RESULT),
        (STALE_ORACLE_INTENT, stale_ctx(), STALE_ORACLE_RESULT),
    ];

    for (intent_bytes, ctx, expected) in cases {
        // 8 iterations per case. Each iteration must produce the
        // same bytes as every other. Guards against any hidden
        // nondeterminism (HashMap iteration order, address layout,
        // wall-clock leakage) that would break Ziranity's
        // replay-equivalence contract.
        let mut last: Option<Vec<u8>> = None;
        for _ in 0..8 {
            let out = handler
                .handle(intent_bytes, ctx)
                .expect("handler succeeds on replay");
            assert_eq!(out.as_slice(), *expected);
            if let Some(ref prev) = last {
                assert_eq!(&out, prev);
            }
            last = Some(out);
        }
    }
}

#[test]
fn handler_output_is_byte_identical_across_freshly_constructed_handlers() {
    // Same intent, two different handler instances constructed from the
    // same config. Output must be byte-identical — handler construction
    // must not inject any entropy.
    let a = build_test_handler();
    let b = build_test_handler();

    let out_a = a
        .handle(ALLOW_700_ADA_INTENT, &fresh_ctx())
        .expect("handler a succeeds");
    let out_b = b
        .handle(ALLOW_700_ADA_INTENT, &fresh_ctx())
        .expect("handler b succeeds");
    assert_eq!(out_a, out_b);
}

// ---- Fixture regeneration (ignored) ----
//
// Run these with:
//   cargo test --test integration_fixtures -- --ignored regenerate
//
// Only needed when the demo scenarios deliberately change. After
// regenerating, diff the resulting `.postcard` files and commit the
// deltas alongside whatever code change motivated them.

#[test]
#[ignore = "regenerates fixtures; run manually only"]
fn regenerate_allow_700_ada_fixtures() {
    let intent_bytes = encode_intent(&demo_intent(ALLOW_AMOUNT)).expect("encode");
    let handler = build_test_handler();
    let result_bytes = handler
        .handle(&intent_bytes, &fresh_ctx())
        .expect("handler succeeds");
    std::fs::write(
        fixture_dir().join("allow_700_ada_intent.postcard"),
        &intent_bytes,
    )
    .expect("write intent");
    std::fs::write(
        fixture_dir().join("allow_700_ada_result.postcard"),
        &result_bytes,
    )
    .expect("write result");
}

#[test]
#[ignore = "regenerates fixtures; run manually only"]
fn regenerate_deny_900_ada_fixtures() {
    let intent_bytes = encode_intent(&demo_intent(DENY_AMOUNT)).expect("encode");
    let handler = build_test_handler();
    let result_bytes = handler
        .handle(&intent_bytes, &fresh_ctx())
        .expect("handler succeeds");
    std::fs::write(
        fixture_dir().join("deny_900_ada_intent.postcard"),
        &intent_bytes,
    )
    .expect("write intent");
    std::fs::write(
        fixture_dir().join("deny_900_ada_result.postcard"),
        &result_bytes,
    )
    .expect("write result");
}

#[test]
#[ignore = "regenerates fixtures; run manually only"]
fn regenerate_stale_oracle_fixtures() {
    let intent_bytes = encode_intent(&demo_intent(ALLOW_AMOUNT)).expect("encode");
    let handler = build_test_handler();
    let result_bytes = handler
        .handle(&intent_bytes, &stale_ctx())
        .expect("handler succeeds");
    std::fs::write(
        fixture_dir().join("stale_oracle_intent.postcard"),
        &intent_bytes,
    )
    .expect("write intent");
    std::fs::write(
        fixture_dir().join("stale_oracle_result.postcard"),
        &result_bytes,
    )
    .expect("write result");
}
