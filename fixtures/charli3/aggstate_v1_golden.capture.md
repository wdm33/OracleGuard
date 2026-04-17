# AggState Golden Fixture — Capture Record

This file records the exact on-chain source of the golden Charli3
AggState datum used by the Cluster 3 Slice 02 normalization tests.
The datum bytes themselves are in `aggstate_v1_golden.cbor`.

## Source

| Field | Value |
|---|---|
| Network | Cardano Preprod |
| Captured at (UTC) | 2026-04-17 |
| Kupo endpoint | `http://35.209.192.203:1442` (public hackathon instance) |
| Oracle address | `addr_test1wq3pacs7jcrlwehpuy3ryj8kwvsqzjp9z6dpmx8txnr0vkq6vqeuu` |
| Policy id | `886dcb2363e160c944e63cf544ce6f6265b22ef7c4e2478dd975078e` |
| Asset name (hex) | `43334153` (ASCII `C3AS`) |
| Transaction id | `d44b5a1d7f41e8d2...` (prefix; full value fetched from Kupo) |
| Output index | `1` |
| Datum hash | `c41e7cfd4a6080e8e703807bc3dec2fcd8073773309f594d1a83ee39b1c35fb0` |

## Datum bytes

Hex: `d8799fd87b9fa3001a0003efd0011b0000019d9b68b1d8021b0000019d9b71d998ffff`

## Decoded structure

```
Constr 0 wrapping Constr 2 wrapping { 0: price_microusd,
                                       1: created_ms,
                                       2: expiry_ms }
```

Verified decoded values at capture time:

| Field | Value | Interpretation |
|---|---|---|
| `price_map[0]` | `258_000` | ADA at $0.258 USD (microusd integer) |
| `price_map[1]` | `1_776_428_823_000` | creation time, posix ms — 2026-04-17 UTC |
| `price_map[2]` | `1_776_429_423_000` | expiry time, posix ms (creation + 600_000 ms) |
| Validity window | `600_000` ms | 600 s / 10 min, matches Implementation Plan §12 |

## Reproducing the capture

```
curl -sS 'http://35.209.192.203:1442/matches/addr_test1wq3pacs7jcrlwehpuy3ryj8kwvsqzjp9z6dpmx8txnr0vkq6vqeuu?unspent' \
  | jq -r '.[] | select(.value.assets | keys[] | endswith(".43334153")) | .datum_hash' \
  | head -1 \
  | xargs -I {} curl -sS 'http://35.209.192.203:1442/datums/{}'
```

The UTxO set rolls forward; the specific `transaction_id` above may be
spent. Kupo's `/datums/<hash>` endpoint continues to return the datum
so long as the hash was ever observed, which keeps this fixture
reproducible even after the source UTxO is consumed.

## Why this fixture is authoritative for tests

- captured from the same public Charli3 feed referenced by the
  OracleGuard Implementation Plan §12
- price value `258_000` matches the plan's documented on-chain median
- validity window of 600_000 ms matches the plan's 600-second window
- outer CBOR frame (`Constr 0 { Constr 2 { map } }`) matches every
  other AggState UTxO observed at this address; changing to a
  different frame would be a feed-level schema change
