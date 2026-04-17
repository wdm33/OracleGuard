#!/bin/bash
# Run workspace tests with deterministic thread discipline.
# Single-threaded by default so output ordering and seeded RNGs stay
# reproducible across runs and across CI environments.
set -euo pipefail
exec cargo test --workspace --locked -- --test-threads=1 "$@"
