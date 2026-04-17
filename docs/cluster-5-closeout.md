# Cluster 5 Closeout — Three-Gate Authorization Closure

This document is the explicit handoff from Cluster 5 to Cluster 6. It
exists to prove that the public authorization surface is stable enough
for routing, submission, and settlement work to proceed without
reopening authorization-boundary ambiguity.

## Cluster 5 mechanical completion standard

| Exit criterion (Cluster 5 overview §6) | Satisfied | Evidence |
|---|---|---|
| Gate order is fixed as anchor gate → registry gate → grant gate | yes | `crates/oracleguard-policy/src/authorize.rs::authorize_disbursement` sequences `run_anchor_gate → run_registry_gate → run_grant_gate` with fail-fast short-circuit; `docs/authorization-gates.md §Gate order` is the authoritative prose; `authorize::tests::authorize_disbursement_{anchor_failure_precedes_registry_failure, registry_failure_precedes_grant_failure, anchor_failure_precedes_grant_failure}` pin the multi-failure precedence |
| Denial reasons are represented as a closed typed set | yes | `oracleguard_schemas::reason::DisbursementReasonCode` — eight `repr(u8)` variants, `reason::tests::{variant_list_is_frozen_at_eight_codes, repr_u8_discriminants_are_stable, reason_code_postcard_round_trips}`; reason-to-gate partition pinned by `DisbursementReasonCode::gate()` and `reason::tests::{gate_partition_matches_documented_table, every_reason_code_maps_to_exactly_one_gate}` |
| Anchor gate validates the policy reference deterministically | yes | `authorize::run_anchor_gate` composes `validate_intent_version` + `PolicyAnchorView::is_policy_registered`; pure (no I/O, no clock, no alloc); 9 unit tests in `authorize::tests::anchor_gate_*` cover valid/invalid paths, version-vs-registration precedence, determinism, and the reason-to-gate invariant |
| Registry gate validates allocation, subject, and destination authorization deterministically | yes | `authorize::run_registry_gate` composes allocation lookup → subject → asset → destination against `AllocationRegistryView`; 15 unit tests in `authorize::tests::registry_gate_*` cover every deny path, within-gate precedence (allocation ≻ subject ≻ asset ≻ destination), byte-identical destination comparison (including `length`), and the v1 destination-to-`SubjectNotAuthorized` mapping |
| Grant gate consumes only canonical evaluator inputs and outputs | yes | `authorize::run_grant_gate` calls `check_freshness` (schemas) then `evaluate_disbursement` and packages the `Allow` into `AuthorizedEffectV1` without duplicating band math; `authorize::tests::grant_gate_does_not_duplicate_cap_math` exercises two distinct bands and asserts the grant gate returns the evaluator-selected `release_cap_basis_points`; the `AmountZero → OraclePriceZero → ReleaseCapExceeded` suffix inherits from the Cluster 4 evaluator with no reshuffling |
| Denied outcomes emit no authorized effect | yes | Structural: `AuthorizationResult::Denied { reason, gate }` has no effect field (pattern-match enforced by `closure_proofs::{denied_variant_has_no_effect_field, authorized_variant_has_only_effect_field}`); behavioral: `closure_proofs::every_reachable_deny_reason_emits_no_effect` sweeps all eight canonical reasons through `authorize_disbursement` and asserts `Denied` every time |
| Destination validation is enforced before effect emission | yes | `run_registry_gate` step 4 runs against `approved_destinations` and denies with `SubjectNotAuthorized` before the grant gate is reached; `run_grant_gate` receives only the allocation basis and cannot rewrite the destination; `closure_proofs::allow_path_produces_effect_mirroring_intent` proves the authorized effect's `destination` equals the intent's `destination` verbatim |
| Failure precedence is deterministic and documented | yes | `docs/authorization-gates.md §Within-gate check order` is the authoritative precedence document; `authorize::tests::documented_within_{anchor,registry,grant}_gate_precedence` mirror the doc table; multi-failure tests at the orchestrator layer (see gate-order row) cover the cross-gate order; every denial carries `gate == reason.gate()` (invariant pinned by `authorize::tests::authorize_disbursement_denied_gate_equals_reason_gate` and `every_reason_denies_at_its_own_gate`) |
| No downstream layer is required to reinterpret or finish authorization meaning | yes | The only public surface for authorization is `authorize_disbursement(intent, anchor, registry, now_unix_ms) -> AuthorizationResult`; Cluster 6 consumers read `AuthorizationResult::Authorized { effect: AuthorizedEffectV1 }` as the single source of truth for settlement and `AuthorizationResult::Denied { reason, gate }` as the typed denial; there is no partial-state, nullable-effect, or stringly-typed path into authorization meaning |

## Readiness questions (Cluster 5 → Cluster 6)

### Is gate order fixed?

**Yes.** The fixed order anchor → registry → grant is encoded
mechanically in `authorize::authorize_disbursement` (fail-fast, match +
early return per gate) and documented in
`docs/authorization-gates.md §Gate order`. Within-gate check order is
documented per gate and mirrored by unit tests. Multi-failure
precedence is exercised by orchestrator integration tests
(`authorize_disbursement_anchor_failure_precedes_registry_failure`
and friends).

