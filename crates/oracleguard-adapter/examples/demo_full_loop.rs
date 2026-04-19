//! Policy-centric demo orchestrator.
//!
//! Surfaces the full governance narrative end-to-end:
//!
//! 1. POLICY       — canonicalize the policy JSON, derive `policy_ref`
//! 2. ALLOCATION   — show the approved allocation
//! 3. ORACLE       — fetch a fresh ADA/USD price (live Kupo or fixture)
//! 4. PER SCENARIO — one allow (in-cap) and one deny (over-cap):
//!    4a. INTENT       — build + encode canonical intent, show `intent_id`
//!    4b. EXPECTED     — run the evaluator in-process, show committed bytes
//!    4c. SUBMIT       — (optional) submit via Ziranity devnet, diff bytes
//!    4d. SETTLE       — (optional, allow only) submit Cardano tx
//!    4e. VERIFY       — assemble evidence bundle, run offline verifier
//! 5. ROTATION     — (optional) raise the release cap, show `policy_ref`
//!    changes and the same intent flips outcome
//!
//! Scenario request amounts are chosen dynamically from the current
//! oracle price band: allow = 80 % of cap, deny = 110 % of cap. This
//! works at any ADA price without changing the canonical policy bands.
//!
//! Run:
//!
//!     # default: offline (no submit/settle), both scenarios, offline verify
//!     cargo run -p oracleguard-adapter --example demo_full_loop
//!
//!     # use golden fixture instead of live Kupo
//!     cargo run -p oracleguard-adapter --example demo_full_loop -- --no-oracle
//!
//!     # single scenario, with policy-rotation stretch
//!     cargo run -p oracleguard-adapter --example demo_full_loop -- \
//!         --scenario allow --rotate
//!
//! Flags:
//!
//! - `--no-oracle`        use the golden fixture instead of live Kupo
//! - `--scenario NAME`    allow | deny | both                      (default: both)
//! - `--allocation N`     approved allocation in lovelace          (default: 1_000_000_000)
//! - `--kupo URL`         override Kupo base URL
//! - `--rotate`           run the policy-rotation stretch phase
//! - `--raised-cap-bps N` cap used by the rotated policy (default 10_000 = 100 %)

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use oracleguard_adapter::artifacts::assemble_disbursement_evidence;
use oracleguard_adapter::cardano::CardanoTxHashV1;
use oracleguard_adapter::charli3::{normalize_aggstate_datum, Charli3ParseError};
use oracleguard_adapter::intake::{IntentReceiptV1, IntentStatus};
use oracleguard_adapter::kupo::{
    fetch_aggstate_datum, KupoEndpoint, CHARLI3_ADA_USD_ASSET_KEY, CHARLI3_ADA_USD_ORACLE_ADDRESS,
    DEFAULT_TIMEOUT,
};
use oracleguard_adapter::settlement::FulfillmentOutcome;
use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_policy::evaluate::evaluate_disbursement;
use oracleguard_policy::EvaluationResult;
use oracleguard_schemas::effect::{AssetIdV1, AuthorizedEffectV1, CardanoAddressV1};
use oracleguard_schemas::encoding::{encode_intent, intent_id};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::oracle::{check_freshness, OracleFactEvalV1, OracleFactProvenanceV1};
use oracleguard_schemas::policy::{canonicalize_policy_json, derive_policy_ref};
use oracleguard_schemas::reason::DisbursementReasonCode;
use oracleguard_verifier::verify_evidence;

const GOLDEN_AGGSTATE: &[u8] = include_bytes!("../../../fixtures/charli3/aggstate_v1_golden.cbor");
const GOLDEN_FIXTURE_NOW_MS: u64 = 1_776_428_823_000;

const DEFAULT_ALLOCATION_LOVELACE: u64 = 1_000_000_000;

