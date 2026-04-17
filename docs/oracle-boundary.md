# Oracle Fact Boundary

This document defines the structural split between oracle data that may
affect authorization and oracle data that is audit-only. The split is
the central invariant of Cluster 3:

> Only canonical oracle evaluation fields may affect authorization;
> provenance remains auditable but semantically inert.

The rule is enforced by having two distinct types in
`oracleguard-schemas`. The evaluator consumes only the eval type; the
provenance type rides alongside for evidence.

## Evaluation-domain fact — `OracleFactEvalV1`

Every field here may legitimately influence an authorization decision.
Every field is fixed-width, so canonical byte identity does not depend
on heap layout.

| Field            | Width       | Role                                                                                  |
|------------------|-------------|---------------------------------------------------------------------------------------|
| `asset_pair`     | `[u8; 16]`  | Canonical asset-pair identifier (e.g. `"ADA/USD"` padded to 16 with zero bytes).      |
| `price_microusd` | `u64`       | Integer micro-USD price. `1_000_000` microusd = 1 USD. No floating point, ever.       |
| `source`         | `[u8; 16]`  | Canonical oracle-source identifier (e.g. `"charli3"` padded to 16 with zero bytes).   |

## Provenance-domain fact — `OracleFactProvenanceV1`

Every field here is retained for audit and evidence only. **No field
in this struct may influence an authorization decision.** Structural
separation is the enforcement mechanism.

| Field                 | Width       | Role                                                                                     |
|-----------------------|-------------|------------------------------------------------------------------------------------------|
| `timestamp_unix`      | `u64`       | Oracle creation time, posix milliseconds. Audit-only.                                    |
| `expiry_unix`         | `u64`       | Oracle expiry, posix milliseconds. Input to the freshness check ONLY — never to math.    |
| `aggregator_utxo_ref` | `[u8; 32]`  | UTxO reference pointing at the Charli3 `AggState` UTxO the adapter read. Audit-only.     |

### Freshness check is not authorization-relevant math

`expiry_unix` is used by the adapter to reject stale datums before they
enter any authoritative path (producing `DisbursementReasonCode::OracleStale`).
A stale datum never reaches the evaluator at all. Once a datum is
accepted, `expiry_unix` plays no further role — the release-cap math
sees only `price_microusd`.

## Canonical identifier mapping

Two 16-byte identifiers are pinned in `oracleguard_schemas::oracle`:

| Label       | Constant               | Bytes                                |
|-------------|------------------------|--------------------------------------|
| `"ADA/USD"` | `ASSET_PAIR_ADA_USD`   | `b"ADA/USD"` + nine zero bytes       |
| `"charli3"` | `SOURCE_CHARLI3`       | `b"charli3"` + nine zero bytes       |

`canonical_asset_pair` and `canonical_source` are the only sanctioned
path from a human-readable label to these constants, and they accept
only the exact strings in the table above. No casing variant, no
whitespace-padded form, no punctuation variant (`ADA-USD`, `ADA_USD`)
is recognized. Any label outside this table returns `None` and must
not reach the evaluator.

Adding a new pair or source is a schema-surface change: it requires
both a new constant, a new match arm in the relevant helper, an
updated row in this table, and a test covering the new mapping.

## Cardano Preprod mapping (Charli3)

| On-chain field       | Domain          | Maps to                                                |
|----------------------|-----------------|--------------------------------------------------------|
| `AggState.price_map[0]` | eval         | `OracleFactEvalV1.price_microusd` (already integer microusd, no conversion) |
| `AggState.price_map[1]` | provenance   | `OracleFactProvenanceV1.timestamp_unix` (posix ms)     |
| `AggState.price_map[2]` | provenance   | `OracleFactProvenanceV1.expiry_unix` (posix ms)        |
| UTxO reference       | provenance      | `OracleFactProvenanceV1.aggregator_utxo_ref`           |

## Non-interference

Changes to any provenance-only field MUST NOT alter:

- the canonical encoding of the identity-bearing portion of a
  `DisbursementIntentV1` (see `oracleguard_schemas::encoding::encode_intent_identity`);
- the value of `oracleguard_schemas::encoding::intent_id`;
- the `OracleFactEvalV1` produced by
  `oracleguard_adapter::charli3::normalize_aggstate_datum` for the same
  on-chain price.

These invariants are enforced mechanically. The test map below pins
each provenance mutation to the test that locks its non-interference:

