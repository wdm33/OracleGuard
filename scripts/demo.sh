#!/usr/bin/env bash
# OracleGuard demo orchestrator.
#
# Drives the real end-to-end path, in order:
#
#   1. policy      — canonicalize the policy JSON, show policy_ref
#   2. allocation  — show the approved pool → receiver allocation
#   3. oracle      — probe hackathon Kupo for reachability
#   4. scenarios   — run smoke.sh allow + smoke.sh deny against a
#                    4-node Ziranity devnet; PASS means the committed
#                    output bytes match the recorded fixture
#   5. settle      — if allow passed, submit a real Cardano Preprod
#                    disbursement tx via scripts/cardano_disburse.py
#   6. verify      — replay all recorded evidence bundles through the
#                    offline verifier (cargo test)
#   7. rotate      — (optional) show that raising the cap produces a
#                    new policy_ref
#
# USAGE
#   scripts/demo.sh                  # full live demo
#   scripts/demo.sh --dry            # no devnet, no settlement
#   scripts/demo.sh --rotate         # include the policy-rotation phase
#   scripts/demo.sh --dry --rotate   # narrative + offline verify + rotate
#
# PREREQ (for live mode)
#   - ~/.local/opt/ziranity-v1.1.0-oracleguard-linux-x86_64/config/smoke.sh
#   - .venv/bin/python with pycardano  (see scripts/preflight.sh)
#   - POOL_MNEMONIC exported (only needed for §5 settlement)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SMOKE="$HOME/.local/opt/ziranity-v1.1.0-oracleguard-linux-x86_64/config/smoke.sh"
VENV_PY="$REPO_ROOT/.venv/bin/python"

POOL_ADDR="addr_test1qz4f2vac8nn7tp802mxj3cu40a7xhhzc3agut6spq6rpz5rgtlvyed9yn3ncuv3fgaadfmvn64d7egjn824t7pj99xfs4y58d0"
RECEIVER_HEX="000ee03e45b05d6225eb7143d2be23a3b8629b4d9e61621bb30afcc53a95474e79d1298d291e200a2c889fe13b97785cc41279629d99cfb87d"
OGMIOS_URL="http://35.209.192.203:1337"
KUPO_URL="http://35.209.192.203:1442"

DRY=0
ROTATE=0
for a in "$@"; do
  case "$a" in
    --dry)    DRY=1 ;;
    --rotate) ROTATE=1 ;;
    --help|-h)
      sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) echo "unknown flag: $a" >&2; exit 2 ;;
  esac
done

phase() { printf '\n─── %s ───────────────────────────────────────────\n' "$1"; }

# ---- 1. POLICY ----
phase "1. POLICY"
sed 's/^/  /' fixtures/policy_v1.json
POLICY_REF=$(sha256sum fixtures/policy_v1.canonical.bytes | awk '{print $1}')
echo "  canonical bytes : $(wc -c < fixtures/policy_v1.canonical.bytes) bytes"
echo "  policy_ref      : $POLICY_REF"

# ---- 2. ALLOCATION ----
phase "2. ALLOCATION"
echo "  pool            : test_wallet_1 (Preprod)"
echo "  approved        : 1000 ADA"
echo "  receiver        : test_wallet_3 (Preprod)"

# ---- 3. ORACLE (reachability probe) ----
phase "3. ORACLE (Charli3 ADA/USD, live Preprod)"
if curl -sSfL --max-time 5 "$KUPO_URL/health" >/dev/null 2>&1; then
  echo "  Kupo            : $KUPO_URL  [OK]"
else
  echo "  Kupo            : $KUPO_URL  [unreachable]"
fi

# ---- 4. SCENARIOS ----
ALLOW_OK=0
DENY_OK=0

run_smoke() {
  local name="$1"
  phase "4. SCENARIO — $name"
  if [ "$DRY" = 1 ]; then
    echo "  (--dry) skipping Ziranity devnet submit"
    return 0
  fi
  if [ ! -x "$SMOKE" ]; then
    echo "  smoke.sh not found at:"
    echo "    $SMOKE"
    echo "  → skipping (install the Ziranity handover bundle)"
    return 1
  fi
  "$SMOKE" "$name"
}