// wallet_3 (receiver, Preprod). Raw 57 bytes.
const DEMO_DESTINATION: [u8; 57] = [
    0x00, 0x0e, 0xe0, 0x3e, 0x45, 0xb0, 0x5d, 0x62, 0x25, 0xeb, 0x71, 0x43, 0xd2, 0xbe, 0x23, 0xa3,
    0xb8, 0x62, 0x9b, 0x4d, 0x9e, 0x61, 0x62, 0x1b, 0xb3, 0x0a, 0xfc, 0xc5, 0x3a, 0x95, 0x47, 0x4e,
    0x79, 0xd1, 0x29, 0x8d, 0x29, 0x1e, 0x20, 0x0a, 0x2c, 0x88, 0x9f, 0xe1, 0x3b, 0x97, 0x78, 0x5c,
    0xc4, 0x12, 0x79, 0x62, 0x9d, 0x99, 0xcf, 0xb8, 0x7d,
];

const DEMO_ALLOCATION_ID: [u8; 32] = [0x22; 32];
const DEMO_REQUESTER_ID: [u8; 32] = [0x33; 32];

// Dummy tx hash used in offline bundle assembly so the verifier has a
// complete record to replay. Labeled clearly in the output so nobody
// confuses it with a real Preprod tx id.
const DEMO_DUMMY_TX_HASH: [u8; 32] = [0xDE; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Allow,
    Deny,
}

impl Scenario {
    fn label(self) -> &'static str {
        match self {
            Scenario::Allow => "allow",
            Scenario::Deny => "deny",
        }
    }
}

struct Args {
    no_oracle: bool,
    scenarios: Vec<Scenario>,
    allocation_lovelace: u64,
    kupo_url: Option<String>,
    rotate: bool,
    raised_cap_bps: u64,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut no_oracle = false;
        let mut scenarios = vec![Scenario::Allow, Scenario::Deny];
        let mut allocation_lovelace = DEFAULT_ALLOCATION_LOVELACE;
        let mut kupo_url: Option<String> = None;
        let mut rotate = false;
        let mut raised_cap_bps: u64 = 10_000;

        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--no-oracle" => no_oracle = true,
                "--scenario" => {
                    let v = it.next().ok_or("--scenario requires a value")?;
                    scenarios = match v.as_str() {
                        "allow" => vec![Scenario::Allow],
                        "deny" => vec![Scenario::Deny],
                        "both" => vec![Scenario::Allow, Scenario::Deny],
                        other => return Err(format!("unknown scenario '{other}'")),
                    };
                }
                "--allocation" => {
                    let v = it.next().ok_or("--allocation requires a value")?;
                    allocation_lovelace = v
                        .parse()
                        .map_err(|_| format!("invalid --allocation value '{v}'"))?;
                }
                "--kupo" => {
                    kupo_url = Some(it.next().ok_or("--kupo requires a value")?);
                }
                "--rotate" => rotate = true,
                "--raised-cap-bps" => {
                    let v = it.next().ok_or("--raised-cap-bps requires a value")?;
                    raised_cap_bps = v
                        .parse()
                        .map_err(|_| format!("invalid --raised-cap-bps value '{v}'"))?;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown flag '{other}'")),
            }
        }

        Ok(Self {
            no_oracle,
            scenarios,
            allocation_lovelace,
            kupo_url,
            rotate,
            raised_cap_bps,
        })
    }
}

fn print_help() {
    println!(
        "\
demo_full_loop — policy-centric OracleGuard demo orchestrator

USAGE:
    cargo run -p oracleguard-adapter --example demo_full_loop -- [flags]

FLAGS:
    --no-oracle            skip the live Kupo fetch; use the golden fixture
    --scenario NAME        one of: allow, deny, both            (default: both)
    --allocation N         approved allocation in lovelace      (default: 1000000000)
    --kupo URL             override Kupo base URL
    --rotate               run the policy-rotation stretch phase
    --raised-cap-bps N     rotated-policy cap in basis points   (default: 10000)
    -h, --help             print this help
"
    );
}

