//! End-to-end example: construct a canonical `DisbursementIntentV1`
//! from a live-captured Charli3 AggState datum fixture.
//!
//! Demonstrates the Cluster 3 oracle-boundary usage rules from
//! `docs/oracle-boundary.md`: the adapter normalizes raw on-chain
//! bytes into a split `(OracleFactEvalV1, OracleFactProvenanceV1)`,
//! and the intent constructor keeps those domains in their correct
//! slots. No decision logic; no settlement.
//!
//! Run:
//!     cargo run -p oracleguard-adapter --example build_intent_from_charli3

use oracleguard_adapter::charli3::normalize_parse_validate;
use oracleguard_schemas::effect::{AssetIdV1, CardanoAddressV1};
use oracleguard_schemas::encoding::{encode_intent, intent_id};
use oracleguard_schemas::intent::DisbursementIntentV1;

const AGGSTATE_GOLDEN: &[u8] =
    include_bytes!("../../../fixtures/charli3/aggstate_v1_golden.cbor");

fn main() {
    // The transaction id of the UTxO holding the AggState token. In a
    // real run this comes from the Kupo query alongside the datum
    // bytes. Here we use the documented fixture capture reference.
    let aggregator_utxo_ref: [u8; 32] = {
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(b"d44b5a1d7f41e8d2");
        out
    };

    // Use a wall-clock value well inside the fixture's validity
    // window so the freshness check passes deterministically.
    let now_unix_ms: u64 = 1_776_428_823_000;

    let (oracle_fact, oracle_provenance) =
        match normalize_parse_validate(AGGSTATE_GOLDEN, aggregator_utxo_ref, now_unix_ms) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("oracle normalization rejected: {e:?}");
                std::process::exit(1);
            }
        };

    // Fixture identifiers — in a real run these come from the Katiba
    // anchor, the approved allocation registry, and the requester's
    // signed request.
    let policy_ref: [u8; 32] = [0x11; 32];
    let allocation_id: [u8; 32] = [0x22; 32];
    let requester_id: [u8; 32] = [0x33; 32];
    let requested_amount_lovelace: u64 = 700_000_000;
    let destination = CardanoAddressV1 {
        bytes: [0x55; 57],
        length: 57,
    };

    let intent = DisbursementIntentV1::new_v1(
        policy_ref,
        allocation_id,
        requester_id,
        oracle_fact,
        oracle_provenance,
        requested_amount_lovelace,
        destination,
        AssetIdV1::ADA,
    );

    let canonical_bytes = match encode_intent(&intent) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("encode failed: {e:?}");
            std::process::exit(1);
        }
    };
    let id = match intent_id(&intent) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("intent_id failed: {e:?}");
            std::process::exit(1);
        }
    };

    println!("intent_version       = {}", intent.intent_version);
    println!("oracle_fact.price    = {} microusd", intent.oracle_fact.price_microusd);
    println!(
        "oracle_provenance.ts = {} ms (audit-only)",
        intent.oracle_provenance.timestamp_unix
    );
    println!("canonical byte len   = {}", canonical_bytes.len());
    print!("intent_id            = ");
    for b in id {
        print!("{b:02x}");
    }
    println!();
}
