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

## Cluster 6 landing zones

Cluster 5 — Three-Gate Authorization Closure — is closed. See
`docs/cluster-5-closeout.md` for the handoff record; the pinned
surface it produced is:

- Gate taxonomy `AuthorizationGate` (three `repr(u8)` variants) and
  the `DisbursementReasonCode::gate()` const partition →
  `crates/oracleguard-schemas/src/gate.rs`,
  `crates/oracleguard-schemas/src/reason.rs`
- Canonical authorized-effect wire type `AuthorizedEffectV1` →
  `crates/oracleguard-schemas/src/effect.rs`
- Pure freshness helpers `check_freshness` and the `OracleStale` error
  type (moved from the adapter in Slice 05) →
  `crates/oracleguard-schemas/src/oracle.rs` (the adapter re-exports
  both to keep its public API stable)
- Pure trait boundaries `PolicyAnchorView`, `AllocationRegistryView`,
  and `AllocationFacts` →
  `crates/oracleguard-policy/src/authorize.rs`
- Per-gate pure functions `run_anchor_gate`, `run_registry_gate`,
  `run_grant_gate` →
  `crates/oracleguard-policy/src/authorize.rs`
- Top-level orchestrator `authorize_disbursement(intent, anchor,
  registry, now_unix_ms) -> AuthorizationResult` and the closed
  `AuthorizationResult` type → `crates/oracleguard-policy/src/authorize.rs`
- Golden authorized-effect fixture (postcard-encoded
  `AuthorizedEffectV1`) →
  `fixtures/authorize/authorized_effect_golden.postcard`
- Gate documentation (authoritative order, within-gate precedence,
  reason-to-gate partition, v1 destination-mapping compatibility) →
  `docs/authorization-gates.md`

Cluster 6 — Authority Routing and Submission Boundary — lands its
work at:

- Routing / dispatch — consume `AuthorizationResult::Authorized { effect }`
  and route on the already-validated `effect.asset` and destination.
  Do NOT re-run any gate; do NOT re-derive routing from `intent`
  fields the registry gate has already cleared.
- Submission envelope — the adapter's Ziranity CLI shell builds the
  envelope around `AuthorizedEffectV1` canonical bytes. The domain
  separator `ORACLEGUARD_DISBURSEMENT_V1` and the well-known
  disbursement OCU constant remain the Ziranity-side contract.
- Evidence bundle — persist the `(intent_bytes, AuthorizationResult)`
  pair; on denial the bundle carries `(reason, gate)` and no effect.

See `docs/cluster-1-closeout.md`, `docs/cluster-2-closeout.md`,
`docs/cluster-3-closeout.md`, `docs/cluster-4-closeout.md`, and
`docs/cluster-5-closeout.md` for the prior handoff records.

## Private integration posture

Private runtime integration (consensus hooks, runtime wiring,
Shadowbox-side projection) lives **out of tree**. It depends on the public
crates; it does not redefine any semantic type, canonical encoding, or
evaluator behavior. A judge must be able to reach every public semantic
claim without reading private code.
