# OracleGuard Architecture

This document is the authoritative ownership reference for the public
OracleGuard workspace. Every crate in the workspace MUST match the
responsibilities described here. If the code or an individual crate README
drifts from this document, the architecture is wrong — not the document.

## Design stance

- OracleGuard is a **thin, policy-governed adapter** for Cardano
  disbursements. It does not absorb policy authority, consensus semantics,
  or runtime wiring.
- The public repo is the **authoritative source** of canonical semantic
  meaning, canonical byte encoding, and judge-facing verification logic.
- Core/shell boundaries are explicit. Pure functional code lives in
  `oracleguard-schemas` and `oracleguard-policy`. Shell code lives in
  `oracleguard-adapter`. `oracleguard-verifier` is a deterministic consumer
  of public semantics, not a redefinition of them.
- Private Ziranity integration depends on the public crates; it does not
  redefine any type, evaluator behavior, or canonical encoding.

## Crate ownership table

| Crate                   | Owns                                                                                                                        | Must NOT own                                                                                        |
|-------------------------|-----------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------------------------------------------------------------|
| `oracleguard-schemas`   | Canonical public semantic types; versioned wire/domain structs; reason codes; authorized-effect types; evidence types.      | Shell logic; I/O; evaluator decisions; verifier orchestration.                                      |
| `oracleguard-policy`    | Pure release-cap evaluation; deterministic decision outputs; math over canonical inputs.                                    | I/O; non-determinism; shell orchestration; consumer of adapter or verifier.                         |
| `oracleguard-adapter`   | Charli3 fetch/normalize shell; Ziranity CLI submission shell; Cardano settlement shell; artifact persistence helpers.       | Semantic meaning; evaluator logic; policy identity derivation; verifier orchestration.              |
| `oracleguard-verifier`  | Offline evidence bundle inspection; replay and integrity checks **using** public semantics.                                 | Alternate semantic definitions; evaluator re-implementation; transport or runtime policy decisions. |

## Non-ownership rules (negative space)

These rules are deliberately redundant with the table above. They exist
because the failure mode being prevented — a crate quietly growing into a
responsibility it was never supposed to own — is common enough to warrant
an explicit prohibition.

- The schemas crate MUST NOT contain shell logic, I/O, or evaluator
  behavior. It is a type and encoding authority only.
- The policy crate MUST NOT perform I/O, use non-deterministic constructs,
  or depend on the adapter or verifier crates. Its inputs are canonical
  semantic values; its outputs are deterministic decisions.
- The adapter crate MUST NOT define semantic meaning, own policy-identity
  derivation, or make release-cap decisions. It only transports, persists,
  and normalizes material that the schemas and policy crates have already
  given meaning to.
- The verifier crate MUST NOT reinvent canonical encoding, define a second
  evaluator, or introduce "alternate interpretations" of public semantics.
  It verifies bundles *with* public semantics; it does not author them.
- No crate may introduce a generic `utils` module without a bounded,
  documented purpose. Generic catch-alls are how ownership boundaries rot.

## Dependency-direction policy

The workspace enforces a strict dependency direction. The rules below are
authoritative. `scripts/check_deps.sh` is the mechanical enforcer and is
wired into CI; `scripts/check_deps_negative_demo.sh` is a spot-check demo
that proves the enforcer rejects a forbidden edge.

Allowed edges:

| From                   | May depend on                                             |
|------------------------|-----------------------------------------------------------|
| `oracleguard-schemas`  | nothing in this workspace                                 |
| `oracleguard-policy`   | `oracleguard-schemas`                                     |
| `oracleguard-adapter`  | `oracleguard-schemas`, `oracleguard-policy`               |
| `oracleguard-verifier` | `oracleguard-schemas`, `oracleguard-policy`               |

Forbidden at all times:

- Any edge from a lower-authority crate (adapter, verifier) being consumed
  by a higher-authority crate (schemas, policy).
