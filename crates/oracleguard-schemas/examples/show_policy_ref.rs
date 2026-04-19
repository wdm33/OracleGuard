//! Show the `policy_ref` for a policy JSON (original or rotated)
//! using the authoritative canonicalization + derivation functions
//! from `oracleguard_schemas::policy`. The rotated version reads the
//! original, replaces `release_cap_basis_points`, and re-derives.
//!
//! Keeps the "policy_ref is a 32-byte sha256 over canonical bytes"
//! claim in exactly one place (the schemas crate). Demo scripts call
//! this binary instead of re-implementing the rule.
//!
//! Usage:
//!     show_policy_ref \
//!       [--original-path fixtures/policy_v1.json] \
//!       [--cap-bps 10000]
//!
//! If `--cap-bps` is supplied, prints the ORIGINAL vs ROTATED side
//! by side. If not, prints only the ORIGINAL.

use std::path::PathBuf;

use oracleguard_schemas::policy::{canonicalize_policy_json, derive_policy_ref};
use serde_json::Value;

fn main() {
    let args = parse_args();

    let raw_original = match std::fs::read(&args.original_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: read {}: {e}", args.original_path.display());
            std::process::exit(1);
        }
    };

    let orig_canon = match canonicalize_policy_json(&raw_original) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: canonicalize original: {e:?}");
            std::process::exit(1);
        }
    };
    let orig_ref = derive_policy_ref(&orig_canon);
    let orig_cap_bps = extract_cap_bps(&raw_original).unwrap_or(0);

    println!("ORIGINAL policy   (release_cap_basis_points = {orig_cap_bps})");
    println!("  canonical bytes : {}", orig_canon.len());
    println!("  policy_ref      : {}", hex32(orig_ref.as_bytes()));

    if let Some(new_cap_bps) = args.cap_bps {
        println!();
        let mut value: Value = match serde_json::from_slice(&raw_original) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error: parse original JSON: {e}");
                std::process::exit(1);
            }
        };
        value["release_cap_basis_points"] = Value::Number(serde_json::Number::from(new_cap_bps));
        let raw_rotated = match serde_json::to_vec(&value) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: serialize rotated: {e}");
                std::process::exit(1);
            }
        };
        let rot_canon = match canonicalize_policy_json(&raw_rotated) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error: canonicalize rotated: {e:?}");
                std::process::exit(1);
            }
        };
        let rot_ref = derive_policy_ref(&rot_canon);
        println!("ROTATED policy    (release_cap_basis_points = {new_cap_bps})");
        println!("  canonical bytes : {}", rot_canon.len());
        println!("  policy_ref      : {}", hex32(rot_ref.as_bytes()));
        println!();
        println!(
            "  changed         : {}",
            if orig_ref.as_bytes() != rot_ref.as_bytes() {
                "yes"
            } else {
                "no"
            }
        );
    }
}

struct Args {
    original_path: PathBuf,
    cap_bps: Option<u64>,
}

fn parse_args() -> Args {
    let mut original_path = PathBuf::from("fixtures/policy_v1.json");
    let mut cap_bps: Option<u64> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--original-path" => {
                let v = it
                    .next()
                    .unwrap_or_else(|| exit_bad("--original-path needs a value"));
                original_path = v.into();
            }
            "--cap-bps" => {
                let v = it
                    .next()
                    .unwrap_or_else(|| exit_bad("--cap-bps needs a value"));
                cap_bps = Some(v.parse().unwrap_or_else(|_| exit_bad("invalid --cap-bps")));
            }
            "-h" | "--help" => {
                eprintln!("see the file header for usage");
                std::process::exit(0);
            }
            other => exit_bad(&format!("unknown flag '{other}'")),
        }
    }
    Args {
        original_path,
        cap_bps,
    }
}

fn exit_bad(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2);
}

fn extract_cap_bps(raw: &[u8]) -> Option<u64> {
    let v: Value = serde_json::from_slice(raw).ok()?;
    v.get("release_cap_basis_points")?.as_u64()
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
