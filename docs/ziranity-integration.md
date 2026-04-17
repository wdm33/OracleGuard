# Ziranity Integration

OracleGuard is a thin, policy-governed adapter for Cardano disbursements
that Ziranity uses to convert authorized release decisions into settled
on-chain effects.

This document describes the integration posture *from the public repo's
side*. It does not describe private Ziranity runtime internals, and it
does not ask the reader to trust anything that is not inspectable from
public code.

## Direction of dependency

```
Ziranity private runtime
        │
        │  depends on (consumer)
        ▼
 oracleguard-schemas ──────► oracleguard-policy
        ▲                           ▲
        │                           │
        └────── oracleguard-adapter ┴────── oracleguard-verifier
                 (shell: Charli3, Ziranity CLI submit, Cardano settle)
```

- Private Ziranity runtime **consumes** the public crates.
- Private Ziranity runtime **does not** redefine any public semantic
  type, canonical encoding, evaluator behavior, or verifier surface.
- The public crates have no path dependency on private Ziranity code,
  enforced by `scripts/check_deps.sh`.

## What Ziranity is responsible for

- Runtime wiring of the adapter into the Ziranity process lifecycle.
- Consensus hooks that attach release decisions to Ziranity's state
  machine.
- Shadowbox-side projection wiring.
- Operational secrets (wallet keys, oracle node credentials), which
  must never land in this public repo.

## What OracleGuard is responsible for

- Defining the canonical semantic types that both sides agree on.
- Owning the release-cap evaluator and its deterministic decisions.
- Providing the shell that talks to Charli3, the Ziranity CLI, and
  Cardano.
- Providing an offline verifier that can be pointed at an evidence
  bundle and reach a verdict without any private code.

## Interfaces (conceptual, not yet implemented)

- Ziranity calls `oracleguard-adapter` to submit an authorized
  `DisbursementIntentV1` after its own authorization gate.
- `oracleguard-adapter` normalizes Charli3 oracle data into canonical
  types from `oracleguard-schemas`.
- `oracleguard-policy` evaluates the release cap deterministically and
  produces a decision with a reason code.
- The adapter persists evidence artifacts; a judge later runs
  `oracleguard-verifier` on the bundle to confirm integrity and replay.

Each of these interfaces will be pinned to specific canonical types in
subsequent clusters. The constraint today is that none of those types or
behaviors may be invented privately.

## Trust assumptions

- The reader trusts that the private Ziranity runtime uses these public
  crates unmodified. This can be audited by inspecting the private
  runtime's `Cargo.lock` or equivalent; the public crates do not need
  to trust the private runtime's source code.
- The reader trusts nothing about private runtime internals when
  verifying public claims (policy identity, canonical bytes,
  evaluator output, bundle integrity). All four are reachable from
  this repo alone.
