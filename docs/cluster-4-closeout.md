# Cluster 4 Closeout — Release-Cap Evaluation Logic

This document is the explicit handoff from Cluster 4 to Cluster 5. It
exists to prove that the public release-cap evaluator is stable enough
for three-gate authorization work to proceed without reopening
evaluator-boundary ambiguity.

## Cluster 4 mechanical completion standard

| Exit criterion (Cluster 4 overview §6) | Satisfied | Evidence |
|---|---|---|
| `evaluate_disbursement` exists in `oracleguard-policy` as a pure function | yes | `crates/oracleguard-policy/src/evaluate.rs` — `pub fn evaluate_disbursement(intent, allocation_basis_lovelace) -> EvaluationResult`; crate denies `clippy::panic`, `clippy::unwrap_used`, `clippy::float_arithmetic`, `clippy::todo`, `clippy::unimplemented` at the root |
| Threshold and basis-point logic matches the documented release bands | yes | `crates/oracleguard-policy/src/math.rs` — `THRESHOLD_HIGH_MICROUSD=500_000`, `THRESHOLD_MID_MICROUSD=350_000`, `BAND_HIGH_BPS=10_000`, `BAND_MID_BPS=7_500`, `BAND_LOW_BPS=5_000`; compile-time `const _: () = assert!(...)` guards ordering; 14 boundary tests pin `349_999/350_000/499_999/500_000` + `0` + `u64::MAX` + the `450_000`/`258_000` demo anchors |
| Intermediate arithmetic is integer-only | yes | `compute_max_releasable_lovelace` uses `u128` intermediate; `#![deny(clippy::float_arithmetic)]` applied at the crate root; no `f32`/`f64` anywhere in the evaluator path |
| Overflow handling is deterministic and closed | yes | `compute_max_releasable_lovelace` returns `Option<u64>` with `None` on post-division downcast failure; `decide_grant` emits exclusively `ReleaseCapExceeded` on deny; `math::tests::oversized_bps_triggers_none_downcast`, `maximum_oversized_bps_is_still_total`, `full_cap_pipeline_is_total_for_valid_bands` lock the closure |
| Allow fixture replays identically across runs | yes | `fixtures/eval/allow_700_ada_result.postcard` (8 bytes); `crates/oracleguard-policy/tests/golden_evaluator.rs::allow_fixture_produces_golden_bytes` + `allow_fixture_replay_is_byte_identical` (8 runs, all byte-identical) |
| Deny fixture replays identically across runs | yes | `fixtures/eval/deny_900_ada_result.postcard` (2 bytes); `deny_fixture_produces_golden_bytes` + `deny_fixture_replay_is_byte_identical` |
| Evaluator outputs are typed and closed rather than stringly or ad hoc | yes | `EvaluationResult = Allow { authorized_amount_lovelace, release_cap_basis_points } \| Deny { reason: DisbursementReasonCode }`; destructuring-guard test `error::tests::evaluation_result_variant_set_is_frozen`; `decide_grant` narrows deny reasons to a single enum variant |
| No environment-derived or shell-derived behavior influences the evaluator | yes | Crate depends only on `oracleguard-schemas` and `serde` (dev-only `postcard`); no `std::env`, no `std::fs`, no `std::time`, no `std::process`, no network deps |
| The evaluator is legible enough that judges can inspect it as the public novel contribution | yes | `docs/evaluator-contract.md` states the full contract in one page; `docs/evaluator-fixtures.md` ties the goldens to Implementation Plan §16; the evaluator body in `evaluate.rs` is ~20 lines of explicit gate sequencing |

## Readiness questions (Cluster 4 → Cluster 5)

### Is `evaluate_disbursement` public and pure?

**Yes.** The function is the single public entry point of
`oracleguard-policy`. It has no I/O, no wall-clock reads, no mutable
global state, and no panics. Its inputs are a
`DisbursementIntentV1` reference and a `u64` allocation basis; its
output is a closed `EvaluationResult`. The crate root denies
`clippy::float_arithmetic`, `clippy::panic`, `clippy::unwrap_used`,
`clippy::expect_used`, `clippy::todo`, `clippy::unimplemented`, and
`clippy::print_stdout/stderr`.

### Are threshold and cap semantics fixed?

