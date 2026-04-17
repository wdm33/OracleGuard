#!/bin/bash
# OracleGuard Cluster 7 fulfillment-boundary determinism gate.
#
# Runs the integration suite that pins:
#   - exact effect-to-settlement field copy-through
#   - ADA-only MVP asset guard
#   - deny/no-tx closure across every canonical reason code
#   - pending-receipt closure
#   - allow-path tx-hash capture via a deterministic fixture backend
#   - end-to-end Cluster 6 → Cluster 7 byte stability
#
# If any assertion in `tests/fulfillment_boundary.rs` regresses, this
# gate fails. Architecture claims about the Cluster 7 fulfillment
# boundary are proved by these tests, not by prose.

set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH" >&2
  exit 2
fi

echo "fulfillment-boundary determinism check: starting"
cargo test -p oracleguard-adapter --test fulfillment_boundary --quiet
echo "fulfillment-boundary determinism check: OK"
