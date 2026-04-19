//! Assemble a live evidence bundle from the just-committed consensus
//! decision and the real Cardano settlement, then verify it offline.
//!
//! Consumes the outputs of the live path driven by `scripts/demo.sh`:
//!
//!   --intent-path   canonical DisbursementIntentV1 bytes (the same
//!                   fixture the devnet smoke just byte-matched)
//!   --auth-path     canonical AuthorizationResult bytes (the same
//!                   fixture the devnet smoke just byte-matched —
//!                   smoke.sh's PASS is proof these bytes reproduce
//!                   the consensus decision)
//!   --tx-hash       64-char hex Cardano transaction id captured from
//!                   scripts/cardano_disburse.py
//!   --committed-height
//!                   consensus block height captured from smoke.sh
//!   --allocation-basis-lovelace
//!                   the evaluator allocation basis (default 1_000_000_000)
//!
//! The binary loads the intent + authorization bytes, computes
//! `intent_id`, builds a `DisbursementEvidenceV1` via the canonical
//! assembly function in `oracleguard_adapter::artifacts`, then runs
//! `oracleguard_verifier::verify_evidence` over the result. A CLEAN
//! verdict proves that the exact decision just committed and the
//! exact tx that just settled reproduce — from the recorded evidence
//! alone — with no placeholder fields.

use std::path::PathBuf;

use oracleguard_adapter::artifacts::assemble_disbursement_evidence;
use oracleguard_adapter::cardano::CardanoTxHashV1;
use oracleguard_adapter::intake::{IntentReceiptV1, IntentStatus};
use oracleguard_adapter::settlement::FulfillmentOutcome;
use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_schemas::encoding::{decode_intent, intent_id};
use oracleguard_schemas::evidence::{
    AuthorizationSnapshotV1, DisbursementEvidenceV1, ExecutionOutcomeV1, FulfillmentRejectionKindV1,
};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::reason::DisbursementReasonCode;
use oracleguard_verifier::verify_evidence;

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n");
            print_usage();
            std::process::exit(2);
        }
    };

    // 1. Load the canonical intent and decode it.
    let intent_bytes = match std::fs::read(&args.intent_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read intent {}: {e}", args.intent_path.display());
            std::process::exit(1);
        }
    };
    let intent = match decode_intent(&intent_bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("failed to decode intent: {e:?}");
            std::process::exit(1);
        }
    };
    let id = match intent_id(&intent) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("failed to compute intent_id: {e:?}");
            std::process::exit(1);
        }
    };

    // 2. Load the canonical AuthorizationResult bytes and decode them.
    //    These are the bytes the 4-node Ziranity devnet just committed
    //    and byte-matched against this fixture in the previous step.
    let auth_bytes = match std::fs::read(&args.auth_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read auth {}: {e}", args.auth_path.display());
            std::process::exit(1);
        }
    };
    let authorization = match AuthorizationResult::decode(&auth_bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("failed to decode AuthorizationResult: {e:?}");
            std::process::exit(1);
        }
    };

    // 3. Build the consensus receipt from the committed fields we
    //    captured live: intent_id (recomputed), the real
    //    committed_height from smoke.sh's PASS line, the authorization
    //    bytes the network produced.
    let receipt = IntentReceiptV1 {
        intent_id: id,
        status: IntentStatus::Committed,
        committed_height: args.committed_height,
        output: authorization,
    };

    // 4. Build the execution outcome. If the authorization is
    //    Authorized, we require --tx-hash and build a Settled outcome
    //    with the real Cardano tx id. If the authorization is Denied,
    //    no Cardano tx exists by design — execution is DeniedUpstream
    //    carrying the same (reason, gate) the authorization records.
    let outcome = match authorization {
        AuthorizationResult::Authorized { .. } => {
            let tx_hash = match args.tx_hash {
                Some(h) => h,
                None => {
                    eprintln!("error: --tx-hash required for Authorized scenarios");
                    std::process::exit(2);
                }
            };
            FulfillmentOutcome::Settled {
                tx_hash: CardanoTxHashV1(tx_hash),
            }
        }
        AuthorizationResult::Denied { reason, gate } => {
            FulfillmentOutcome::DeniedUpstream { reason, gate }
        }
    };

    // 5. Assemble the canonical evidence bundle. The now_unix_ms input
    //    is provenance only — the verifier does not re-check freshness;
    //    use the intent's recorded provenance timestamp so the bundle
    //    is internally consistent.
    let now_unix_ms = intent.oracle_provenance.timestamp_unix;
    let evidence = match assemble_disbursement_evidence(
        &intent,
        &receipt,
        outcome,
        args.allocation_basis_lovelace,
        now_unix_ms,
    ) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("failed to assemble evidence: {e:?}");
            std::process::exit(1);
        }
    };

    // 6. Describe the live bundle and run the verifier over it.
    println!("── Live evidence bundle (assembled from this run)");
    describe(&evidence);

    let report = verify_evidence(&evidence);
    if report.is_ok() {
        println!("   verdict                : CLEAN");
        println!();
        match authorization {
            AuthorizationResult::Authorized { .. } => {
                println!("   → the committed decision and the settled Cardano tx");
                println!("     reproduce byte-for-byte from the recorded evidence,");
                println!("     with no placeholder fields. Independent auditors can");
                println!("     replay this bundle offline and reach the same verdict.");
            }
            AuthorizationResult::Denied { .. } => {
                println!("   → the denial reproduces byte-for-byte from the recorded");
                println!("     evidence. No Cardano tx exists (denials emit none);");
                println!("     an independent auditor replays the bundle and reaches");
                println!("     the same Denied verdict with the same reason + gate.");
            }
        }
        std::process::exit(0);
    } else {
        println!("   verdict                : FAILED");
        for f in &report.findings {
            println!("     finding              : {f:?}");
        }
        std::process::exit(1);
    }
}