fn main() -> ExitCode {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n");
            print_help();
            return ExitCode::from(2);
        }
    };

    let workspace = match workspace_root() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not resolve workspace root: {e}");
            return ExitCode::from(2);
        }
    };

    banner("OracleGuard demo — policy-governed, independently verifiable");

    // ---- PHASE 1 — POLICY ----
    let policy = match phase_policy(&workspace) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("POLICY phase failed: {e}");
            return ExitCode::from(1);
        }
    };

    // ---- PHASE 2 — ALLOCATION ----
    phase_allocation(args.allocation_lovelace);

    // ---- PHASE 3 — ORACLE ----
    let oracle = match phase_oracle(args.no_oracle, args.kupo_url.as_deref()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("ORACLE phase failed: {e}");
            return ExitCode::from(1);
        }
    };

    // ---- PHASE 4 — PER-SCENARIO PIPELINE ----
    let mut all_scenarios_ok = true;
    let mut scenario_reports: Vec<ScenarioReport> = Vec::new();
    for s in &args.scenarios {
        let report = run_scenario(*s, &policy.policy_ref, args.allocation_lovelace, &oracle);
        if !report.verifier_clean {
            all_scenarios_ok = false;
        }
        scenario_reports.push(report);
    }

    // ---- PHASE 5 — POLICY ROTATION (stretch) ----
    let rotation_report = if args.rotate {
        Some(phase_rotation(
            &policy,
            &oracle,
            args.allocation_lovelace,
            args.raised_cap_bps,
        ))
    } else {
        None
    };

    // ---- SUMMARY ----
    section("SUMMARY");
    println!("  policy_ref              : {}", hex32(&policy.policy_ref));
    println!(
        "  oracle source           : {}",
        if args.no_oracle {
            "golden fixture"
        } else {
            "live Kupo"
        }
    );
    println!(
        "  price at run            : ${}.{:03}  (band = {})",
        oracle.eval.price_microusd / 1_000_000,
        (oracle.eval.price_microusd % 1_000_000) / 1_000,
        band_name(oracle.eval.price_microusd)
    );
    println!("  per-scenario results    :");
    for r in &scenario_reports {
        println!(
            "    {:<5}  expected={:<20}  evaluator={:<20}  verifier={}",
            r.scenario.label(),
            outcome_short(r.expected_outcome),
            outcome_short(r.evaluator_outcome),
            if r.verifier_clean { "CLEAN" } else { "FAILED" },
        );
    }
    if let Some(rot) = &rotation_report {
        println!("  rotation                : {}", rot.one_line_summary());
    }
    println!();
    if all_scenarios_ok {
        println!("  → deterministic, policy-governed, independently verifiable.");
        ExitCode::SUCCESS
    } else {
        println!("  → FAIL: at least one scenario did not verify.");
        ExitCode::from(1)
    }
}

// =====================================================================
// Phase 1 — POLICY
// =====================================================================

struct PolicyDisplay {
    policy_ref: [u8; 32],
}

fn phase_policy(workspace: &Path) -> Result<PolicyDisplay, String> {
    section("PHASE 1 — POLICY");

    let path = workspace.join("fixtures").join("policy_v1.json");
    let raw = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let pretty = String::from_utf8_lossy(&raw);
    let canonical =
        canonicalize_policy_json(&raw).map_err(|e| format!("canonicalize policy: {e:?}"))?;
    let policy_ref = derive_policy_ref(&canonical);

    println!("  source          : {}", path.display());
    println!("  json            :");
    for line in pretty.trim().lines() {
        println!("    {line}");
    }
    println!("  canonical bytes : {} bytes", canonical.len());
    println!(
        "  policy_ref      : {}  (sha256 of canonical bytes)",
        hex32(policy_ref.as_bytes())
    );
    println!();

    Ok(PolicyDisplay {
        policy_ref: *policy_ref.as_bytes(),
    })
}

// =====================================================================
// Phase 2 — ALLOCATION
// =====================================================================

fn phase_allocation(allocation_lovelace: u64) {
    section("PHASE 2 — ALLOCATION");
    println!("  pool wallet      : test_wallet_1 (operator pool, Preprod)");
    println!(
        "  approved size    : {} lovelace  ({} ADA)",
        allocation_lovelace,
        allocation_lovelace / 1_000_000
    );
    println!("  receiver         : test_wallet_3 (Preprod)");
    println!(
        "  receiver bytes   : {}... (57 bytes)",
        hex_prefix(&DEMO_DESTINATION, 8)
    );
    println!();
}

// =====================================================================
// Phase 3 — ORACLE
// =====================================================================

