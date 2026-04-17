#!/bin/bash
# Negative demonstration for scripts/check_deps.sh.
#
# Proves the dependency-direction enforcer is real, not aspirational.
# Temporarily injects a forbidden edge (policy -> adapter) into
# oracleguard-policy/Cargo.toml, runs the check, asserts it fails, then
# restores the original manifest.
#
# This script is for human spot-checking only. It must NOT run in CI
# (CI runs check_deps.sh itself against the real manifests).
set -euo pipefail

cd "$(dirname "$0")/.."

manifest="crates/oracleguard-policy/Cargo.toml"
backup="$(mktemp)"
cp "$manifest" "$backup"
restore() { cp "$backup" "$manifest"; rm -f "$backup"; }
trap restore EXIT

# Inject the forbidden edge.
cat >> "$manifest" <<'EOF'

# Injected by check_deps_negative_demo.sh — must be removed on exit.
oracleguard-adapter = { path = "../oracleguard-adapter" }
EOF

echo "Injected forbidden edge: oracleguard-policy -> oracleguard-adapter"
echo "Expecting scripts/check_deps.sh to FAIL..."

if ./scripts/check_deps.sh >/dev/null 2>&1; then
  echo "NEGATIVE DEMO FAILED: check_deps.sh accepted a forbidden edge." >&2
  exit 1
fi

echo "OK: check_deps.sh correctly rejected the forbidden edge."