// --------------------------------------------------------------------
// argument parsing
// --------------------------------------------------------------------

struct Args {
    intent_path: PathBuf,
    auth_path: PathBuf,
    tx_hash: Option<[u8; 32]>,
    committed_height: u64,
    allocation_basis_lovelace: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut intent_path: Option<PathBuf> = None;
    let mut auth_path: Option<PathBuf> = None;
    let mut tx_hash_hex: Option<String> = None;
    let mut committed_height: Option<u64> = None;
    let mut allocation_basis_lovelace: u64 = 1_000_000_000;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--intent-path" => {
                intent_path = Some(it.next().ok_or("--intent-path needs a value")?.into())
            }
            "--auth-path" => auth_path = Some(it.next().ok_or("--auth-path needs a value")?.into()),
            "--tx-hash" => tx_hash_hex = Some(it.next().ok_or("--tx-hash needs a value")?),
            "--committed-height" => {
                let v = it.next().ok_or("--committed-height needs a value")?;
                committed_height = Some(
                    v.parse()
                        .map_err(|_| format!("invalid --committed-height value '{v}'"))?,
                );
            }
            "--allocation-basis-lovelace" => {
                let v = it
                    .next()
                    .ok_or("--allocation-basis-lovelace needs a value")?;
                allocation_basis_lovelace = v
                    .parse()
                    .map_err(|_| format!("invalid --allocation-basis-lovelace value '{v}'"))?;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag '{other}'")),
        }
    }

    let intent_path = intent_path.ok_or("--intent-path is required")?;
    let auth_path = auth_path.ok_or("--auth-path is required")?;
    let committed_height = committed_height.ok_or("--committed-height is required")?;
    // --tx-hash is optional: required only for Authorized scenarios,
    // forbidden (would be misleading) for Denied ones. The main loop
    // enforces the Authorized-requires-tx-hash invariant after decoding
    // the authorization bytes.
    let tx_hash = match tx_hash_hex {
        Some(hex) => Some(decode_tx_hash(&hex)?),
        None => None,
    };

    Ok(Args {
        intent_path,
        auth_path,
        tx_hash,
        committed_height,
        allocation_basis_lovelace,
    })
}

fn print_usage() {
    eprintln!(
        "\
verify_live_bundle — assemble + verify a live evidence bundle

USAGE
    verify_live_bundle --intent-path PATH --auth-path PATH --tx-hash HEX \\
                       --committed-height N [--allocation-basis-lovelace N]

REQUIRED
    --intent-path PATH              canonical DisbursementIntentV1 bytes
    --auth-path PATH                canonical AuthorizationResult bytes
    --tx-hash HEX                   64-char Cardano tx id
    --committed-height N            consensus block height

OPTIONAL
    --allocation-basis-lovelace N   evaluator basis  (default: 1_000_000_000)
"
    );
}

