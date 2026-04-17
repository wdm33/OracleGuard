#!/bin/bash
# OracleGuard Cluster 6 routing/submission-boundary determinism gate.
#
# Runs the integration suite that pins:
#   - routing single-authority invariance
#   - canonical payload emission stability
#   - CLI wrapper argument-construction determinism
#   - consensus-output intake round-trip
#
# If any assertion in `tests/routing_boundary.rs` regresses, this gate
# fails. Architecture claims about the Cluster 6 boundary are proved
# by these tests, not by prose.

set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH" >&2
  exit 2
fi

echo "routing/submission-boundary determinism check: starting"
cargo test -p oracleguard-adapter --test routing_boundary --quiet
echo "routing/submission-boundary determinism check: OK"