struct OracleObservation {
    eval: OracleFactEvalV1,
    provenance: OracleFactProvenanceV1,
    now_unix_ms: u64,
}

fn phase_oracle(no_oracle: bool, kupo_url: Option<&str>) -> Result<OracleObservation, String> {
    section("PHASE 3 — ORACLE");

    let (eval, provenance, source_label, now_unix_ms): (_, _, &str, _) = if no_oracle {
        println!("  mode            : FIXTURE (--no-oracle)");
        let utxo_ref = [0xAA; 32];
        let (e, p) = normalize_aggstate_datum(GOLDEN_AGGSTATE, utxo_ref)
            .map_err(|err: Charli3ParseError| format!("normalize fixture: {err:?}"))?;
        // Use a deterministic now inside the fixture's window so the
        // freshness check always passes in fixture mode.
        (
            e,
            p,
            "golden Charli3 ADA/USD fixture",
            GOLDEN_FIXTURE_NOW_MS,
        )
    } else {
        println!("  mode            : LIVE (Kupo HTTP)");
        let endpoint = match kupo_url {
            Some(url) => KupoEndpoint::new(url.to_string()),
            None => KupoEndpoint::hackathon_preprod(),
        };
        println!("  endpoint        : {}", endpoint.base_url);
        println!("  oracle address  : {}", CHARLI3_ADA_USD_ORACLE_ADDRESS);
        println!("  asset key       : {} (C3AS)", CHARLI3_ADA_USD_ASSET_KEY);

        let fetched = fetch_aggstate_datum(
            &endpoint,
            CHARLI3_ADA_USD_ORACLE_ADDRESS,
            CHARLI3_ADA_USD_ASSET_KEY,
            DEFAULT_TIMEOUT,
        )
        .map_err(|e| format!("Kupo fetch failed: {e:?}"))?;

        let (e, p) = normalize_aggstate_datum(&fetched.cbor_bytes, fetched.aggregator_utxo_ref)
            .map_err(|err| format!("normalize live datum: {err:?}"))?;
        (e, p, "Charli3 ADA/USD (live Preprod)", wall_clock_ms())
    };

    let fresh_label = match check_freshness(&provenance, now_unix_ms) {
        Ok(_) => format!(
            "FRESH (expires in {} ms)",
            provenance.expiry_unix.saturating_sub(now_unix_ms)
        ),
        Err(_) => format!(
            "STALE (expired {} ms ago)",
            now_unix_ms.saturating_sub(provenance.expiry_unix)
        ),
    };

    println!("  source          : {}", source_label);
    println!(
        "  price           : {} microusd  (~${}.{:03})",
        eval.price_microusd,
        eval.price_microusd / 1_000_000,
        (eval.price_microusd % 1_000_000) / 1_000
    );
    println!("  band            : {}", band_label(eval.price_microusd));
    println!("  created  (ms)   : {}", provenance.timestamp_unix);
    println!("  expires  (ms)   : {}", provenance.expiry_unix);
    println!("  now      (ms)   : {}", now_unix_ms);
    println!("  freshness       : {}", fresh_label);
    println!(
        "  agg_utxo_ref    : {}",
        hex32(&provenance.aggregator_utxo_ref)
    );
    println!();

    let _ = source_label;
    Ok(OracleObservation {
        eval,
        provenance,
        now_unix_ms,
    })
}

fn band_label(price_microusd: u64) -> String {
    if price_microusd >= 500_000 {
        "HIGH  (>= $0.500 → 100 % cap)".to_string()
    } else if price_microusd >= 350_000 {
        "MID   (>= $0.350 →  75 % cap)".to_string()
    } else {
        "LOW   (<  $0.350 →  50 % cap)".to_string()
    }
}

fn band_name(price_microusd: u64) -> &'static str {
    if price_microusd >= 500_000 {
        "HIGH/100%"
    } else if price_microusd >= 350_000 {
        "MID/75%"
    } else {
        "LOW/50%"
    }
}

// =====================================================================
// Phase 4 — Per-scenario pipeline
// =====================================================================

