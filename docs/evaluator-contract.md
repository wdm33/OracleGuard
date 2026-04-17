# Evaluator Contract

The release-cap evaluator in `oracleguard-policy` is the public,
deterministic decision core of OracleGuard. It is the single function
through which a canonical `DisbursementIntentV1` becomes an allow or a
deny with a typed reason. Every downstream component — Cluster 5's
three-gate orchestration, Cluster 7's authorized effect emission,
Cluster 8's offline verifier — depends on this contract without
re-deriving it.

This document is the canonical statement of the contract. Code and
tests implement it; docs/cluster-*-closeout.md audit it; nothing else
is entitled to redefine it.

## 1. Signature

```rust
pub fn evaluate_disbursement(
    intent: &DisbursementIntentV1,
    allocation_basis_lovelace: u64,
) -> EvaluationResult
```

- `intent` — canonical v1 intent from `oracleguard-schemas`. The
  caller is responsible for having validated the schema version; see
  §4 below.
- `allocation_basis_lovelace` — the approved allocation size, in
  lovelace, that the requester is drawing from. In Cluster 4 this is
  passed directly by tests. In Cluster 5 the registry gate resolves it
  from `intent.allocation_id` and calls this function.

## 2. Result

```rust
pub enum EvaluationResult {
    Allow {
        authorized_amount_lovelace: u64,
        release_cap_basis_points: u64,
    },
    Deny { reason: DisbursementReasonCode },
}
```

- An `Allow` variant MUST carry
  `authorized_amount_lovelace == intent.requested_amount_lovelace`. The
  evaluator never silently reduces the request.
- A `Deny` variant MUST carry exactly one of the eight canonical reason
  codes in `oracleguard_schemas::reason::DisbursementReasonCode`.
- The variant set is frozen. Adding or reordering variants is a
  canonical-byte change that invalidates Slice 06 golden fixtures and
  requires a version-boundary crossing.

## 3. Purity and determinism

The evaluator is a pure function of its two arguments:

- no I/O, no file reads, no network calls, no process spawning,
- no floating-point arithmetic (`#![deny(clippy::float_arithmetic)]`),
- no wall-clock reads,
- no mutable global state,
- no nondeterministic iteration,
- no panics (`#![deny(clippy::panic)]`).

`evaluate_disbursement(a, b) == evaluate_disbursement(a, b)` holds for
every `(a, b)` pair, on every run, on every platform that meets the
workspace's MSRV.

## 4. What the evaluator assumes

The evaluator assumes that the caller has already:

1. Validated that `intent.intent_version == INTENT_VERSION_V1`. In
   debug builds Slice 05 will `debug_assert!` this; in release builds
   a non-v1 intent is a caller contract violation and is a Cluster 5
   anchor-gate concern. The eight reason codes do not include a
   version-mismatch variant by design.
2. Checked oracle freshness via
   `oracleguard_adapter::charli3::check_freshness` if the fact came
   from a live fetch. `OracleStale` is emitted outside the evaluator.
3. Resolved `allocation_basis_lovelace` from a registry lookup keyed
   by `intent.allocation_id`. Cluster 4 exposes the parameter
   directly; Cluster 5 wraps the call.

The evaluator never consults `intent.oracle_provenance` for any
authorization purpose. Provenance fields are audit-only and are
structurally excluded from the intent-id (see
`oracleguard_schemas::encoding`).

## 5. Grant-gate semantics (Cluster 4 scope)

The evaluator runs the grant gate in this exact order. Anchor and
registry gates are Cluster 5.

1. If `intent.requested_amount_lovelace == 0` → `Deny { AmountZero }`.
2. If `validate_oracle_fact_eval(&intent.oracle_fact)` rejects the
   fact → `Deny { OraclePriceZero }`.
3. Select a release band from `intent.oracle_fact.price_microusd`
   (Slice 02).
4. Compute `max_releasable` from `allocation_basis_lovelace` and the
   selected band (Slice 03). If the arithmetic pipeline cannot
   produce a `u64`, short-circuit to `Deny { ReleaseCapExceeded }`
   (Slice 04).
5. Compare `intent.requested_amount_lovelace` to `max_releasable`:
   - `requested <= max_releasable` → `Allow { requested, band_bps }`.
   - `requested > max_releasable` → `Deny { ReleaseCapExceeded }`.

Equality at the cap is an allow. This is the sole sanctioned rounding
rule and it is documented here so Cluster 5's grant-gate wiring cannot
silently reinterpret it.

## 6. Threshold bands

| Condition                                  | Constant          | Band constant    | Basis points |
|--------------------------------------------|-------------------|------------------|--------------|
| `price_microusd >= THRESHOLD_HIGH_MICROUSD`| `500_000`         | `BAND_HIGH_BPS`  | `10_000`     |
| `price_microusd >= THRESHOLD_MID_MICROUSD` | `350_000`         | `BAND_MID_BPS`   | `7_500`      |
| `price_microusd <  THRESHOLD_MID_MICROUSD` | —                 | `BAND_LOW_BPS`   | `5_000`      |

The constants live in `oracleguard_policy::math`. Band selection is
implemented by `select_release_band_bps(price_microusd)`, which is
total over `u64` and pure.

Comparisons are inclusive on the lower bound. `validate_oracle_fact_eval`
rejects `price_microusd == 0` before the evaluator runs, so the
zero-price point of the low band is unreachable in practice, but the
band function remains total so replay is well-defined for every `u64`.

## 7. Arithmetic

Release-cap math is computed by
`oracleguard_policy::math::compute_max_releasable_lovelace`:

```text
product  = allocation_lovelace (u128) * release_cap_bps (u128)
cap_u128 = product / BASIS_POINT_SCALE
cap      = u64::try_from(cap_u128)   // Some | None
```

The multiplication is total because `u64 * u64` always fits in `u128`.
The post-division downcast back to `u64` is fallible; a `None` downcast
deterministically maps to `Deny { ReleaseCapExceeded }`. Floor division
(integer truncation) is the only sanctioned rounding rule — fractional
lovelace never round up.

For valid bands `{BAND_LOW_BPS, BAND_MID_BPS, BAND_HIGH_BPS}` (i.e.
`release_cap_bps ≤ BASIS_POINT_SCALE`) and any
`allocation_lovelace ≤ u64::MAX`, the downcast never fails — this is
proven by `u64_max_allocation_at_every_valid_band_is_some` and
`u64_max_allocation_high_band_yields_u64_max`. The `None` branch
exists only to make the function total on inputs outside the valid
band set (Slice 04 exercises those cases).

## 8. Evolution

Changes to this contract require:

- a version boundary on the result type (touching Slice 06 fixtures),
- a matching update to the reason-code enum in `oracleguard-schemas`
  (touching every component that pattern-matches it),
- a closeout update to `docs/cluster-4-closeout.md`,
- regeneration of the golden fixtures under `fixtures/eval/` via the
  `#[ignore]` regenerate test.

No change to this contract may land without all four.
