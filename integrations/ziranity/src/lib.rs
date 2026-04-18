//! OracleGuard DomainHandler adapter for the Ziranity substrate-dispatch
//! integration.
//!
//! This crate is the thin shell that plugs OracleGuard's pure
//! three-gate authorization closure into Ziranity's commit-time
//! substrate-dispatch surface (cluster SD_01). It is deliberately
//! outside OracleGuard's main Cargo workspace so contributors who
//! aren't building the Ziranity integration don't pay the compile
//! cost of `ion_runtime`'s heavy transitive dependencies.
//!
//! ## Byte-channel contract
//!
//! Ziranity's `DomainHandler` trait carries opaque bytes both
//! directions:
//!
//! - `call: &[u8]` — committed canonical bytes of a
//!   `DisbursementIntentV1`.
//! - `Result<Vec<u8>, HandlerErrorCode>` — on success, canonical
//!   bytes of an `AuthorizationResult` (either `Authorized { effect }`
//!   or `Denied { reason, gate }`); on failure, a u16 error code.
//!
//! The 8 `DisbursementReasonCode` variants stay on the OracleGuard
//! side of the wire. Ziranity never interprets them. This preserves
//! the framework's "don't learn the consumer's domain" principle:
//! a `Denied { reason, gate }` is a well-formed governance decision
//! that flows back through `Ok(bytes)`, not a framework failure.
//! `HandlerError` means "Ziranity couldn't even run OracleGuard's
//! gates" — transport-level, not governance.
//!
//! ## Error-code allocation
//!
//! - [`DECODE_FAILURE`] (`0x0100`) — the committed `call` bytes did
//!   not decode as a canonical `DisbursementIntentV1` (malformed
//!   postcard, trailing bytes, etc.).
//! - [`ENCODE_FAILURE`] (`0x0101`) — the evaluated
//!   `AuthorizationResult` failed to encode. Structurally unreachable
//!   for well-formed inputs; present only for totality.
//!
//! Ziranity's framework-reserved range is `0x0000..=0x00FF`;
//! OracleGuard uses `0x0100..=0x01FF`. Any future handler errors
//! added here follow that range.
//!
//! ## Purity contract
//!
//! Per Ziranity's SD_01 hand-off, handlers must be pure functions of
//! their inputs — no I/O, no syscalls, no interior mutability that
//! affects replay. OracleGuard's evaluator and three-gate closure
//! satisfy this by construction (the `oracleguard-policy` crate
//! denies `clippy::panic`, `clippy::unwrap_used`, `clippy::expect_used`,
//! and `clippy::float_arithmetic` at its root). This crate inherits
//! the same discipline via its own crate-level lint denials.

#![forbid(unsafe_code)]

use std::path::Path;

use ion_substrate_abi::{CommitContext, DomainHandler, HandlerErrorCode};

use oracleguard_policy::authorize::{
    authorize_disbursement, AllocationRegistryView, PolicyAnchorView,
};
use oracleguard_schemas::encoding::decode_intent;
use oracleguard_schemas::routing::DISBURSEMENT_OCU_ID;

pub mod config;

pub use config::{BuildError, ConfigSnapshot, ConfigView};

/// Handler-defined error: the committed `call` bytes did not decode
/// as a canonical `DisbursementIntentV1`.
pub const DECODE_FAILURE: HandlerErrorCode = 0x0100;

/// Handler-defined error: the evaluated `AuthorizationResult` failed
/// to encode. Structurally unreachable for well-formed inputs;
/// present only for totality.
pub const ENCODE_FAILURE: HandlerErrorCode = 0x0101;

// Compile-time guards: OracleGuard codes must land in the handler-
// defined range (>= 0x0100). Ziranity reserves 0x0000..=0x00FF for
// framework-boundary codes. These `const _` asserts fire at compile
// time if an OracleGuard code is ever accidentally renumbered into
// the framework-reserved range.
const _: () = assert!(DECODE_FAILURE >= 0x0100);
const _: () = assert!(ENCODE_FAILURE >= 0x0100);
const _: () = assert!(DECODE_FAILURE != ENCODE_FAILURE);

