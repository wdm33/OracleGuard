# Cluster 7 Closeout — Authorized Effect and Cardano Fulfillment

This document is the explicit handoff from Cluster 7 to Cluster 8. It
exists to prove that the public fulfillment boundary is stable enough
for evidence bundling and offline verification to proceed without
reopening effect, mapping, or execution-outcome ambiguity.

## Cluster 7 mechanical completion standard

| Exit criterion (Cluster 7 overview §6) | Satisfied | Evidence |
|---|---|---|
| `AuthorizedEffectV1` is defined in public schemas | yes | `oracleguard_schemas::effect::AuthorizedEffectV1` is a fixed-width `Copy` struct with per-field docstrings. Unit tests `authorized_effect_exposes_exact_fixed_width_fields`, `authorized_effect_postcard_round_trips`, `authorized_effect_equality_is_field_wise`, `authorized_effect_is_copy_and_equates_by_value`, and `authorized_effect_golden_fixture_decodes_to_canonical_demo_effect` pin the struct's shape, canonical bytes, and identity against `fixtures/authorize/authorized_effect_golden.postcard` at the authority crate. The Cluster 5 demo-authorization golden tests in `crates/oracleguard-policy/tests/closure_proofs.rs` pin the same bytes from the policy side. |
| The fulfillment path accepts only typed authorized effects | yes | `oracleguard_adapter::settlement::fulfill` consumes `IntentReceiptV1`, matches on `AuthorizationResult::Authorized { effect }`, and passes the typed `AuthorizedEffectV1` into `settlement_request_from_effect`. No stringly-typed or ad-hoc execution-command surface exists anywhere in the adapter or verifier crates. |
| Amount, destination, and asset are copied through exactly | yes | `settlement_request_from_effect` is a pure field copy — no rounding, unit conversion, or bech32 re-encoding. Unit tests `mapping_copies_destination_exactly`, `mapping_copies_asset_exactly`, `mapping_copies_amount_exactly` (including a u64 sweep with `u64::MAX`), plus `mapping_is_independent_of_release_cap_basis_points` and `mapping_is_independent_of_policy_ref_allocation_requester` mechanically prove the pass-through. Integration suite `tests/fulfillment_boundary.rs::copy_through_is_exact_across_amount_sweep`, `copy_through_is_exact_across_destination_sweep`, and `copy_through_is_independent_of_policy_allocation_requester_and_cap` pin the same at integration scope. |
| Allow-path fulfillment produces a transaction hash or equivalent stable settlement reference | yes | `FulfillmentOutcome::Settled { tx_hash: CardanoTxHashV1 }` is the typed capture surface. `CardanoTxHashV1` is a 32-byte fixed-width `Copy` struct with strict lowercase-hex round-trip (`from_hex` / `to_hex`), pinned by `tx_hash_round_trips_via_hex_for_boundary_values`, `tx_hash_round_trips_for_sample_fixture`, and rejection tests covering wrong length, non-hex, uppercase, and non-UTF8. Fixture `fixtures/cardano/tx_hash_sample.hex` supplies the cross-test reference hash. |
| Deny-path processing produces no transaction | yes | `fulfill` matches `AuthorizationResult::Denied` before any backend reference is dereferenced; integration test `deny_path_never_calls_backend_across_all_reason_codes` iterates all 8 `DisbursementReasonCode × gate` pairs and asserts `backend.submit_count() == 0` for each. The unit-level equivalent `every_deny_reason_short_circuits_without_backend_call` asserts the same at `settlement.rs` scope. |
| Asset mismatch and invalid destination handling are deterministic | yes | Asset mismatch at upstream is surfaced verbatim as `FulfillmentOutcome::DeniedUpstream { reason: AssetMismatch, gate: Registry }`; fulfillment-side non-ADA is a distinct `FulfillmentOutcome::RejectedAtFulfillment { kind: NonAdaAsset }`. Destination approval is enforced by the registry gate (Cluster 5) and surfaces as `SubjectNotAuthorized` per the v1 compatibility pin; `fulfill` copies `reason`/`gate` verbatim. Determinism pinned by `fulfill_is_deterministic_for_deny`, `fulfill_is_deterministic_for_reject`, and `fulfill_is_deterministic_for_settled`. |
| No fulfillment path can exceed the authorized amount | yes | Structural: `SettlementRequest::amount_lovelace` is assigned from `effect.authorized_amount_lovelace` with no arithmetic; no code path in `settlement.rs` or `cardano.rs` touches the amount. Property tests `settled_request_amount_equals_authorized_amount_exactly` (in-module) and `allow_path_request_amount_equals_authorized_amount_for_sweep` (integration) sweep `[1, 700_000_000, 1_000_000_000, u64::MAX]` and assert byte-identity. |
| Public docs explain the fulfillment boundary clearly enough for judges and reviewers | yes | `docs/fulfillment-boundary.md` is the single reviewer-facing explanation, with six sections covering the authorized-effect input surface, exact copy-through rule, ADA-only MVP scope, deny/no-tx closure, tx-hash capture contract, and the full public-surface summary. `docs/cluster-7-closeout.md` (this document) covers mechanical completion, readiness questions, and Cluster 8 handoff. `docs/architecture.md` landing zones updated to Cluster 8 targets. |
| No downstream wallet or shell layer is required to finish semantic meaning | yes | The `SettlementBackend` trait takes a fully-formed `SettlementRequest` whose fields are byte-identical to the authorized effect's. `CardanoCliSettlementBackend::submit` calls `cardano-cli transaction txid --tx-file <path>` and parses the stdout into a `CardanoTxHashV1`; it does not mutate amount, destination, or asset, and it does not construct semantic meaning. The trait is defined next to the shell backend so alternative backends inherit the same narrow surface. |

