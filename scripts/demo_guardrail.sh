#!/bin/bash
# Negative demo: prove the core-crate float_arithmetic deny actually fires.
#
# Injects a banned construct into oracleguard-policy/src/lib.rs, runs
# clippy with -D warnings, asserts failure, and restores the file. For
# local spot-checking; do not wire into CI.
set -euo pipefail

cd "$(dirname "$0")/.."

target="crates/oracleguard-policy/src/lib.rs"
backup="$(mktemp)"
cp "$target" "$backup"
restore() { cp "$backup" "$target"; rm -f "$backup"; }
trap restore EXIT

cat >> "$target" <<'EOF'

fn __guardrail_demo_violation() {
    let _ = 1.0_f64 + 2.0_f64;
}
EOF

echo "Injected forbidden construct (float arithmetic) into $target"
echo "Expecting 'cargo clippy -p oracleguard-policy -- -D warnings' to FAIL..."

if cargo clippy -p oracleguard-policy --all-targets -- -D warnings >/dev/null 2>&1; then
  echo "GUARDRAIL DEMO FAILED: clippy accepted banned construct." >&2
  exit 1
fi

echo "OK: clippy correctly rejected float arithmetic in policy."