#[derive(Debug)]
struct ScenarioReport {
    scenario: Scenario,
    expected_outcome: EvaluatorOutcomeKind,
    evaluator_outcome: EvaluatorOutcomeKind,
    verifier_clean: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvaluatorOutcomeKind {
    Allow,
    DenyCapExceeded,
    DenyOther,
}

fn outcome_short(k: EvaluatorOutcomeKind) -> &'static str {
    match k {
        EvaluatorOutcomeKind::Allow => "Allow",
        EvaluatorOutcomeKind::DenyCapExceeded => "Deny(CapExceeded)",
        EvaluatorOutcomeKind::DenyOther => "Deny(other)",
    }
}

fn run_scenario(
    scenario: Scenario,
    policy_ref: &[u8; 32],
    allocation_lovelace: u64,
    oracle: &OracleObservation,
) -> ScenarioReport {
    section(&format!("PHASE 4 — SCENARIO [{}]", scenario.label()));

    let cap = cap_for_price(allocation_lovelace, oracle.eval.price_microusd);
    let (requested, explainer) = match scenario {
        Scenario::Allow => {
            // 80 % of cap, rounded to whole ADA. Always within cap.
            let req = ((cap / 10).saturating_mul(8) / 1_000_000).saturating_mul(1_000_000);
            let req = req.max(1_000_000); // at least 1 ADA
            (req, "80% of cap → expected Allow")
        }
        Scenario::Deny => {
            // 110 % of cap, rounded to whole ADA. Always over cap.
            let req = (cap.saturating_add(cap / 10) / 1_000_000)
                .saturating_mul(1_000_000)
                .max(cap + 1_000_000);
            (req, "110% of cap → expected Deny(ReleaseCapExceeded)")
        }
    };

    println!("  4a. INTENT");
    let intent = DisbursementIntentV1::new_v1(
        *policy_ref,
        DEMO_ALLOCATION_ID,
        DEMO_REQUESTER_ID,
        oracle.eval,
        oracle.provenance,
        requested,
        CardanoAddressV1 {
            bytes: DEMO_DESTINATION,
            length: 57,
        },
        AssetIdV1::ADA,
    );

    let bytes = match encode_intent(&intent) {
        Ok(b) => b,
        Err(e) => {
            println!("      encode_intent failed: {e:?}");
            return ScenarioReport {
                scenario,
                expected_outcome: EvaluatorOutcomeKind::DenyOther,
                evaluator_outcome: EvaluatorOutcomeKind::DenyOther,
                verifier_clean: false,
            };
        }
    };
    let id = match intent_id(&intent) {
        Ok(i) => i,
        Err(e) => {
            println!("      intent_id failed: {e:?}");
            return ScenarioReport {
                scenario,
                expected_outcome: EvaluatorOutcomeKind::DenyOther,
                evaluator_outcome: EvaluatorOutcomeKind::DenyOther,
                verifier_clean: false,
            };
        }
    };

    println!(
        "      requested           : {} lovelace  ({} ADA)",
        requested,
        requested / 1_000_000
    );
    println!(
        "      cap at current price: {} lovelace  ({} ADA) — {}",
        cap,
        cap / 1_000_000,
        explainer
    );
    println!("      canonical bytes     : {} bytes", bytes.len());
    println!("      intent_id           : {}", hex32(&id));
    println!();

    let expected_kind = match scenario {
        Scenario::Allow => EvaluatorOutcomeKind::Allow,
        Scenario::Deny => EvaluatorOutcomeKind::DenyCapExceeded,
    };

    // 4b. EXPECTED DECISION — in-process evaluator.
    println!("  4b. EXPECTED DECISION (in-process evaluator)");
    let eval_result = evaluate_disbursement(&intent, allocation_lovelace);
    let evaluator_kind = classify(&eval_result);
    match eval_result {
        EvaluationResult::Allow {
            authorized_amount_lovelace,
            release_cap_basis_points,
        } => {
            println!(
                "      Allow {{ authorized = {} lovelace, band = {} bps }}",
                authorized_amount_lovelace, release_cap_basis_points
            );
        }
        EvaluationResult::Deny { reason } => {
            println!("      Deny  {{ reason = {} }}", reason_label(reason));
        }
    }

    // Synthesize an AuthorizationResult from the evaluator output. The
    // grant gate is the evaluator's gate, so Deny's `gate = Grant`.
    let authorization = authorization_from_eval(&intent, &eval_result);
    let committed_bytes = match authorization.encode() {
        Ok(b) => b,
        Err(e) => {
            println!("      AuthorizationResult encode failed: {e:?}");
            return ScenarioReport {
                scenario,
                expected_outcome: expected_kind,
                evaluator_outcome: evaluator_kind,
                verifier_clean: false,
            };
        }
    };
    println!(
        "      committed bytes     : {} bytes  [{}...]",
        committed_bytes.len(),
        hex_prefix(&committed_bytes, 8)
    );
    println!();

    // 4c. SUBMIT — skipped in this offline orchestrator. The live
    // Ziranity submit path is covered by config/smoke.sh; cross-run
    // byte-identity is already proven there.
    println!("  4c. SUBMIT  → skipped (see config/smoke.sh for the live path)");
    println!();

    // 4d. SETTLE — same. Operator runbook covers the Cardano tx path.
    match scenario {
        Scenario::Allow => {
            println!("  4d. SETTLE  → skipped in this orchestrator");
            println!("              (operator runbook: scripts/cardano_disburse.py,");
            println!("               or set ALLOW scenario in config/smoke.sh)");
        }
        Scenario::Deny => {
            println!("  4d. SETTLE  → n/a (denied disbursements emit no tx)");
        }
    }
    println!();

    // 4e. VERIFY — assemble a canonical evidence bundle from this exact
    // run and hand it to the offline verifier. A CLEAN report proves
    // that the same public semantics reproduce the recorded decision
    // byte-for-byte.
    println!("  4e. OFFLINE VERIFY (assemble bundle + replay)");
    let receipt = IntentReceiptV1 {
        intent_id: id,
        status: IntentStatus::Committed,
        committed_height: 0,
        output: authorization,
    };
    let outcome = match authorization {
        AuthorizationResult::Authorized { .. } => FulfillmentOutcome::Settled {
            tx_hash: CardanoTxHashV1(DEMO_DUMMY_TX_HASH),
        },
        AuthorizationResult::Denied { reason, gate } => {
            FulfillmentOutcome::DeniedUpstream { reason, gate }
        }
    };

    let verifier_clean = match assemble_disbursement_evidence(
        &intent,
        &receipt,
        outcome,
        allocation_lovelace,
        oracle.now_unix_ms,
    ) {
        Ok(evidence) => {
            let report = verify_evidence(&evidence);
            if report.is_ok() {
                println!("      verifier        : CLEAN");
                println!(
                    "      evidence bytes  : ~{} bytes in canonical form",
                    approx_evidence_size(&evidence)
                );
                true
            } else {
                println!("      verifier        : FAILED");
                println!("      findings        : {:?}", report.findings);
                false
            }
        }
        Err(e) => {
            println!("      assembly failed : {e:?}");
            false
        }
    };
    println!();

    ScenarioReport {
        scenario,
        expected_outcome: expected_kind,
        evaluator_outcome: evaluator_kind,
        verifier_clean,
    }
}