## Readiness questions (Cluster 7 → Cluster 8)

### Is `AuthorizedEffectV1` fixed and public?

**Yes.** The struct is a fixed-width, `Copy`, `PartialEq`, `Hash`
public type in `oracleguard_schemas::effect`. Its fields are locked
for v1; any change is a canonical-byte version-boundary event. The
schema-scope golden fixture test pins the demo authorization's
canonical bytes to `fixtures/authorize/authorized_effect_golden.postcard`
independently of the policy-crate tests that originally produced the
fixture.

### Does fulfillment copy amount, destination, and asset through exactly?

**Yes.** `settlement_request_from_effect` is a pure field copy. No
rounding, unit conversion, bech32 re-encoding, or normalization
happens between `AuthorizedEffectV1` and `SettlementRequest`. The
in-module tests cover amount (including `u64::MAX`), destination
(full 57-byte sweep), and asset (policy_id, asset_name,
asset_name_len). The integration suite covers the same properties
against the receipt → fulfill chain.

### Is deny/no-tx closure enforced?

**Yes.** `fulfill` matches `AuthorizationResult::Denied` before any
backend reference is dereferenced. The integration test iterates
every canonical `DisbursementReasonCode × gate` pair and asserts
zero backend calls. A structural audit test in `settlement.rs`
(`settlement_production_source_does_not_call_authorize_or_evaluate`)
inspects its own production half and fails if any evaluator or
per-gate function is ever called — mechanically pinning that
OracleGuard does not recompute authorization on the fulfillment
path.

### Is allow-path tx-reference capture stable?

**Yes.** `CardanoTxHashV1` is the typed capture surface.
`FulfillmentOutcome::Settled { tx_hash }` is the only outcome that
corresponds to a submitted Cardano transaction. The
`SettlementBackend` trait is the single-point capture boundary; the
RED `CardanoCliSettlementBackend` parses `cardano-cli transaction
txid` stdout into `CardanoTxHashV1` with strict lowercase-hex
validation. No log-scraping or post-hoc reconstruction remains.

### Can Cluster 8 inspect execution results without reconstructing fulfillment semantics?

**Yes.** `FulfillmentOutcome` is the typed execution-outcome surface
with exactly three variants: `Settled { tx_hash }`,
`DeniedUpstream { reason, gate }`, and
`RejectedAtFulfillment { kind }`. Every outcome of a `fulfill` call
maps to exactly one variant; evidence can branch on these three
cases without running any authorization logic, invoking any
backend, or interpreting any log. The `FulfillmentRejection`
enum's discriminants (`NonAdaAsset`, `ReceiptNotCommitted`)
enumerate the MVP fulfillment-side rejection kinds.

## Handoff targets for Cluster 8

Cluster 8 — **Evidence Bundle and Offline Verification** — should
land its work at the following explicit locations:

- Evidence bundle assembly — consume `FulfillmentOutcome` as the
  typed execution-outcome surface. Allow path records `tx_hash`;
  upstream deny records `(reason, gate)`; fulfillment-side
  rejection records `kind`. Canonical bytes come from Cluster 6
  surfaces (`emit_payload`, `render_cli_receipt`) and Cluster 7
  surfaces (`CardanoTxHashV1`, `FulfillmentOutcome`), never from
  fresh re-encoding or re-derivation.
- Offline verifier — replay from canonical bytes using public
  semantic types (`AuthorizedEffectV1`, `AuthorizationResult`,
  `SettlementRequest`, `CardanoTxHashV1`). The verifier must
  recompute the authorization decision from canonical inputs and
  match against the recorded `AuthorizationResult`; it must not
  redefine effect, request, outcome, or tx-hash meaning.
- No new execution-command surface. `AuthorizedEffectV1` and
  `SettlementRequest` are the only typed inputs that reach
  settlement; Cluster 8 is free to inspect them but must not
  introduce an alternative type.

## What Cluster 8 must NOT do based on Cluster 7 state

- It must not re-run authorization or re-derive
  `release_cap_basis_points`. Cluster 5 is the authoritative
  decision surface; Cluster 7 consumes it verbatim.
- It must not reinterpret `destination`, `asset`, or
  `authorized_amount_lovelace`. The fulfillment boundary's exact
  copy-through rule already ties the on-chain output to the
  authorized effect.
- It must not translate denial reasons or gate attributions. The
  `(reason, gate)` pair is canonical; evidence surfaces it
  verbatim.
- It must not add a second tx-hash type. `CardanoTxHashV1` is the
  fixed-width canonical Cardano transaction id surface for v1.
- It must not introduce settlement logic. `fulfill` is the single
  fulfillment entrypoint; alternative surfaces are a
  canonical-byte version-boundary event.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 7. Cluster 8 begins from a clean CI
(`cargo test --workspace`, `cargo clippy --workspace --all-targets
-- -D warnings`, `cargo fmt --all -- --check`,
`scripts/check_deps.sh`, `scripts/check_routing_determinism.sh`,
`scripts/check_fulfillment_determinism.sh` all green), a pinned
`AuthorizedEffectV1`, an exact effect-to-settlement mapping, an
ADA-only MVP guard, a deny/no-tx closure, a typed execution-outcome
surface, a canonical tx-hash type with a stable backend capture
boundary, and a workspace in which every remaining ambiguity about
what counts as a fulfillment input, an execution branch, or a
capture reference has been answered in writing, in code, and in
tests.