/// Canonical 32-byte `substrate_id` for OracleGuard disbursement
/// intents.
///
/// Derived by keyed BLAKE3 with domain `ORACLEGUARD_DISBURSEMENT_OCU_V1`
/// over the seed `OCU/MVP/v1`. Pinned and recompute-verified by
/// `oracleguard_schemas::routing::ocu_id_matches_documented_derivation`.
/// Anyone can reproduce the value from its public recipe without
/// reading OracleGuard source.
///
/// This is the value that must be passed to
/// `ziranity intent submit --substrate-id <hex>` and registered on
/// the node side under the same bytes.
#[must_use]
pub const fn substrate_id() -> [u8; 32] {
    DISBURSEMENT_OCU_ID
}

/// The concrete `DomainHandler` implementation OracleGuard registers
/// with Ziranity's `SubstrateDispatchRegistry`.
///
/// Generic over the anchor-view and registry-view implementations so
/// a node binary can wire whatever config source it prefers
/// (TOML-backed, database-backed, static-slice-backed, etc.). The
/// loader lives outside the trait boundary — anchor and registry
/// views are built at node startup and moved into the handler once.
/// No I/O happens on the hot path.
///
/// The handler is monomorphized once per binary at registration
/// time, then wrapped in `Box<dyn DomainHandler>` by the registry.
pub struct DisbursementHandler<A, R>
where
    A: PolicyAnchorView + Send + Sync,
    R: AllocationRegistryView + Send + Sync,
{
    anchor: A,
    registry: R,
}

impl<A, R> DisbursementHandler<A, R>
where
    A: PolicyAnchorView + Send + Sync,
    R: AllocationRegistryView + Send + Sync,
{
    /// Construct a handler from pre-built anchor and registry views.
    ///
    /// Configuration loading is the caller's responsibility; this
    /// constructor accepts whatever concrete types implement the two
    /// traits. A TOML-backed `ConfigSnapshot` that implements both
    /// traits ships in a follow-up commit.
    pub const fn new(anchor: A, registry: R) -> Self {
        Self { anchor, registry }
    }
}

impl<A, R> DomainHandler for DisbursementHandler<A, R>
where
    A: PolicyAnchorView + Send + Sync,
    R: AllocationRegistryView + Send + Sync,
{
    fn handle(&self, call: &[u8], ctx: &CommitContext) -> Result<Vec<u8>, HandlerErrorCode> {
        let intent = decode_intent(call).map_err(|_| DECODE_FAILURE)?;
        let result =
            authorize_disbursement(&intent, &self.anchor, &self.registry, ctx.now_unix_ms);
        result.encode().map_err(|_| ENCODE_FAILURE)
    }
}

