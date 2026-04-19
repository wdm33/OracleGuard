#!/usr/bin/env python3
"""Read the Charli3 on-chain oracle settings (C3CS UTxO) and print the
max-validity-window ceiling the aggregator contract enforces.

The admin-configured `time_uncertainty_aggregation` field is the
exact ceiling checked by the Plutus aggregator's
`validity_range_size <= time_accuracy` rule (lib/services/time.ak,
trace T4). Querying it once at startup lets us set
`odv_validity_length` in the YAML config precisely, rather than
guessing between 120000 and 300000.

Uses the installed charli3_odv_client's OracleSettingsVariant CBOR
codec directly so this helper cannot drift from the on-chain
schema the client itself parses.

Usage:
    .venv/bin/python scripts/show_oracle_settings.py

Exits 0 on success, 1 on any fetch/parse failure.
"""

import json
import sys
import urllib.request

from charli3_odv_client.models.datums import OracleSettingsVariant

KUPO_URL = "http://35.209.192.203:1442"
ORACLE_ADDR = (
    "addr_test1wq3pacs7jcrlwehpuy3ryj8kwvsqzjp9z6dpmx8txnr0vkq6vqeuu"
)
# "C3CS" token asset-name suffix in hex. Assets are referenced in Kupo
# as <policy_id>.<asset_name_hex>; we filter by the hex suffix.
C3CS_HEX = "43334353"


def http_get_json(url: str):
    with urllib.request.urlopen(url, timeout=10) as resp:
        return json.loads(resp.read())


def find_c3cs_utxo():
    matches = http_get_json(f"{KUPO_URL}/matches/{ORACLE_ADDR}?unspent")
    for r in matches:
        assets = r["value"].get("assets", {})
        if any(a.endswith(f".{C3CS_HEX}") for a in assets):
            return r
    raise SystemExit("no C3CS UTxO at oracle address (oracle settings missing?)")


def fetch_datum_cbor(datum_hash: str) -> bytes:
    resp = http_get_json(f"{KUPO_URL}/datums/{datum_hash}")
    return bytes.fromhex(resp["datum"])


def main() -> None:
    utxo = find_c3cs_utxo()
    print(f"C3CS UTxO         : {utxo['transaction_id']}#{utxo['output_index']}")
    print(f"datum hash        : {utxo['datum_hash']}")

    try:
        cbor = fetch_datum_cbor(utxo["datum_hash"])
    except Exception as e:
        print(f"error: fetch datum: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        variant = OracleSettingsVariant.from_cbor(cbor)
        settings = variant.datum
    except Exception as e:
        print(f"error: decode OracleSettingsVariant: {e}", file=sys.stderr)
        sys.exit(1)

    print()
    print("Oracle settings (from on-chain C3CS datum):")
    # Surface the fields that gate submission timing. Other fields on
    # the datum exist (node keys, pause state, etc.) but aren't
    # load-bearing for our demo's decision about odv_validity_length.
    ms = settings.time_uncertainty_aggregation
    print(
        f"  time_uncertainty_aggregation : {ms} ms  ({ms / 1000:g} s)  "
        "← odv_validity_length ceiling"
    )
    ms_p = settings.time_uncertainty_platform
    print(f"  time_uncertainty_platform    : {ms_p} ms  ({ms_p / 1000:g} s)")

    print()
    ceiling = ms
    # Recommend a safety margin so the contract ceiling isn't hit by
    # rounding or a slot-boundary nudge.
    recommended = max(ceiling - 10_000, ceiling // 2)
    print(
        f"  → recommended odv_validity_length  ≤ {ceiling} (chain ceiling)"
    )
    print(
        f"  → safe default                     : {recommended} "
        f"(ceiling minus a ~10 s cushion)"
    )


if __name__ == "__main__":
    main()