- Any edge between adapter and verifier in either direction — they are
  peer consumers of public semantics, not of each other.
- Any out-of-tree `path = "..."` dependency. Private Ziranity code is not
  a path dependency of any public crate.

If `scripts/check_deps.sh` and this table ever disagree, the script is the
source of truth; update the table to match, not the other way around.

## Cluster 7 landing zones

Cluster 6 — Authority Routing and Submission Boundary — is closed.
See `docs/cluster-6-closeout.md` for the handoff record; the pinned
surface it produced is:

- Fixed disbursement authority OCU constant
  `DISBURSEMENT_OCU_ID: [u8; 32]` with its derivation recipe
  (`DISBURSEMENT_OCU_DOMAIN`, `DISBURSEMENT_OCU_SEED`,
  `derive_disbursement_ocu_id`) →
  `crates/oracleguard-schemas/src/routing.rs`;
  `fixtures/routing/disbursement_ocu_id.hex`;
  `docs/authority-routing.md`
- Single-authority routing rule `route(intent) -> [u8; 32]` that
  always returns `DISBURSEMENT_OCU_ID`, with invariance tests over
  non-authority field mutations →
  `crates/oracleguard-schemas/src/routing.rs`
- Canonical payload emission named alias
  `emit_payload(intent) -> Vec<u8>` and strict `decode_intent` →
  `crates/oracleguard-schemas/src/encoding.rs`
- Adapter payload helpers `payload_bytes` + `write_payload_to_file`
  (RED disk write of canonical bytes) →
  `crates/oracleguard-adapter/src/ziranity_submit.rs`
- CLI submission wrapper `SubmitConfig`, pure `build_cli_args`,
  RED `submit_intent` spawning `ziranity intent submit` via
  `std::process::Command` →
  `crates/oracleguard-adapter/src/ziranity_submit.rs`;
  `fixtures/routing/sample_cli_args.golden.txt`
- Canonical `AuthorizationResult::{encode, decode}` (strict,
  trailing-byte-rejecting postcard parser) →
  `crates/oracleguard-policy/src/authorize.rs`
- Adapter consensus-output intake: `IntentReceiptV1`,
  `parse_consensus_output`, `parse_cli_receipt`, `render_cli_receipt`
  with a structural audit test pinning no-reinterpretation →
  `crates/oracleguard-adapter/src/intake.rs`
- Routing/submission-boundary integration suite and CI gate →
  `crates/oracleguard-adapter/tests/routing_boundary.rs`;
  `scripts/check_routing_determinism.sh`

Cluster 7 — Authorized Effect and Cardano Fulfillment — lands its
work at:

- Settlement construction — consume `IntentReceiptV1.output` matched
  against `AuthorizationResult::Authorized { effect }` and build the
  Cardano transaction from `AuthorizedEffectV1` fields. Do NOT re-run
  authorization, re-derive `release_cap_basis_points`, or reshape the
  authorized effect.
- Denial path — on `AuthorizationResult::Denied { reason, gate }`,
  settlement does not execute. Evidence captures `(reason, gate)`
  verbatim; no local retry or fixup.
- Bundle persistence — record the `(intent_bytes, receipt_bytes,
  cardano_tx_bytes)` trail for allow; `(intent_bytes, receipt_bytes)`
  for deny. Canonical bytes come from Cluster 6 surfaces, not from
  fresh re-encoding.

See `docs/cluster-1-closeout.md`, `docs/cluster-2-closeout.md`,
`docs/cluster-3-closeout.md`, `docs/cluster-4-closeout.md`,
`docs/cluster-5-closeout.md`, and `docs/cluster-6-closeout.md` for
the prior handoff records.

## Private integration posture

Private runtime integration (consensus hooks, runtime wiring,
Shadowbox-side projection) lives **out of tree**. It depends on the public
crates; it does not redefine any semantic type, canonical encoding, or
evaluator behavior. A judge must be able to reach every public semantic
claim without reading private code.
