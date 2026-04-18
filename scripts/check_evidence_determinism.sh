#!/bin/bash
# OracleGuard Cluster 8 evidence-bundle / offline-verifier determinism gate.
#
# Runs the integration suite that pins:
#   - every MVP evidence fixture verifies clean through the public pipeline
#   - verify_bundle is byte-identically reproducible across runs
#   - fixture re-encoding round-trips without drift
#   - the public evaluator reproduces recorded authorization snapshots
#   - recorded intent_id equals recomputed intent_id on every fixture
#   - mutating any authoritative field produces at least one typed finding
#   - malformed input surfaces typed errors without panicking
#
# If any assertion in `tests/verification_surface.rs` regresses, this
# gate fails. Architecture claims about the Cluster 8 verification
# surface are proved by these tests, not by prose.

set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH" >&2
  exit 2
fi

echo "evidence-bundle determinism check: starting"
cargo test -p oracleguard-verifier --test verification_surface --quiet -- --test-threads=1
echo "evidence-bundle determinism check: OK"
