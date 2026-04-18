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

## Cluster 8 landing zones

Cluster 7 — Authorized Effect and Cardano Fulfillment — is closed.
See `docs/cluster-7-closeout.md` for the handoff record and
`docs/fulfillment-boundary.md` for the reviewer-facing explanation.
The pinned surface it produced is:

- `AuthorizedEffectV1` pinned as the sole fulfillment-input surface
  (expanded docstring + schema-scope golden fixture test) →
  `crates/oracleguard-schemas/src/effect.rs`;
  `fixtures/authorize/authorized_effect_golden.postcard`
- Exact effect-to-settlement mapping `SettlementRequest` +
  `settlement_request_from_effect` (pure, byte-identical field copy-
  through; independent of authorization-provenance fields) →
  `crates/oracleguard-adapter/src/settlement.rs`
- ADA-only MVP fulfillment guard `guard_mvp_asset` +
  `FulfillmentRejection::NonAdaAsset` →
  `crates/oracleguard-adapter/src/settlement.rs`
- Deny/no-tx closure via the single entrypoint `fulfill` +
  `FulfillmentOutcome { Settled, DeniedUpstream, RejectedAtFulfillment }`
  and `FulfillmentRejection::ReceiptNotCommitted` for pending
  receipts; structural audit test pins that the production source
  does not call any evaluator or per-gate function →
  `crates/oracleguard-adapter/src/settlement.rs`
- Transaction-hash capture type `CardanoTxHashV1` (32-byte
  BLAKE2b-256, strict lowercase-hex round-trip) and the
  `SettlementBackend` submission trait →
  `crates/oracleguard-adapter/src/cardano.rs`;
  `fixtures/cardano/tx_hash_sample.hex`
- RED `CardanoCliSettlementBackend` skeleton that captures a tx id
  via `cardano-cli transaction txid --tx-file <path>` →
  `crates/oracleguard-adapter/src/cardano.rs`
- Fulfillment-boundary integration suite and CI gate →
  `crates/oracleguard-adapter/tests/fulfillment_boundary.rs`;
  `scripts/check_fulfillment_determinism.sh`

Cluster 8 — Evidence Bundle and Offline Verification — landed the
following public surfaces:

- Canonical evidence schema `DisbursementEvidenceV1` linking
  policy reference, canonical intent, evaluator inputs
  (`allocation_basis_lovelace`, `now_unix_ms`), the three-gate
  `AuthorizationSnapshotV1`, and the typed `ExecutionOutcomeV1`
  (`Settled { tx_hash }`, `DeniedUpstream { reason, gate }`,
  `RejectedAtFulfillment { kind }`) →
  `crates/oracleguard-schemas/src/evidence.rs`
- Evidence assembly from adapter-domain outcomes via `From` impls
  (`CardanoTxHashV1 → [u8; 32]`,
  `FulfillmentRejection → FulfillmentRejectionKindV1`,
  `FulfillmentOutcome → ExecutionOutcomeV1`,
  `AuthorizationResult → AuthorizationSnapshotV1`) plus
  `assemble_disbursement_evidence` →
  `crates/oracleguard-adapter/src/artifacts.rs`,
  `crates/oracleguard-policy/src/authorize.rs`
- Four MVP golden fixtures covering Settled, DeniedUpstream, and
  both `RejectedAtFulfillment` kinds →
  `fixtures/evidence/{allow_700_ada,deny_900_ada,reject_non_ada,reject_pending}_bundle.postcard`
- Offline verifier (`verify_bundle`, `verify_evidence`) running
  five checks — version pin, `intent_id` recomputation,
  authorized-effect byte-identity against the intent,
  `gate == reason.gate()` partition, cross-variant
  authorization/execution matrix — plus pure-evaluator
  replay-equivalence → `crates/oracleguard-verifier`
- Typed closed-set `VerifierFinding` enum and `VerifierReport`
  aggregate for byte-identically reproducible judge-facing
  output → `crates/oracleguard-verifier/src/report.rs`
- Verification-surface integration suite and CI gate →
  `crates/oracleguard-verifier/tests/verification_surface.rs`;
  `scripts/check_evidence_determinism.sh`

See `docs/cluster-1-closeout.md`, `docs/cluster-2-closeout.md`,
`docs/cluster-3-closeout.md`, `docs/cluster-4-closeout.md`,
`docs/cluster-5-closeout.md`, `docs/cluster-6-closeout.md`,
`docs/cluster-7-closeout.md`, and `docs/cluster-8-closeout.md` for
the full handoff records. Cluster 8 completes the initial
OracleGuard MVP invariant-cluster sequence;
`docs/evidence-bundle.md` is the judge-facing public explanation of
the verification surface.

## Private integration posture

Private runtime integration (consensus hooks, runtime wiring,
Shadowbox-side projection) lives **out of tree**. It depends on the public
crates; it does not redefine any semantic type, canonical encoding, or
evaluator behavior. A judge must be able to reach every public semantic
claim without reading private code.
