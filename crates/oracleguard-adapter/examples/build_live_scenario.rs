//! Build live allow + deny scenario artifacts from the current
//! on-chain Charli3 AggState, then stage the resulting canonical
//! `DisbursementIntentV1` + predicted `AuthorizationResult` postcard
//! files so `smoke.sh` can submit them and byte-diff consensus
//! output against our offline prediction.
//!
//! Architecture boundary this enforces:
//!
//!   - the **policy** and **policy_ref** stay fixed across runs
//!   - the **oracle fact** varies — it's the input the governance
//!     rule consumes
//!   - the **active band + cap** is derived from the live price
//!   - the **scenario amount** is sized to land inside (allow) or
//!     outside (deny) that live-derived cap
//!   - the offline evaluator predicts the authorization result
//!   - smoke.sh proves consensus reproduces that prediction byte-for-byte
//!
//! Usage:
//!     build_live_scenario \
//!       --template-intent   <path to allow_700_ada_intent.postcard> \
//!       --allocation-lovelace 1000000000 \
//!       --allow-intent-out  <path> \
//!       --allow-result-out  <path> \
//!       --deny-intent-out   <path> \
//!       --deny-result-out   <path>
//!
//! On success prints key=value lines demo.sh sources:
//!     allow_intent_id=<64 hex>
//!     allow_amount_lovelace=<u64>
//!     allow_band_bps=<u64>
//!     allow_verdict=Allow
//!     deny_intent_id=<64 hex>
//!     deny_amount_lovelace=<u64>
//!     deny_band_bps=<u64>
//!     deny_verdict=Deny:ReleaseCapExceeded
//!     oracle_price_microusd=<u64>
//!     oracle_created_ms=<u64>
//!     oracle_expiry_ms=<u64>
//!     oracle_aggregator_utxo_ref=<64 hex>

use std::path::PathBuf;

use oracleguard_adapter::charli3::normalize_aggstate_datum;
use oracleguard_adapter::kupo::{
    fetch_aggstate_datum, KupoEndpoint, CHARLI3_ADA_USD_ASSET_KEY,
    CHARLI3_ADA_USD_ORACLE_ADDRESS, DEFAULT_TIMEOUT,
};
use oracleguard_policy::authorize::AuthorizationResult;
use oracleguard_policy::evaluate::evaluate_disbursement;
use oracleguard_policy::EvaluationResult;
use oracleguard_schemas::effect::AuthorizedEffectV1;
use oracleguard_schemas::encoding::{decode_intent, encode_intent, intent_id};
use oracleguard_schemas::gate::AuthorizationGate;
use oracleguard_schemas::intent::DisbursementIntentV1;
use oracleguard_schemas::oracle::{OracleFactEvalV1, OracleFactProvenanceV1};
use oracleguard_schemas::reason::DisbursementReasonCode;

#[derive(Debug, Clone, Copy)]
enum Scenario {
    Allow,
    Deny,
}

impl Scenario {
    fn prefix(self) -> &'static str {
        match self {
            Scenario::Allow => "allow",
            Scenario::Deny => "deny",
        }
    }
}