**Yes.** The three bands and two thresholds are `pub const` values in
`oracleguard_policy::math`, anchored by compile-time `const _: () =
assert!(...)` ordering guards. The band selector
`select_release_band_bps` is a `const fn` total over `u64`. The cap
computation `compute_max_releasable_lovelace` is documented with
floor-division semantics and tested at every boundary. The band table
in `docs/evaluator-contract.md §6` points at the constants by name so
prose cannot drift from code.

### Are arithmetic edge cases closed?

**Yes.** The product is computed in `u128`, so `u64 × u64`
multiplication is structurally total. The post-division downcast is
fallible; the evaluator maps a `None` downcast to
`Deny { ReleaseCapExceeded }`. For the three valid band constants and
any `u64` allocation, the downcast never fails — proven by
`math::tests::u64_max_allocation_at_every_valid_band_is_some` and
`u64_max_allocation_high_band_yields_u64_max`. The `None` branch
remains live only because `compute_max_releasable_lovelace` is a
public helper that Cluster 5 may call with arbitrary bps values.

### Are outputs typed and stable?

**Yes.** `EvaluationResult` is `#[derive(Serialize, Deserialize)]` and
its postcard encoding is pinned by two golden fixtures totalling 10
bytes. The `Deny` variant carries a `DisbursementReasonCode` whose
eight variants and `repr(u8)` discriminants are already pinned in
`oracleguard-schemas::reason::tests::repr_u8_discriminants_are_stable`.
Any movement in either enum breaks both the evaluator-layer goldens
(here) and the reason-code discriminant tests (in schemas).

### Can Cluster 5 consume the evaluator without reimplementing policy math?

**Yes.** The grant gate is a single call:

```rust
let result = evaluate_disbursement(&intent, allocation_basis_lovelace);
```

Cluster 5's anchor gate produces the preconditions (v1 intent, policy
anchored) and its registry gate produces `allocation_basis_lovelace`
and the `SubjectNotAuthorized` / `AssetMismatch` / `AllocationNotFound`
short-circuits. Freshness (`OracleStale`) continues to be emitted by
`oracleguard_adapter::charli3::check_freshness` before the evaluator
runs. Cluster 5 wires these gates; it does not re-derive any of them.

## Handoff targets for Cluster 5

Cluster 5 — **Three-Gate Authorization Closure** — should land its work
at the following explicit locations:

- Anchor gate — policy registration lookup + the non-v1 `intent_version`
  branch (emits `PolicyNotFound`) → a new module in
  `oracleguard-policy` or a dedicated authorization crate; the
  evaluator's `debug_assert!` on `intent_version == INTENT_VERSION_V1`
  becomes a release-build anchor-gate check.
- Registry gate — allocation/subject/asset validation (emits
  `AllocationNotFound`, `SubjectNotAuthorized`, `AssetMismatch`) and
  production of the `allocation_basis_lovelace` argument.
- Freshness wiring — a call to
  `oracleguard_adapter::charli3::check_freshness` before the evaluator
  runs. The adapter function is already pure with an explicit `now`
  argument; Cluster 5 supplies the clock at the orchestration seam.
- Grant gate — a single call to `evaluate_disbursement`. No
  reimplementation, no re-ordering of the AmountZero / OraclePriceZero
  / band / cap / decide_grant sequence.

The release rule is pinned in the Implementation Plan §9 and the
`oracleguard_policy::math` constants. Cluster 5 composes the gates; it
does not re-derive the release-cap arithmetic.

## What Cluster 5 must NOT do based on Cluster 4 state

- It must not redefine `EvaluationResult` or any helper in
  `oracleguard-policy::math`. The public shape is pinned by the
  golden fixtures and by `docs/evaluator-contract.md`.
- It must not introduce a second evaluator function. All release-cap
  decisions go through `evaluate_disbursement`.
- It must not call `evaluate_disbursement` with an unvalidated
  `allocation_basis_lovelace` — the registry gate is responsible for
  sourcing it.
- It must not reshuffle the grant-gate check order. The sequence
  `AmountZero → OraclePriceZero → band → cap → decide_grant` is
  locked by tests and by the contract doc.
- It must not perform floating-point arithmetic, wall-clock reads, or
  I/O inside any authoritative gate. The workspace lints already
  forbid floats in BLUE crates.
- It must not regenerate `fixtures/eval/*.postcard` without documenting
  the canonical-byte reason in a closeout update.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 4. Cluster 5 begins from a clean CI, a public pure
evaluator with two golden fixtures, a pinned set of threshold/band
constants, and a workspace in which every remaining ambiguity about
how grant decisions are produced has been answered in writing and in
code.
