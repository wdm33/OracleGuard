# Authoritative-Path Guardrails

This repo ships CI guardrails that are stricter on authoritative core
crates than on shell crates. The guardrails are mechanical: they are
executed in CI and a negative demo proves they actually fail on the
violation they claim to prevent.

## What runs in CI

Every push and pull request runs:

1. `cargo fmt --all --check` — formatting drift is a merge blocker.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clippy
   warnings are errors workspace-wide.
3. `cargo build --workspace --locked` — the workspace must build with
   pinned dependencies.
4. `cargo test --workspace` — tests must pass with a fixed thread
   count, as configured below.
5. `./scripts/check_deps.sh` — dependency-direction enforcement (see
   `docs/architecture.md`).

## Core vs shell: lint strictness

Workspace baseline (every crate):

- `unsafe_code = forbid` (workspace lint)
- `clippy::todo`, `clippy::unimplemented` = deny (workspace lint)

Authoritative core crates — `oracleguard-schemas` and
`oracleguard-policy` — apply an **additional** crate-level attribute
bundle in `lib.rs`:

- `#![forbid(unsafe_code)]`
- `#![deny(clippy::float_arithmetic)]` — non-negotiable for the
  evaluator's determinism story.
- `#![deny(clippy::panic)]`
- `#![deny(clippy::unwrap_used, clippy::expect_used)]` — panics in core
  paths are a bug, not a convenience.
- `#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]`
  — pure core must not perform output.

Shell crates — `oracleguard-adapter` and `oracleguard-verifier` —
inherit only the workspace baseline. They need panics, I/O, and
structured logging to do their jobs, and the workspace-wide
`-D warnings` + `unsafe_code = forbid` is enough for the shell tier.

This split is deliberate. It would be false safety to apply the same
strict lints uniformly; the architectural claim is that core and shell
are *different*, and the lint profile reflects that.

## Determinism posture for tests

`cargo test` in CI runs with thread discipline that keeps output
deterministic. See `scripts/deterministic_test.sh` for the exact
invocation.

## The guardrail is real

`scripts/demo_guardrail.sh` temporarily injects a banned construct
(`let _ = 1.0_f64 + 2.0;`) into `oracleguard-policy/src/lib.rs`, runs
`cargo clippy -p oracleguard-policy -- -D warnings`, asserts the build
fails, and restores the file. Run it locally to confirm the
`float_arithmetic` deny is not aspirational.

If a future contributor needs an exception, it must be documented
inline with a `#[allow(...)]` attribute and a justification comment.
Blanket exceptions in core crates are forbidden.