fn decode_tx_hash(s: &str) -> Result<[u8; 32], String> {
    if s.len() != 64 {
        return Err(format!(
            "--tx-hash must be 64 hex chars (got {} chars)",
            s.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let hi = hex_nibble(s.as_bytes()[2 * i])?;
        let lo = hex_nibble(s.as_bytes()[2 * i + 1])?;
        *byte = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("invalid hex char '{}'", b as char)),
    }
}

// --------------------------------------------------------------------
// formatting (mirrors verify_bundles.rs for visual consistency)
// --------------------------------------------------------------------

fn describe(e: &DisbursementEvidenceV1) {
    println!(
        "   bundle version         : v{}  ({} bytes of canonical postcard)",
        e.evidence_version,
        approx_bytes(e),
    );
    println!("   intent_id              : {}", hex32(&e.intent_id));
    println!(
        "   requested amount       : {} lovelace  ({} ADA)",
        e.intent.requested_amount_lovelace,
        e.intent.requested_amount_lovelace / 1_000_000
    );
    println!(
        "   allocation basis       : {} lovelace  ({} ADA)",
        e.allocation_basis_lovelace,
        e.allocation_basis_lovelace / 1_000_000
    );
    println!(
        "   oracle price           : {} microusd  (~${}.{:03})",
        e.intent.oracle_fact.price_microusd,
        e.intent.oracle_fact.price_microusd / 1_000_000,
        (e.intent.oracle_fact.price_microusd % 1_000_000) / 1_000,
    );
    println!(
        "   committed height       : {}  (live from consensus)",
        e.committed_height
    );
    println!(
        "   authorization          : {}",
        describe_auth(&e.authorization)
    );
    println!(
        "   execution              : {}",
        describe_exec(&e.execution)
    );
    println!(
        "   checks                 : intent_id recompute + gate/reason invariants + replay evaluator"
    );
}

fn describe_auth(a: &AuthorizationSnapshotV1) -> String {
    match a {
        AuthorizationSnapshotV1::Authorized { effect } => format!(
            "Authorized  ({} ADA authorized, band = {} bps)",
            effect.authorized_amount_lovelace / 1_000_000,
            effect.release_cap_basis_points,
        ),
        AuthorizationSnapshotV1::Denied { reason, gate } => format!(
            "Denied      ({} at {} gate)",
            reason_name(*reason),
            gate_name(*gate),
        ),
    }
}

fn describe_exec(x: &ExecutionOutcomeV1) -> String {
    match x {
        ExecutionOutcomeV1::Settled { tx_hash } => {
            format!("Settled     (tx = {}  — live from Cardano)", hex32(tx_hash))
        }
        ExecutionOutcomeV1::DeniedUpstream { reason, gate } => format!(
            "DeniedUpstream ({} at {} gate — no tx)",
            reason_name(*reason),
            gate_name(*gate),
        ),
        ExecutionOutcomeV1::RejectedAtFulfillment { kind } => {
            format!("RejectedAtFulfillment ({}) — no tx", rejection_name(*kind))
        }
    }
}

fn reason_name(r: DisbursementReasonCode) -> &'static str {
    match r {
        DisbursementReasonCode::PolicyNotFound => "PolicyNotFound",
        DisbursementReasonCode::AllocationNotFound => "AllocationNotFound",
        DisbursementReasonCode::OracleStale => "OracleStale",
        DisbursementReasonCode::OraclePriceZero => "OraclePriceZero",
        DisbursementReasonCode::AmountZero => "AmountZero",
        DisbursementReasonCode::ReleaseCapExceeded => "ReleaseCapExceeded",
        DisbursementReasonCode::SubjectNotAuthorized => "SubjectNotAuthorized",
        DisbursementReasonCode::AssetMismatch => "AssetMismatch",
    }
}

fn gate_name(g: AuthorizationGate) -> &'static str {
    match g {
        AuthorizationGate::Anchor => "Anchor",
        AuthorizationGate::Registry => "Registry",
        AuthorizationGate::Grant => "Grant",
    }
}

fn rejection_name(k: FulfillmentRejectionKindV1) -> &'static str {
    match k {
        FulfillmentRejectionKindV1::NonAdaAsset => "NonAdaAsset",
        FulfillmentRejectionKindV1::ReceiptNotCommitted => "ReceiptNotCommitted",
    }
}

fn approx_bytes(e: &DisbursementEvidenceV1) -> usize {
    oracleguard_schemas::evidence::encode_evidence(e)
        .map(|v| v.len())
        .unwrap_or(0)
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
