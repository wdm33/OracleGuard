# Cluster 1 Closeout — Public Repo Setup and Semantic Boundary Lock

This document is the explicit handoff from Cluster 1 to Cluster 2. It
exists to prove that the workspace is structurally ready for canonical
schema and policy-identity work — not merely present.

## Cluster 1 mechanical completion standard

| Exit criterion (from Cluster 1 overview §6)                             | Satisfied | Evidence                                                                                                         |
|-------------------------------------------------------------------------|-----------|------------------------------------------------------------------------------------------------------------------|
| Public workspace exists with intended crate structure                   | yes       | `Cargo.toml` workspace manifest; `crates/oracleguard-{schemas,policy,adapter,verifier}`                          |
| Crate ownership boundaries are explicit and documented                  | yes       | `docs/architecture.md` ownership table + negative-space rules; per-crate `README.md` with owned / NOT-owned      |
| Dependency direction is mechanically enforced                           | yes       | `scripts/check_deps.sh` runs in CI; `scripts/check_deps_negative_demo.sh` proves it rejects forbidden edges      |
| Public/private semantic boundary is documented                          | yes       | `README.md` public/private + crate map sections; `docs/ziranity-integration.md`; `docs/verifier.md`              |
| Repo-level authoritative-path guardrails exist in CI                    | yes       | `.github/workflows/ci.yml` runs fmt, clippy `-D warnings`, tests; `docs/guardrails.md`; `scripts/demo_guardrail.sh` proves deny fires |
| Repo is structurally ready for Cluster 2 schema work                    | yes       | See readiness questions below                                                                                    |
| No ambiguity about where canonical bytes and public evaluation belong   | yes       | See readiness questions below                                                                                    |

## Readiness questions (Cluster 1 → Cluster 2)

### Can Cluster 2 land `DisbursementIntentV1` without ambiguity about ownership?

**Yes.** The target landing zone is
`crates/oracleguard-schemas/src/intent.rs`. The module already documents
what it owns (versioned intent structs and their canonical byte
encoding) and what it must not own. The workspace ownership table in
`docs/architecture.md` reinforces this.

### Is `policy_ref` derivation clearly expected to live in public code?

**Yes.** Policy identity is explicitly listed in the README "What a
judge can verify without any private code" section and in
`docs/verifier.md` as one of the four offline-verifiable checks. The
canonical encoding from which `policy_ref` is derived lives in
`oracleguard-schemas`; the derivation itself belongs either in
`oracleguard-schemas` (if purely structural) or in
`oracleguard-policy` (if evaluator-adjacent). Cluster 2 will choose
between those two, but both are public crates — the question "is
`policy_ref` public?" is closed: yes.

### Are dependency rules already in place so semantic logic cannot drift into the wrong crate?

**Yes.** `scripts/check_deps.sh` is wired into CI and already rejects:

- any internal dependency out of `oracleguard-schemas`
- `oracleguard-policy` depending on `oracleguard-adapter` or
  `oracleguard-verifier`
- any out-of-tree path dependency

The negative demo (`scripts/check_deps_negative_demo.sh`) has been run
locally and confirmed to fail the check when a forbidden edge is
injected.

### Can a reviewer tell where canonical bytes belong before any real schema lands?

**Yes.** Both the per-crate `README.md` for `oracleguard-schemas` and
the module docstrings in `intent.rs`, `oracle.rs`, `effect.rs`,
`reason.rs`, and `evidence.rs` explicitly state that canonical byte
encoding lives in the schemas crate. No other crate's documentation
claims this responsibility.

## Handoff targets for Cluster 2

Cluster 2 — **Canonical Policy Identity and Intent Schema** — should
land its work at the following explicit locations:

- `DisbursementIntentV1` → `crates/oracleguard-schemas/src/intent.rs`
- Canonical byte encoding primitives → `crates/oracleguard-schemas` (new
  module if not an existing one; the slice should declare it)
- Policy identity type and `policy_ref` derivation → initially
  `crates/oracleguard-schemas` for the data, with derivation in whichever
  of schemas/policy the slice specifies
- Reason codes needed for identity/intent validation → extend
  `crates/oracleguard-schemas/src/reason.rs`
- Identity and intent fixtures → `fixtures/`
- Semantic identity tests → alongside the code in `oracleguard-schemas`
  (unit) plus an integration suite in the schemas crate if needed

## What Cluster 2 must NOT do based on Cluster 1 state

- It must not introduce a path dependency on private Ziranity code.
  `scripts/check_deps.sh` will fail.
- It must not place canonical types in the adapter or verifier crates.
  Per-crate READMEs and the architecture doc both forbid this.
- It must not introduce floating-point arithmetic in the evaluator path.
  `scripts/demo_guardrail.sh` proves this is a real, firing deny.
- It must not weaken the workspace-level `-D warnings`, `unsafe_code =
  forbid`, or `clippy::todo/unimplemented` denies to make progress.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 1. Cluster 2 begins from a clean CI and a workspace
in which every remaining ambiguity about canonical ownership has been
answered in writing and in code.