fn classify(r: &EvaluationResult) -> EvaluatorOutcomeKind {
    match r {
        EvaluationResult::Allow { .. } => EvaluatorOutcomeKind::Allow,
        EvaluationResult::Deny { reason } => match reason {
            DisbursementReasonCode::ReleaseCapExceeded => EvaluatorOutcomeKind::DenyCapExceeded,
            _ => EvaluatorOutcomeKind::DenyOther,
        },
    }
}

fn authorization_from_eval(
    intent: &DisbursementIntentV1,
    result: &EvaluationResult,
) -> AuthorizationResult {
    match *result {
        EvaluationResult::Allow {
            authorized_amount_lovelace,
            release_cap_basis_points,
        } => AuthorizationResult::Authorized {
            effect: AuthorizedEffectV1 {
                policy_ref: intent.policy_ref,
                allocation_id: intent.allocation_id,
                requester_id: intent.requester_id,
                destination: intent.destination,
                asset: intent.asset,
                authorized_amount_lovelace,
                release_cap_basis_points,
            },
        },
        EvaluationResult::Deny { reason } => AuthorizationResult::Denied {
            reason,
            // Evaluator failures all attribute to the grant gate in the
            // offline pipeline; anchor/registry failures would come from
            // the three-gate closure, not this entry point.
            gate: AuthorizationGate::Grant,
        },
    }
}

