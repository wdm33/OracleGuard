# Cluster 6 Closeout — Authority Routing and Submission Boundary

This document is the explicit handoff from Cluster 6 to Cluster 7. It
exists to prove that the public routing and submission boundary is
stable enough for authorized-effect fulfillment and Cardano settlement
to proceed without reopening authority or transport ambiguity.

## Cluster 6 mechanical completion standard

| Exit criterion (Cluster 6 overview §6) | Satisfied | Evidence |
|---|---|---|
| One well-known disbursement OCU identifier is fixed for the MVP | yes | `oracleguard_schemas::routing::DISBURSEMENT_OCU_ID` (32-byte derived constant); `fixtures/routing/disbursement_ocu_id.hex`; `docs/authority-routing.md`; `routing::tests::ocu_id_matches_documented_derivation` pins the baked literal to the derivation recipe, `ocu_id_is_not_zero` and `ocu_id_hex_fixture_matches_constant` pin the non-zero + fixture invariants |
| Routing does not depend on destination, oracle source, payout request, or any other dynamic field not intended to define authority | yes | `oracleguard_schemas::routing::route` always returns `DISBURSEMENT_OCU_ID` regardless of intent fields; 11 per-field invariance tests (`route_is_invariant_under_{destination,oracle_source,oracle_price,oracle_asset_pair,provenance,requested_amount,asset,requester_id,allocation_id,policy_ref}_change`) + a 64-iteration combined sweep (`route_sweep_over_many_mutations_yields_single_target`); integration-level coverage in `crates/oracleguard-adapter/tests/routing_boundary.rs::routing_is_invariant_over_64_combined_mutations` |
| OracleGuard constructs only the domain payload | yes | `oracleguard_schemas::encoding::emit_payload` is a named alias of `encode_intent` producing canonical postcard bytes only; `encoding::tests::emit_payload_{equals_encode_intent, matches_golden_fixture, round_trips_via_decode_intent, is_stable_across_calls}` pin payload identity; adapter re-export `oracleguard_adapter::ziranity_submit::payload_bytes` has matching tests; `write_payload_to_file` writes canonical bytes only, verified by `write_payload_to_file_writes_canonical_bytes_only` |
| OracleGuard uses a deterministic submission shell boundary | yes | `SubmitConfig` + pure `build_cli_args` construct the `ziranity intent submit` argument vector deterministically; golden fixture `fixtures/routing/sample_cli_args.golden.txt` pins the full vector; 10+ unit tests pin flag order, object_id hex encoding, node `host:port` formatting, and per-field isolation (`*_change_isolates_to_*_field`); integration tests in `routing_boundary.rs` add `build_cli_args_is_stable_for_identical_config` and routing-agnosticism assertions |
| Ziranity envelope, signing, and transport responsibilities remain outside OracleGuard | yes | `ziranity_submit` module docstring explicitly lists these as out-of-scope; no module in the adapter constructs `ClientIntentV1`, performs Ed25519 signing, computes BLAKE3 `intent_id`, or implements MCP framing; `submit_intent` spawns the CLI via `std::process::Command` and surfaces raw stdout bytes as `RawCliStdout` without interpretation |
| Returned consensus output is read without semantic reinterpretation | yes | `oracleguard_adapter::intake` parses CLI receipt bytes into `IntentReceiptV1` whose `output` is a decoded `AuthorizationResult` — the authoritative type from Cluster 5; the intake module contains a structural audit test (`intake_does_not_recompute_authorization`) that inspects the production half of its own source and fails if it ever calls `authorize_disbursement`, `evaluate_disbursement`, or any per-gate function; `AuthorizationResult::decode` is strict (trailing-byte rejection) |
| The public docs explain the routing and submission boundary clearly enough for judges and reviewers | yes | `docs/authority-routing.md` covers single-authority rule, OCU derivation recipe, and change policy; `docs/cluster-6-closeout.md` (this document) covers mechanical completion, readiness questions, and Cluster 7 handoff; `docs/architecture.md` landing zones updated to Cluster 7 targets |
| No multi-authority ambiguity remains for the disbursement use case | yes | Structural: `route` has one return path (the fixed constant); there is no alternate routing surface in the public crates; `SubmitConfig::new_mvp` binds `object_id` to `DISBURSEMENT_OCU_ID`; `build_cli_args` emits `--object_id` as the hex of that field with no derivation; integration test `build_cli_args_object_id_defaults_to_fixed_ocu` pins the default binding |

## Readiness questions (Cluster 6 → Cluster 7)

### Is the authority OCU fixed?

**Yes.** `DISBURSEMENT_OCU_ID` is a 32-byte derived constant in
`oracleguard_schemas::routing`, produced by a BLAKE3 keyed hash of the
`ORACLEGUARD_DISBURSEMENT_OCU_V1` derive-key domain over the fixed
`OCU/MVP/v1` seed. The baked literal is pinned to the derivation recipe
by a non-ignored test. `fixtures/routing/disbursement_ocu_id.hex`
surfaces the operator-facing hex form. The constant is the single
authority target referenced by the routing rule, the CLI wrapper, and
the public authority-routing documentation.