| Provenance mutation | Locked by |
|---|---|
| `oracle_provenance.timestamp_unix` → any value | `oracleguard_schemas::encoding::tests::oracle_provenance_timestamp_does_not_change_intent_id`; `tests/provenance_non_interference.rs::identity_bytes_are_invariant_under_timestamp_mutation` |
| `oracle_provenance.expiry_unix` → any value | `oracleguard_schemas::encoding::tests::oracle_provenance_expiry_does_not_change_intent_id`; `tests/provenance_non_interference.rs::identity_bytes_are_invariant_under_expiry_mutation` |
| `oracle_provenance.aggregator_utxo_ref` → any value | `oracleguard_schemas::encoding::tests::oracle_provenance_utxo_ref_does_not_change_intent_id`; `tests/provenance_non_interference.rs::identity_bytes_are_invariant_under_utxo_ref_mutation`; `oracleguard_adapter::charli3::tests::utxo_ref_does_not_change_eval` |
| CBOR datum provenance (`price_map[1]`, `price_map[2]`) → any value | `oracleguard_adapter::charli3::tests::provenance_only_datum_mutations_do_not_change_eval`; `provenance_only_datum_mutations_yield_identical_eval_canonical_bytes` |

Dual: the full canonical encoding of the intent (not its identity
projection) IS provenance-sensitive by design, so evidence bundles
retain the provenance bytes. This is locked by
`tests/provenance_non_interference.rs::full_intent_bytes_change_when_provenance_changes`.

## Intent construction pattern

The sanctioned pipeline for building a `DisbursementIntentV1` from a
Charli3 AggState datum is:

```
raw CBOR bytes
  │
  │  oracleguard_adapter::charli3::normalize_parse_validate(bytes, utxo_ref, now_ms)
  ▼
(OracleFactEvalV1, OracleFactProvenanceV1)          — may also be an
  │                                                    OraclePriceZero /
  │                                                    OracleStale reason
  ▼
DisbursementIntentV1::new_v1(
    policy_ref, allocation_id, requester_id,
    oracle_fact = (eval half),
    oracle_provenance = (provenance half),
    requested_amount_lovelace, destination, asset,
)
```

Rules for this pipeline:

1. **Do not construct `OracleFactEvalV1` by hand from provenance
   data.** The only sanctioned producers are the normalization helpers
   in the adapter. If evidence replay needs a fixed fact, load it from
   canonical bytes, not by copying fields out of a provenance record.
2. **Do not construct `OracleFactProvenanceV1` from eval fields** —
   the two structs are distinct for a reason. A provenance record
   synthesized from eval data cannot be audited against an on-chain
   source.
3. **Do not place `oracle_provenance` values into the `oracle_fact`
   slot** of `DisbursementIntentV1::new_v1` or vice versa. The fields
   are the same integer types; the compiler cannot tell them apart.
   The calling site is the only place this invariant is visible.
4. **A single intent must embed exactly one matched pair** —
   `(oracle_fact, oracle_provenance)` that came from the same
   normalization call. Mixing a fact from one datum with provenance
   from another is an auditability failure, not merely a style issue.

A compiled end-to-end example that exercises this pipeline is at
`crates/oracleguard-adapter/examples/build_intent_from_charli3.rs`.
Run it with:

```
cargo run -p oracleguard-adapter --example build_intent_from_charli3
```

A doctest on `DisbursementIntentV1::new_v1` locks the eval/provenance
placement at the constructor's call site.

## Hard rules

- No free-form or stringly authoritative `source` — only the canonical
  identifiers in `oracleguard_schemas::oracle` are accepted.
- No alias behavior for `asset_pair` or `source` unless explicitly
  documented here.
- No float anywhere on the eval path. `#![deny(clippy::float_arithmetic)]`
  is set on every BLUE crate.
- No wall-clock reads inside the normalization helper. The adapter's
  freshness check takes `now` as an explicit argument so the helper
  stays pure.
- No provenance field may be promoted to eval without reopening Cluster
  3 and updating this document.

## Cross-references

- Canonical type definitions: `crates/oracleguard-schemas/src/oracle.rs`
- Intent identity rules: `docs/semantic-identity.md`
- Canonical byte encoding: `docs/canonical-encoding.md`
- Charli3 normalization shell: `crates/oracleguard-adapter/src/charli3.rs`
