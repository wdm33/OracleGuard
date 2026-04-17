#!/bin/bash
# OracleGuard dependency-direction enforcer.
#
# Fails if the workspace introduces an edge forbidden by the architecture:
#
#   - oracleguard-schemas MUST NOT depend on any other oracleguard-* crate.
#   - oracleguard-policy MUST NOT depend on oracleguard-adapter or
#     oracleguard-verifier.
#   - oracleguard-adapter MAY depend on oracleguard-schemas and
#     oracleguard-policy only (no verifier).
#   - oracleguard-verifier MAY depend on oracleguard-schemas and
#     oracleguard-policy only (no adapter).
#   - No crate may depend on a private / out-of-tree path dependency.
#
# This check is canonical. Architecture docs describe the rule; this
# script enforces it. If they diverge, update the docs or the script.
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo not found on PATH" >&2
  exit 2
fi

# Extract: <crate_name>\t<dep_name>\t<dep_path_or_empty>
edges=$(cargo metadata --format-version=1 --no-deps | python3 -c '
import json, sys
meta = json.load(sys.stdin)
for pkg in meta.get("packages", []):
    name = pkg.get("name", "")
    for dep in pkg.get("dependencies", []):
        dep_name = dep.get("name", "")
        dep_path = dep.get("path") or ""
        print(f"{name}\t{dep_name}\t{dep_path}")
')

fail=0
fail_edge() {
  echo "FORBIDDEN DEPENDENCY: $1 -> $2   ($3)" >&2
  fail=1
}

workspace_crates=(oracleguard-schemas oracleguard-policy oracleguard-adapter oracleguard-verifier)
is_workspace_crate() {
  local n="$1"
  for c in "${workspace_crates[@]}"; do
    [ "$n" = "$c" ] && return 0
  done
  return 1
}

while IFS=$'\t' read -r from to dep_path; do
  [ -z "$from" ] && continue
  case "$from" in
    oracleguard-*) ;;
    *) continue ;;
  esac

  # Forbid any path dependency that points outside this workspace tree.
  if [ -n "$dep_path" ]; then
    case "$dep_path" in
      */OracleGuard/crates/*) ;;  # in-tree
      *)
        fail_edge "$from" "$to" "out-of-tree path dependency at $dep_path"
        continue
        ;;
    esac
  fi

  # Only inspect workspace-internal edges from here on.
  is_workspace_crate "$to" || continue

  case "$from" in
    oracleguard-schemas)
      # Schemas is at the root of authority. No internal deps allowed.
      fail_edge "$from" "$to" "schemas must not depend on any workspace crate"
      ;;
    oracleguard-policy)
      case "$to" in
        oracleguard-schemas) ;;
        *) fail_edge "$from" "$to" "policy may depend only on schemas" ;;
      esac
      ;;
    oracleguard-adapter)
      case "$to" in
        oracleguard-schemas|oracleguard-policy) ;;
        *) fail_edge "$from" "$to" "adapter may depend only on schemas and policy" ;;
      esac
      ;;
    oracleguard-verifier)
      case "$to" in
        oracleguard-schemas|oracleguard-policy) ;;
        *) fail_edge "$from" "$to" "verifier may depend only on schemas and policy" ;;
      esac
      ;;
  esac
done <<< "$edges"

if [ "$fail" -ne 0 ]; then
  echo
  echo "Dependency-direction check FAILED." >&2
  echo "See docs/architecture.md for the authoritative ownership table." >&2
  exit 1
fi

echo "dependency-direction: OK"