### Is routing single-authority and deterministic?

**Yes.** `route(&DisbursementIntentV1) -> [u8; 32]` always returns
`DISBURSEMENT_OCU_ID`. The function takes the intent by reference so
call-sites read correctly, but the body deliberately does not inspect
its fields. Tests prove routing is invariant under mutation of every
non-authority field (destination, oracle source, price, asset pair,
provenance, requested amount, asset, requester id, allocation id,
policy ref) — both per-field and in a 64-iteration combined sweep.
Integration-level assertions in `tests/routing_boundary.rs` confirm
the property holds at the adapter wiring level as well.

### Does OracleGuard emit only canonical payload bytes?

**Yes.** `emit_payload` in `oracleguard-schemas::encoding` is a named
alias of `encode_intent` — it produces the canonical postcard bytes of
`DisbursementIntentV1` and nothing more. The named alias is deliberate:
the submission role is textually explicit, but byte identity is
forwarded to the single canonical encoder. Golden fixture coverage
(`encoded_bytes_match_golden`, `emit_payload_matches_golden_fixture`)
pins the bytes against `fixtures/intent_v1_golden.postcard`. The
adapter re-export `payload_bytes` has matching tests.
`write_payload_to_file` writes exactly those bytes to disk for the
`--payload` CLI handoff.

### Does the submission wrapper remain bounded to shell invocation?

**Yes.** `build_cli_args` is a pure function with no I/O.
`submit_intent` wraps a `std::process::Command` spawn of the
`ziranity` binary and returns raw stdout bytes on success or surfaces
the exit code + stderr on non-zero exit. The wrapper does not build
`ClientIntentV1` envelopes, does not sign anything, does not compute
BLAKE3 `intent_id`, does not implement the MCP framing protocol. A
golden CLI-args fixture
(`fixtures/routing/sample_cli_args.golden.txt`) pins the full argument
vector for a representative config. Per-field isolation tests prove
changing one config field changes exactly the corresponding argument
value.

### Can downstream fulfillment consume consensus output as already-authoritative?

**Yes.** The adapter's `intake` module parses CLI receipts into a
typed `IntentReceiptV1 { intent_id, status, committed_height, output:
AuthorizationResult }`. The `output` field is the authoritative
authorization decision produced upstream by the three-gate closure
(Cluster 5). OracleGuard does not re-run any gate, does not recompute
`release_cap_basis_points`, does not re-validate destination or
subject, and does not translate denial reasons. A structural audit
test inspects the production half of the intake source and fails if a
future change introduces any of those calls.
`AuthorizationResult::decode` is strict: trailing bytes reject,
truncated input rejects, one byte sequence cannot decode to two
meanings.

## Handoff targets for Cluster 7

Cluster 7 — **Authorized Effect and Cardano Fulfillment** — should
land its work at the following explicit locations:

- Settlement construction — consume `IntentReceiptV1.output` matched
  against `AuthorizationResult::Authorized { effect }`. The
  `AuthorizedEffectV1` fields (`destination`, `asset`,
  `authorized_amount_lovelace`) are the canonical inputs to the
  Cardano transaction builder. Do NOT re-run authorization. Do NOT
  re-derive `release_cap_basis_points`.
- Denial path — on `AuthorizationResult::Denied { reason, gate }`,
  settlement does not execute. Evidence persistence captures
  `(reason, gate)` verbatim; there is no "retry with adjusted amount"
  or similar local fixup — that would violate the deny/no-effect
  closure pinned in Cluster 5.
- Bundle persistence — the evidence bundle already defined by the
  schemas crate is the target. Cluster 7's responsibility is to
  record the `(intent_bytes, receipt_bytes, cardano_tx_bytes)` trail
  for the allow path, or `(intent_bytes, receipt_bytes)` for the
  deny path. Canonical bytes come from Cluster 6 surfaces, not from
  fresh re-encoding.

## What Cluster 7 must NOT do based on Cluster 6 state

- It must not introduce an alternative routing target. The single
  authority OCU is fixed; any new target is a canonical-byte
  version-boundary event.
- It must not re-parse or re-derive the consensus receipt. The
  `parse_cli_receipt` / `parse_consensus_output` surface is the intake
  boundary; settlement consumes its typed output.
- It must not sign or envelope `DisbursementIntentV1` bytes. The
  intent was already submitted; settlement operates on the already-
  authorized effect downstream of consensus.
- It must not absorb MCP transport semantics. The Ziranity CLI owns
  transport; settlement talks to Cardano, not to validators.
- It must not reshape `AuthorizedEffectV1`. The struct is canonical
  bytes; any change is a version-boundary event.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 6. Cluster 7 begins from a clean CI
(`cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`scripts/check_deps.sh`, `scripts/check_routing_determinism.sh` all
green), a fixed disbursement authority OCU constant, a single-
authority deterministic routing rule, a named canonical payload
emission boundary, a bounded shell CLI wrapper, a typed consensus
output intake, and a workspace in which every remaining ambiguity
about what counts as routing, submission, or intake has been answered
in writing, in code, and in tests.