if run_smoke allow; then ALLOW_OK=1; fi || true
if run_smoke deny;  then DENY_OK=1;  fi || true

# ---- 5. CARDANO SETTLEMENT (allow only) ----
TX_ID=""
if [ "$DRY" = 0 ] && [ "$ALLOW_OK" = 1 ]; then
  phase "5. CARDANO SETTLEMENT (allow path)"
  if [ -z "${POOL_MNEMONIC:-}" ]; then
    echo "  POOL_MNEMONIC not set → skipping on-chain settlement"
  elif [ ! -x "$VENV_PY" ]; then
    echo "  .venv/bin/python not found → skipping on-chain settlement"
  else
    echo "  Submitting 700 ADA to wallet_3 via Ogmios ($OGMIOS_URL)..."
    set +e
    TX_ID="$("$VENV_PY" scripts/cardano_disburse.py \
      --ogmios-url "$OGMIOS_URL" \
      --pool-address "$POOL_ADDR" \
      --destination-bytes-hex "$RECEIVER_HEX" \
      --destination-length 57 \
      --amount-lovelace 700000000 \
      --intent-id "$(printf 'dd%.0s' {1..32})" 2>&1 | tail -1)"
    rc=$?
    set -e
    if [ $rc -ne 0 ] || [ -z "$TX_ID" ] || ! [[ "$TX_ID" =~ ^[0-9a-f]{64}$ ]]; then
      echo "  settlement failed (rc=$rc)"
      TX_ID=""
    else
      echo "  tx_id           : $TX_ID"
      echo "  cexplorer       : https://preprod.cexplorer.io/tx/$TX_ID"
    fi
  fi
fi

# ---- 6. OFFLINE VERIFIER ----
phase "6. OFFLINE VERIFIER"
echo "  Replaying 4 recorded evidence bundles..."
VERIFIER_OUT="$(cargo test -p oracleguard-verifier verify_bundle_reports_clean --quiet 2>&1 || true)"
if echo "$VERIFIER_OUT" | grep -q "test result: ok"; then
  echo "  CLEAN — all bundles reproduce byte-for-byte"
else
  echo "  FAILED"
  echo "$VERIFIER_OUT" | sed 's/^/    /'
  exit 1
fi

# ---- 7. POLICY ROTATION (stretch) ----
if [ "$ROTATE" = 1 ]; then
  phase "7. POLICY ROTATION (stretch)"
  ROTATED_REF=$(python3 - <<'EOF'
import json, hashlib
rotated = {
    "schema": "oracleguard.policy.v1",
    "policy_version": 1,
    "anchored_commitment": "katiba://policy/constitutional-release/v1",
    "release_cap_basis_points": 10000,
    "allowed_assets": ["ADA"],
}
canon = json.dumps(rotated, sort_keys=True, separators=(',', ':')).encode()
print(hashlib.sha256(canon).hexdigest())
EOF
)
  echo "  cap_bps 7500 → 10000   (policy rule change)"
  echo "  original policy_ref : $POLICY_REF"
  echo "  rotated  policy_ref : $ROTATED_REF"
  echo "  → a policy change is observable via 32-byte policy_ref"
fi

# ---- SUMMARY ----
phase "SUMMARY"
printf '  policy_ref   : %s\n' "$POLICY_REF"
if [ "$DRY" = 1 ]; then
  printf '  mode         : DRY (no devnet, no settlement)\n'
else
  printf '  allow smoke  : %s\n' "$([ "$ALLOW_OK" = 1 ] && echo PASS || echo 'FAIL/SKIPPED')"
  printf '  deny smoke   : %s\n'  "$([ "$DENY_OK"  = 1 ] && echo PASS || echo 'FAIL/SKIPPED')"
  if [ -n "$TX_ID" ]; then
    printf '  cardano tx   : %s\n' "$TX_ID"
  fi
fi
printf '  verifier     : CLEAN\n'
echo