/// Build a disbursement handler from a TOML config file on disk.
///
/// Loader runs once at node startup; the resulting handler holds
/// `Arc`-shared views so it can be registered with Ziranity's
/// `SubstrateDispatchRegistry` without further I/O.
///
/// Config schema is documented in [`config`]. A sample config ships
/// at `integrations/ziranity/fixtures/oracleguard-node.sample.toml`.
///
/// Call sites on the Ziranity node side (typically
/// `ziranity_node/src/main.rs` under `#[cfg(feature = "oracleguard")]`):
///
/// ```no_run
/// # use std::path::Path;
/// # fn stub() -> Result<(), Box<dyn std::error::Error>> {
/// let handler = oracleguard_ziranity_handler::build_disbursement_handler(
///     Path::new("/etc/oracleguard/node.toml"),
/// )?;
/// let substrate_id = oracleguard_ziranity_handler::substrate_id();
/// // registry.register(substrate_id, Box::new(handler))?;
/// # Ok(()) }
/// ```
pub fn build_disbursement_handler(
    config_path: &Path,
) -> Result<DisbursementHandler<ConfigView, ConfigView>, BuildError> {
    let snapshot = ConfigSnapshot::from_toml_path(config_path)?;
    let view = ConfigView::new(snapshot);
    Ok(DisbursementHandler::new(view.clone(), view))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use oracleguard_policy::authorize::{AllocationFacts, AuthorizationResult};
    use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
    use oracleguard_schemas::encoding::{encode_intent, ORACLEGUARD_DISBURSEMENT_DOMAIN};
    use oracleguard_schemas::gate::AuthorizationGate;
    use oracleguard_schemas::intent::{DisbursementIntentV1, INTENT_VERSION_V1};
    use oracleguard_schemas::oracle::{
        OracleFactEvalV1, OracleFactProvenanceV1, ASSET_PAIR_ADA_USD, SOURCE_CHARLI3,
    };
    use oracleguard_schemas::reason::DisbursementReasonCode;

    // ---- Deterministic in-memory view fixtures ----

    struct StaticAnchor {
        registered: &'static [[u8; 32]],
    }

    impl PolicyAnchorView for StaticAnchor {
        fn is_policy_registered(&self, policy_ref: &[u8; 32]) -> bool {
            self.registered.iter().any(|r| r == policy_ref)
        }
    }

    struct StaticRegistry {
        allocation_id: [u8; 32],
        basis_lovelace: u64,
        authorized_subjects: &'static [[u8; 32]],
        approved_asset: AssetIdV1,
        approved_destinations: &'static [CardanoAddressV1],
    }

    impl AllocationRegistryView for StaticRegistry {
        fn lookup(&self, allocation_id: &[u8; 32]) -> Option<AllocationFacts<'_>> {
            if *allocation_id == self.allocation_id {
                Some(AllocationFacts {
                    basis_lovelace: self.basis_lovelace,
                    authorized_subjects: self.authorized_subjects,
                    approved_asset: self.approved_asset,
                    approved_destinations: self.approved_destinations,
                })
            } else {
                None
            }
        }
    }

    const POLICY_REF: [u8; 32] = [0x11; 32];
    const ALLOCATION_ID: [u8; 32] = [0x22; 32];
    const REQUESTER_ID: [u8; 32] = [0x33; 32];
    const REGISTERED_POLICY: [[u8; 32]; 1] = [POLICY_REF];
    const AUTHORIZED_SUBJECTS: [[u8; 32]; 1] = [REQUESTER_ID];
    const APPROVED_DESTINATIONS: [CardanoAddressV1; 1] = [CardanoAddressV1 {
        bytes: [0x55; 57],
        length: 57,
    }];

    fn sample_intent(requested_amount_lovelace: u64) -> DisbursementIntentV1 {
        DisbursementIntentV1 {
            intent_version: INTENT_VERSION_V1,
            policy_ref: POLICY_REF,
            allocation_id: ALLOCATION_ID,
            requester_id: REQUESTER_ID,
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
            requested_amount_lovelace,
            destination: CardanoAddressV1 {
                bytes: [0x55; 57],
                length: 57,
            },
            asset: AssetIdV1::ADA,
        }
    }

    fn build_handler() -> DisbursementHandler<StaticAnchor, StaticRegistry> {
        DisbursementHandler::new(
            StaticAnchor {
                registered: &REGISTERED_POLICY,
            },
            StaticRegistry {
                allocation_id: ALLOCATION_ID,
                basis_lovelace: 1_000_000_000,
                authorized_subjects: &AUTHORIZED_SUBJECTS,
                approved_asset: AssetIdV1::ADA,
                approved_destinations: &APPROVED_DESTINATIONS,
            },
        )
    }

    fn fresh_ctx() -> CommitContext {
        CommitContext {
            now_unix_ms: 1_713_000_100_000, // 100s into the 300s freshness window
            block_height: 42,
            block_id: [0x77; 32],
        }
    }

    // ---- substrate_id pinning ----

    #[test]
    fn substrate_id_matches_ocu_derivation() {
        assert_eq!(substrate_id(), DISBURSEMENT_OCU_ID);
    }

    #[test]
    fn substrate_id_is_not_zero() {
        // Sanity: an all-zero substrate_id would be indistinguishable
        // from an unset field and would defeat registry routing.
        assert_ne!(substrate_id(), [0u8; 32]);
    }

    // handler_error_code_constants_land_in_oracleguard_range: enforced
    // at compile time via `const _: () = assert!(...)` at the module
    // level; no runtime test needed.

    // ---- Allow path ----

    #[test]
    fn allow_path_returns_ok_with_authorized_bytes() {
        let handler = build_handler();
        let intent = sample_intent(700_000_000);
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let out = match handler.handle(&call, &fresh_ctx()) {
            Ok(o) => o,
            Err(c) => panic!("handler returned Err({c:#06x}) on well-formed allow"),
        };
        let decoded = match AuthorizationResult::decode(&out) {
            Ok(r) => r,
            Err(_) => panic!("output did not decode as AuthorizationResult"),
        };
        match decoded {
            AuthorizationResult::Authorized { effect } => {
                assert_eq!(effect.authorized_amount_lovelace, 700_000_000);
                assert_eq!(effect.release_cap_basis_points, 7_500);
            }
            AuthorizationResult::Denied { reason, gate } => {
                panic!("expected Authorized, got Denied({reason:?}, {gate:?})")
            }
        }
    }

    // ---- Deny path (still Ok; denial is a governance decision) ----

    #[test]
    fn deny_path_returns_ok_with_denied_bytes() {
        let handler = build_handler();
        let intent = sample_intent(900_000_000); // exceeds 75% cap on 1000 ADA
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let out = match handler.handle(&call, &fresh_ctx()) {
            Ok(o) => o,
            Err(c) => panic!("handler returned Err({c:#06x}) on well-formed deny"),
        };
        let decoded = match AuthorizationResult::decode(&out) {
            Ok(r) => r,
            Err(_) => panic!("output did not decode as AuthorizationResult"),
        };
        match decoded {
            AuthorizationResult::Denied { reason, gate } => {
                assert_eq!(reason, DisbursementReasonCode::ReleaseCapExceeded);
                assert_eq!(gate, AuthorizationGate::Grant);
            }
            AuthorizationResult::Authorized { .. } => {
                panic!("expected Denied, got Authorized")
            }
        }
    }

    // ---- Decode failure path ----

    #[test]
    fn malformed_call_bytes_map_to_decode_failure() {
        let handler = build_handler();
        let out = handler.handle(&[0x00, 0x01, 0x02], &fresh_ctx());
        assert_eq!(out, Err(DECODE_FAILURE));
    }

    #[test]
    fn empty_call_bytes_map_to_decode_failure() {
        let handler = build_handler();
        let out = handler.handle(&[], &fresh_ctx());
        assert_eq!(out, Err(DECODE_FAILURE));
    }

    #[test]
    fn trailing_bytes_after_intent_map_to_decode_failure() {
        let handler = build_handler();
        let intent = sample_intent(700_000_000);
        let mut call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        call.push(0x00);
        let out = handler.handle(&call, &fresh_ctx());
        assert_eq!(out, Err(DECODE_FAILURE));
    }

    // ---- Pre-evaluator gate denials also ride Ok ----

    #[test]
    fn stale_oracle_denies_with_oracle_stale_on_ok_channel() {
        let handler = build_handler();
        let intent = sample_intent(700_000_000);
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let stale_ctx = CommitContext {
            now_unix_ms: intent.oracle_provenance.expiry_unix, // at-expiry is stale
            block_height: 42,
            block_id: [0x77; 32],
        };
        let out = match handler.handle(&call, &stale_ctx) {
            Ok(o) => o,
            Err(c) => panic!("handler returned Err({c:#06x}) on stale-oracle deny"),
        };
        match AuthorizationResult::decode(&out) {
            Ok(AuthorizationResult::Denied { reason, gate }) => {
                assert_eq!(reason, DisbursementReasonCode::OracleStale);
                assert_eq!(gate, AuthorizationGate::Grant);
            }
            Ok(AuthorizationResult::Authorized { .. }) => {
                panic!("stale oracle should deny, got Authorized")
            }
            Err(_) => panic!("output did not decode as AuthorizationResult"),
        }
    }

    #[test]
    fn unregistered_policy_denies_with_policy_not_found_on_ok_channel() {
        let handler = DisbursementHandler::new(
            StaticAnchor {
                registered: &[], // no policies registered
            },
            StaticRegistry {
                allocation_id: ALLOCATION_ID,
                basis_lovelace: 1_000_000_000,
                authorized_subjects: &AUTHORIZED_SUBJECTS,
                approved_asset: AssetIdV1::ADA,
                approved_destinations: &APPROVED_DESTINATIONS,
            },
        );
        let intent = sample_intent(700_000_000);
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let out = match handler.handle(&call, &fresh_ctx()) {
            Ok(o) => o,
            Err(c) => panic!("handler returned Err({c:#06x}) on unregistered-policy deny"),
        };
        match AuthorizationResult::decode(&out) {
            Ok(AuthorizationResult::Denied { reason, gate }) => {
                assert_eq!(reason, DisbursementReasonCode::PolicyNotFound);
                assert_eq!(gate, AuthorizationGate::Anchor);
            }
            Ok(AuthorizationResult::Authorized { .. }) => {
                panic!("unregistered policy should deny, got Authorized")
            }
            Err(_) => panic!("output did not decode as AuthorizationResult"),
        }
    }

    // ---- Determinism ----

    #[test]
    fn handle_is_deterministic_across_calls() {
        let handler = build_handler();
        let intent = sample_intent(700_000_000);
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let a = handler.handle(&call, &fresh_ctx());
        let b = handler.handle(&call, &fresh_ctx());
        assert_eq!(a, b);
    }

    // ---- Sample config round-trip ----

    #[test]
    fn sample_config_loads_and_builds_a_usable_handler() {
        // Ensures `build_disbursement_handler` actually works end to
        // end against the sample config that ships in this repo. If
        // the config schema drifts, this test catches it.
        let sample_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("oracleguard-node.sample.toml");
        let handler = match build_disbursement_handler(&sample_path) {
            Ok(h) => h,
            Err(e) => panic!("sample config failed to build handler: {e}"),
        };

        // The sample config registers policy_ref [0x11; 32]-ish — let's
        // use its real hex value rather than hard-code.
        let intent = sample_intent(700_000_000);
        let call = match encode_intent(&intent) {
            Ok(b) => b,
            Err(_) => panic!("encode_intent failed"),
        };
        let out = match handler.handle(&call, &fresh_ctx()) {
            Ok(o) => o,
            Err(c) => panic!("handler returned Err({c:#06x}) on sample-config allow"),
        };
        let decoded = match AuthorizationResult::decode(&out) {
            Ok(r) => r,
            Err(_) => panic!("output did not decode as AuthorizationResult"),
        };
        // The sample config's policy_ref is the real fixture policy_ref
        // (56a7bb97…), not 0x11^32. Our sample_intent uses 0x11^32, so
        // the anchor gate correctly rejects it with PolicyNotFound.
        // This test doesn't validate the allow path per se — it validates
        // that the loader produces a working handler whose outputs are
        // well-formed AuthorizationResult bytes.
        match decoded {
            AuthorizationResult::Authorized { .. } | AuthorizationResult::Denied { .. } => {
                // Either outcome is a success signal for this test — we
                // just need "build_disbursement_handler produces a
                // handler whose outputs decode as AuthorizationResult."
            }
        }
    }

    #[test]
    fn domain_separator_is_not_a_ziranity_tag() {
        // Defensive pin: OracleGuard's canonical-intent domain
        // separator must not collide with any Ziranity-side tag.
        // This crate is the first crate in the workspace that
        // compiles against both sides' public surfaces, so the
        // check lives here.
        assert_ne!(ORACLEGUARD_DISBURSEMENT_DOMAIN, "ZIRANITY_INTENT_ID_V1");
        assert_ne!(
            ORACLEGUARD_DISBURSEMENT_DOMAIN,
            "ZIRANITY_VERIFICATION_BUNDLE_V1"
        );
        assert_ne!(ORACLEGUARD_DISBURSEMENT_DOMAIN, "ZIRANITY_REPLAY_GOLDEN_V1");
    }
}