### Are denial reasons typed and closed?

**Yes.** `DisbursementReasonCode` has eight frozen variants with pinned
`repr(u8)` discriminants and postcard round-trip golden behavior. The
`gate()` const method partitions the reasons across the three
`AuthorizationGate` variants; the partition is pinned by
`reason::tests::gate_partition_matches_documented_table`. No ad-hoc
string denial surface exists anywhere in the public crates.

### Is destination authorization already completed before effect emission?

**Yes.** The registry gate validates destination membership against
`approved_destinations` as step 4 of its within-gate check order and
denies with `SubjectNotAuthorized` on mismatch. Comparison is
byte-identical, including `CardanoAddressV1::length`
(`registry_gate_destination_address_length_matters`). The grant gate
cannot override, fix up, or bypass this check — it receives only
`basis_lovelace` from the registry gate.

The v1 mapping of destination mismatch onto `SubjectNotAuthorized` is
deliberate: the closed eight-variant reason surface has no dedicated
destination code for v1. A dedicated code is a future canonical-byte
version-boundary change and is out of scope for Cluster 5. The mapping
is pinned in:

- `DisbursementReasonCode::SubjectNotAuthorized` docstring
  (v1 compatibility note)
- `docs/authorization-gates.md §Registry gate`
- `authorize::tests::registry_gate_destination_mismatch_emits_subject_not_authorized_v1_mapping`
- `reason::tests::subject_not_authorized_includes_destination_mismatch_for_v1`

### Is the grant gate bound to the public evaluator?

**Yes.** `run_grant_gate` delegates the release-cap decision to
`evaluate_disbursement` and maps its `Allow` / `Deny` outcomes onto
`AuthorizedEffectV1` / `DisbursementReasonCode`. No band math,
threshold comparison, or basis-point arithmetic is duplicated in the
authorization layer. The `release_cap_basis_points` field of every
authorized effect is the exact value the evaluator selected, proven
for two distinct bands by
`grant_gate_does_not_duplicate_cap_math`.

Freshness (the `OracleStale` precondition) moved to
`oracleguard_schemas::oracle::{check_freshness, OracleStale}` in this
cluster so `oracleguard-policy` can consume it without depending on
`oracleguard-adapter`. The adapter re-exports both symbols to keep its
public API stable.

### Can routing/submission consume authorization as already decided rather than still-in-progress logic?

**Yes.** The single authorization entry point is
`oracleguard_policy::authorize::authorize_disbursement`. It returns a
closed `AuthorizationResult`:

- `Authorized { effect: AuthorizedEffectV1 }` — settlement executes
  exactly these canonical bytes.
- `Denied { reason: DisbursementReasonCode, gate: AuthorizationGate }`
  — no effect, no settlement; evidence carries the reason and gate.

Cluster 6 consumers MUST NOT re-run any gate, re-derive any reason,
re-validate any destination, or translate a denial into an authorized
effect. The closure ends at this function's return value.

## Handoff targets for Cluster 6

Cluster 6 — **Authority Routing and Submission Boundary** — should
land its work at the following explicit locations:

- Routing / dispatch — consume `AuthorizationResult::Authorized { effect }`
  from `authorize_disbursement`. Route on `effect.asset` and the
  approved-destination set already validated by the registry gate; do
  not re-derive routing from `intent` fields the registry gate has
  already cleared.
- Submission envelope — the adapter's Ziranity CLI shell builds the
  envelope around the canonical bytes of `AuthorizedEffectV1`. The
  domain separator `ORACLEGUARD_DISBURSEMENT_V1` and the well-known
  disbursement OCU constant remain the Ziranity-side contract.
- Evidence bundle — persist the `(intent_bytes, AuthorizationResult)`
  pair. On denial, the bundle carries
  `(reason: DisbursementReasonCode, gate: AuthorizationGate)` and no
  effect.

## What Cluster 6 must NOT do based on Cluster 5 state

- It must not re-run any gate. The orchestrator has already decided.
- It must not re-validate destination authorization. The registry
  gate already did.
- It must not re-run freshness. The grant gate already did, using the
  caller-supplied `now_unix_ms`.
- It must not translate `SubjectNotAuthorized` into a different
  reason code based on whether the underlying cause was requester or
  destination; consumers see the v1 mapping verbatim.
- It must not re-derive `release_cap_basis_points`. The authorized
  effect carries the evaluator-selected band as canonical bytes.
- It must not introduce a new reason code. Any such change is a
  canonical-byte version-boundary event and requires Katiba sign-off.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 5. Cluster 6 begins from a clean CI, a pure
three-gate authorization closure, a single public entry point
(`authorize_disbursement`), a canonical authorized-effect wire type
(`AuthorizedEffectV1`) with byte-level golden replay, and a workspace
in which every remaining ambiguity about what counts as authorization
has been answered in writing, in code, and in tests.