struct Args {
    template_intent: PathBuf,
    allocation_lovelace: u64,
    allow_intent_out: PathBuf,
    allow_result_out: PathBuf,
    deny_intent_out: PathBuf,
    deny_result_out: PathBuf,
    kupo_url: Option<String>,
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };

    // 1. Fetch the current on-chain AggState via Kupo.
    let endpoint = match args.kupo_url.as_deref() {
        Some(url) => KupoEndpoint::new(url.to_string()),
        None => KupoEndpoint::hackathon_preprod(),
    };
    let fetched = match fetch_aggstate_datum(
        &endpoint,
        CHARLI3_ADA_USD_ORACLE_ADDRESS,
        CHARLI3_ADA_USD_ASSET_KEY,
        DEFAULT_TIMEOUT,
    ) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: kupo fetch failed: {e:?}");
            std::process::exit(1);
        }
    };

    let (eval_fact, provenance) =
        match normalize_aggstate_datum(&fetched.cbor_bytes, fetched.aggregator_utxo_ref) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error: normalize failed: {e:?}");
                std::process::exit(1);
            }
        };

    // 2. Load the template intent (for policy_ref, allocation_id,
    //    requester_id, destination, asset — demo constants).
    let template_bytes = match std::fs::read(&args.template_intent) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "error: read template {}: {e}",
                args.template_intent.display()
            );
            std::process::exit(1);
        }
    };
    let template = match decode_intent(&template_bytes) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: decode template: {e:?}");
            std::process::exit(1);
        }
    };

    // 3. Build both scenarios from the same live oracle fact.
    let cap = cap_for_price(args.allocation_lovelace, eval_fact.price_microusd);

    let allow = build(
        Scenario::Allow,
        &template,
        eval_fact,
        provenance,
        args.allocation_lovelace,
        cap,
    );
    let deny = build(
        Scenario::Deny,
        &template,
        eval_fact,
        provenance,
        args.allocation_lovelace,
        cap,
    );

    // 4. Write the four canonical postcards.
    write(&args.allow_intent_out, &allow.intent_bytes);
    write(&args.allow_result_out, &allow.result_bytes);
    write(&args.deny_intent_out, &deny.intent_bytes);
    write(&args.deny_result_out, &deny.result_bytes);

    // 5. Print structured output for demo.sh to source.
    println!("allow_intent_id={}", hex32(&allow.intent_id));
    println!("allow_amount_lovelace={}", allow.amount);
    println!("allow_band_bps={}", allow.band_bps);
    println!("allow_verdict={}", allow.verdict);
    println!("deny_intent_id={}", hex32(&deny.intent_id));
    println!("deny_amount_lovelace={}", deny.amount);
    println!("deny_band_bps={}", deny.band_bps);
    println!("deny_verdict={}", deny.verdict);
    println!("oracle_price_microusd={}", eval_fact.price_microusd);
    println!("oracle_created_ms={}", provenance.timestamp_unix);
    println!("oracle_expiry_ms={}", provenance.expiry_unix);
    println!(
        "oracle_aggregator_utxo_ref={}",
        hex32(&provenance.aggregator_utxo_ref)
    );
}

struct ScenarioOut {
    intent_bytes: Vec<u8>,
    result_bytes: Vec<u8>,
    intent_id: [u8; 32],
    amount: u64,
    band_bps: u64,
    verdict: String,
}

fn build(
    scenario: Scenario,
    template: &DisbursementIntentV1,
    eval_fact: OracleFactEvalV1,
    provenance: OracleFactProvenanceV1,
    allocation_lovelace: u64,
    cap: u64,
) -> ScenarioOut {
    let amount = pick_amount(scenario, cap);

    let mut intent = *template;
    intent.oracle_fact = eval_fact;
    intent.oracle_provenance = provenance;
    intent.requested_amount_lovelace = amount;

    let intent_bytes = match encode_intent(&intent) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: encode {} intent: {e:?}", scenario.prefix());
            std::process::exit(1);
        }
    };
    let id = match intent_id(&intent) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("error: {} intent_id: {e:?}", scenario.prefix());
            std::process::exit(1);
        }
    };

    let eval_result = evaluate_disbursement(&intent, allocation_lovelace);
    let (authorization, band_bps, verdict) = match eval_result {
        EvaluationResult::Allow {
            authorized_amount_lovelace,
            release_cap_basis_points,
        } => (
            AuthorizationResult::Authorized {
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
            release_cap_basis_points,
            "Allow".to_string(),
        ),
        EvaluationResult::Deny { reason } => (
            AuthorizationResult::Denied {
                reason,
                gate: AuthorizationGate::Grant,
            },
            // band_bps still meaningful: it's what the current price band
            // would cap at, even though the grant gate denied.
            cap_bps_for(eval_fact.price_microusd),
            format!("Deny:{}", reason_name(reason)),
        ),
    };
    let result_bytes = match authorization.encode() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: encode {} result: {e:?}", scenario.prefix());
            std::process::exit(1);
        }
    };

    ScenarioOut {
        intent_bytes,
        result_bytes,
        intent_id: id,
        amount,
        band_bps,
        verdict,
    }
}