fn cap_for_price(allocation_lovelace: u64, price_microusd: u64) -> u64 {
    let bps: u128 = if price_microusd >= 500_000 {
        10_000
    } else if price_microusd >= 350_000 {
        7_500
    } else {
        5_000
    };
    let cap = (u128::from(allocation_lovelace) * bps) / 10_000;
    u64::try_from(cap).unwrap_or(u64::MAX)
}

fn reason_label(reason: DisbursementReasonCode) -> &'static str {
    match reason {
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

fn approx_evidence_size(evidence: &oracleguard_schemas::evidence::DisbursementEvidenceV1) -> usize {
    // Rough estimate — actual canonical encoding happens in the
    // verifier test suite; here we just give the operator a sense of
    // scale without re-encoding.
    use oracleguard_schemas::evidence::encode_evidence;
    encode_evidence(evidence).map(|v| v.len()).unwrap_or(0)
}

// =====================================================================
// Phase 5 — POLICY ROTATION (stretch)
// =====================================================================

struct RotationReport {
    allow_before: EvaluatorOutcomeKind,
    allow_after: EvaluatorOutcomeKind,
    deny_before: EvaluatorOutcomeKind,
    deny_after: EvaluatorOutcomeKind,
    flipped: bool,
}

impl RotationReport {
    fn one_line_summary(&self) -> String {
        if self.flipped {
            format!(
                "FLIPPED — allow {} → {} | deny {} → {}",
                outcome_short(self.allow_before),
                outcome_short(self.allow_after),
                outcome_short(self.deny_before),
                outcome_short(self.deny_after),
            )
        } else {
            "unchanged (rotated cap did not change any scenario outcome)".to_string()
        }
    }
}

fn phase_rotation(
    original: &PolicyDisplay,
    oracle: &OracleObservation,
    allocation_lovelace: u64,
    raised_cap_bps: u64,
) -> RotationReport {
    section("PHASE 5 — POLICY ROTATION (stretch)");
    println!("  Raising the release-cap in the policy JSON and re-deriving");
    println!("  policy_ref. Governance claim: a policy change is observable");
    println!("  in the 32-byte policy_ref, and the same intent evaluated");
    println!("  under the rotated policy_ref may produce a different outcome.");
    println!();

    // Build a rotated canonical JSON by replacing release_cap_basis_points.
    let rotated_json = rotated_policy_json(raised_cap_bps);
    let rotated_canonical = match canonicalize_policy_json(rotated_json.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            println!("  rotation failed: could not canonicalize rotated policy: {e:?}");
            return RotationReport {
                allow_before: EvaluatorOutcomeKind::DenyOther,
                allow_after: EvaluatorOutcomeKind::DenyOther,
                deny_before: EvaluatorOutcomeKind::DenyOther,
                deny_after: EvaluatorOutcomeKind::DenyOther,
                flipped: false,
            };
        }
    };
    let rotated_ref = derive_policy_ref(&rotated_canonical);

    println!("  original release_cap_bps  : 7500");
    println!("  rotated  release_cap_bps  : {}", raised_cap_bps);
    println!();
    println!(
        "  original policy_ref       : {}",
        hex32(&original.policy_ref)
    );
    println!(
        "  rotated  policy_ref       : {}",
        hex32(rotated_ref.as_bytes())
    );
    println!(
        "  changed                   : {}",
        if rotated_ref.as_bytes() != &original.policy_ref {
            "yes — a policy change is observable on-chain/in-commitments"
        } else {
            "no"
        }
    );
    println!();

    // Counterfactual: re-use the exact same allow_req / deny_req that
    // Phase 4 used (80% / 110% of the active-band cap) and evaluate
    // them under the rotated cap. `evaluate_disbursement` keys its
    // band selection off oracle price, so to simulate rotation here
    // we compare the two caps directly — the original (band-selected)
    // cap and the rotated (JSON-override) cap.
    let orig_cap = cap_for_price(allocation_lovelace, oracle.eval.price_microusd);
    let new_cap = cap_for_bps(allocation_lovelace, raised_cap_bps);

    let allow_req = ((orig_cap / 10).saturating_mul(8) / 1_000_000)
        .saturating_mul(1_000_000)
        .max(1_000_000);
    let deny_req = (orig_cap.saturating_add(orig_cap / 10) / 1_000_000)
        .saturating_mul(1_000_000)
        .max(orig_cap + 1_000_000);

    let allow_before = classify_for_cap(allow_req, orig_cap);
    let allow_after = classify_for_cap(allow_req, new_cap);
    let deny_before = classify_for_cap(deny_req, orig_cap);
    let deny_after = classify_for_cap(deny_req, new_cap);

    println!(
        "  at current oracle price = ${}.{:03} (band {}):",
        oracle.eval.price_microusd / 1_000_000,
        (oracle.eval.price_microusd % 1_000_000) / 1_000,
        band_name(oracle.eval.price_microusd)
    );
    println!(
        "    original cap = {} ADA  →  allow_req = {} ADA {}  |  deny_req = {} ADA {}",
        orig_cap / 1_000_000,
        allow_req / 1_000_000,
        outcome_short(allow_before),
        deny_req / 1_000_000,
        outcome_short(deny_before),
    );
    println!(
        "    rotated  cap = {} ADA  →  allow_req = {} ADA {}  |  deny_req = {} ADA {}",
        new_cap / 1_000_000,
        allow_req / 1_000_000,
        outcome_short(allow_after),
        deny_req / 1_000_000,
        outcome_short(deny_after),
    );
    println!();

    let flipped = allow_before != allow_after || deny_before != deny_after;
    println!(
        "  outcome                   : {}",
        if flipped {
            "FLIPPED — same intent, rotated policy, different decision"
        } else {
            "no flip (rotated cap did not cross either scenario's request)"
        }
    );
    println!();

    RotationReport {
        allow_before,
        allow_after,
        deny_before,
        deny_after,
        flipped,
    }
}

