# OracleGuard

An independently verifiable, policy-governed Cardano disbursement adapter.

## Public Crates

This repository is a Cargo workspace. The public API surface lives in four
crates, each with a single, non-overlapping responsibility:

- **`oracleguard-schemas`** — canonical public semantic types (versioned wire
  and domain structs, reason codes, authorized-effect types, evidence types).
- **`oracleguard-policy`** — pure deterministic release-cap evaluation over
  the semantic types defined in `oracleguard-schemas`. No I/O.
- **`oracleguard-adapter`** — imperative shell for Charli3 oracle fetch,
  Ziranity CLI submission, Cardano settlement, and artifact persistence.
  Consumes public semantics; does not redefine them.
- **`oracleguard-verifier`** — offline evidence bundle verifier. Uses the
  public semantics; does not introduce alternate interpretations.

The public repo is the authoritative source of OracleGuard semantic meaning,
canonical bytes, and judge-facing verification logic. Private integration
depends on these crates and must not redefine their semantics.

## Public / Private Boundary

### What is public (in this repo)

- Canonical semantic types (`oracleguard-schemas`)
- Canonical byte encoding and policy identity derivation
- The release-cap evaluator (`oracleguard-policy`)
- The offline evidence bundle verifier (`oracleguard-verifier`)
- The shell adapter for Charli3, Ziranity, and Cardano
  (`oracleguard-adapter`)
- Demos and fixtures

### What is private (out of tree)

- Ziranity runtime wiring
- Consensus hooks
- Shadowbox-side projection wiring

### What a judge can verify without any private code

- Policy identity: canonical bytes for the policy object and its digest
- Canonical intent bytes: `DisbursementIntentV1` encoding and digest
- Evaluator behavior: allow/deny decisions over seeded fixtures
- Offline evidence inspection: verifier runs end-to-end on a bundle
  without reaching into any private runtime

### Trust assumptions, stated plainly

- The private runtime does not redefine any public semantic type, byte
  encoding, or evaluator behavior. It depends on these crates as a
  consumer.
- The public crates are the sole source of canonical meaning. A reader of
  the public repo alone can understand every semantic claim OracleGuard
  makes.

See `docs/architecture.md`, `docs/verifier.md`, and
`docs/ziranity-integration.md` for the full narrative.

## Crate Map

| Crate                   | Purpose                                                                 | Depends on (internal)                                |
|-------------------------|-------------------------------------------------------------------------|------------------------------------------------------|
| `oracleguard-schemas`   | Canonical public semantic types and encoding.                           | —                                                    |
| `oracleguard-policy`    | Pure deterministic release-cap evaluation.                              | `oracleguard-schemas`                                |
| `oracleguard-adapter`   | Shell: Charli3 fetch, Ziranity CLI submit, Cardano settlement.          | `oracleguard-schemas`, `oracleguard-policy`          |
| `oracleguard-verifier`  | Offline evidence bundle inspection and replay.                          | `oracleguard-schemas`, `oracleguard-policy`          |

## Build

```
cargo build --workspace
```

## Git Hooks

After cloning, enable the repository's managed hooks:

```
git config core.hooksPath .githooks
```

The `pre-commit` hook scans staged content for secrets that must never
reach a public repo: mnemonics, Cardano signing keys, raw Ed25519 key
bytes, and bearer tokens. The `commit-msg` hook enforces commit-message
hygiene.

## License

MIT. See `LICENSE`.