fn cap_for_price(allocation_lovelace: u64, price_microusd: u64) -> u64 {
    let bps = cap_bps_for(price_microusd);
    let cap = (u128::from(allocation_lovelace) * u128::from(bps)) / 10_000;
    u64::try_from(cap).unwrap_or(u64::MAX)
}

fn cap_bps_for(price_microusd: u64) -> u64 {
    if price_microusd >= 500_000 {
        10_000
    } else if price_microusd >= 350_000 {
        7_500
    } else {
        5_000
    }
}

fn pick_amount(scenario: Scenario, cap: u64) -> u64 {
    match scenario {
        Scenario::Allow => {
            // 80% of cap, rounded down to whole ADA, minimum 1 ADA.
            let v = (cap / 10).saturating_mul(8) / 1_000_000 * 1_000_000;
            v.max(1_000_000)
        }
        Scenario::Deny => {
            // 110% of cap, rounded down to whole ADA, strictly above cap.
            let v = cap.saturating_add(cap / 10) / 1_000_000 * 1_000_000;
            v.max(cap + 1_000_000)
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

fn write(path: &PathBuf, bytes: &[u8]) {
    if let Err(e) = std::fs::write(path, bytes) {
        eprintln!("error: write {}: {e}", path.display());
        std::process::exit(1);
    }
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn parse_args() -> Result<Args, String> {
    let mut template_intent: Option<PathBuf> = None;
    let mut allocation_lovelace: u64 = 1_000_000_000;
    let mut allow_intent_out: Option<PathBuf> = None;
    let mut allow_result_out: Option<PathBuf> = None;
    let mut deny_intent_out: Option<PathBuf> = None;
    let mut deny_result_out: Option<PathBuf> = None;
    let mut kupo_url: Option<String> = None;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--template-intent" => {
                template_intent = Some(it.next().ok_or("--template-intent needs a value")?.into())
            }
            "--allocation-lovelace" => {
                let v = it.next().ok_or("--allocation-lovelace needs a value")?;
                allocation_lovelace = v
                    .parse()
                    .map_err(|_| format!("invalid --allocation-lovelace '{v}'"))?;
            }
            "--allow-intent-out" => {
                allow_intent_out =
                    Some(it.next().ok_or("--allow-intent-out needs a value")?.into())
            }
            "--allow-result-out" => {
                allow_result_out =
                    Some(it.next().ok_or("--allow-result-out needs a value")?.into())
            }
            "--deny-intent-out" => {
                deny_intent_out = Some(it.next().ok_or("--deny-intent-out needs a value")?.into())
            }
            "--deny-result-out" => {
                deny_result_out = Some(it.next().ok_or("--deny-result-out needs a value")?.into())
            }
            "--kupo-url" => kupo_url = Some(it.next().ok_or("--kupo-url needs a value")?),
            "-h" | "--help" => {
                eprintln!("see the file header for usage");
                std::process::exit(0);
            }
            other => return Err(format!("unknown flag '{other}'")),
        }
    }

    Ok(Args {
        template_intent: template_intent.ok_or("--template-intent required")?,
        allocation_lovelace,
        allow_intent_out: allow_intent_out.ok_or("--allow-intent-out required")?,
        allow_result_out: allow_result_out.ok_or("--allow-result-out required")?,
        deny_intent_out: deny_intent_out.ok_or("--deny-intent-out required")?,
        deny_result_out: deny_result_out.ok_or("--deny-result-out required")?,
        kupo_url,
    })
}
