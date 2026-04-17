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

These invariants are enforced by tests; see Slice 05.

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