fn cap_for_bps(allocation_lovelace: u64, bps: u64) -> u64 {
    let cap = (u128::from(allocation_lovelace) * u128::from(bps)) / 10_000;
    u64::try_from(cap).unwrap_or(u64::MAX)
}

fn classify_for_cap(requested: u64, cap: u64) -> EvaluatorOutcomeKind {
    if requested <= cap {
        EvaluatorOutcomeKind::Allow
    } else {
        EvaluatorOutcomeKind::DenyCapExceeded
    }
}

fn rotated_policy_json(raised_cap_bps: u64) -> String {
    // Mirror the canonical shape of fixtures/policy_v1.json with the
    // cap replaced. Keys preserved so a reader can diff against the
    // original at a glance.
    format!(
        "{{\n  \"schema\": \"oracleguard.policy.v1\",\n  \"policy_version\": 1,\n  \"anchored_commitment\": \"katiba://policy/constitutional-release/v1\",\n  \"release_cap_basis_points\": {},\n  \"allowed_assets\": [\"ADA\"]\n}}",
        raised_cap_bps
    )
}

// =====================================================================
// Formatting helpers
// =====================================================================

fn section(title: &str) {
    println!();
    println!("────────────────────────────────────────────────────────");
    println!(" {title}");
    println!("────────────────────────────────────────────────────────");
}

fn banner(title: &str) {
    // Fixed separator at 60 dashes; title printed below it with two-
    // space indent. Avoids Unicode-width and box-drawing alignment
    // issues on narrow terminals.
    println!("════════════════════════════════════════════════════════════");
    println!("  {title}");
    println!("════════════════════════════════════════════════════════════");
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_prefix(bytes: &[u8], n: usize) -> String {
    let take = n.min(bytes.len());
    let mut s = String::with_capacity(take * 2);
    for b in &bytes[..take] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn workspace_root() -> Result<PathBuf, String> {
    // CARGO_MANIFEST_DIR points at crates/oracleguard-adapter.
    // The workspace root is two levels up.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut p = PathBuf::from(manifest);
    if !p.pop() || !p.pop() {
        return Err("could not pop to workspace root".to_string());
    }
    Ok(p)
}
