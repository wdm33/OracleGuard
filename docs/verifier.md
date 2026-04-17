# OracleGuard Verifier

`oracleguard-verifier` is the offline, judge-facing verification surface
of OracleGuard. It is designed so that any reader of this public repo
can, without running the Ziranity runtime, independently answer:

> *Given this evidence bundle, did the disbursement match the canonical
> policy, canonical intent bytes, and evaluator behavior that the public
> repo claims?*

## What the verifier checks offline

A verifier run, given a bundle on disk, must be able to confirm:

1. **Policy identity** — the canonical bytes of the policy object
   materialized in the bundle digest to the claimed `policy_ref`.
2. **Canonical intent bytes** — the `DisbursementIntentV1` in the bundle
   encodes to the canonical byte sequence defined by
   `oracleguard-schemas`.
3. **Evaluator behavior** — replaying
   `oracleguard-policy` over the canonical inputs reproduces the
   recorded decision and reason code byte-for-byte.
4. **Evidence integrity** — every authorized effect and evidence record
   in the bundle is internally consistent with the canonical types and
   has not been tampered with.

A verifier run must never require network access, the Ziranity runtime,
or any private material. The bundle plus the public crates are the only
inputs.

## What the verifier does NOT do

- It does not redefine any semantic type. It imports canonical meaning
  from `oracleguard-schemas`.
- It does not re-implement the evaluator. It calls into
  `oracleguard-policy` and compares outputs.
- It does not adjudicate policy correctness. It adjudicates
  *consistency* between the claimed bytes, the canonical encoding, and
  the deterministic evaluator output.

## Determinism contract

For a given evidence bundle and a pinned version of the public crates,
`oracleguard-verifier` MUST produce byte-identical verification output
on every run. Non-determinism in the verifier is a bug; it is how
judges detect tampering.

## Judge workflow (target)

1. Clone this repo at a pinned commit.
2. Build the workspace: `cargo build --workspace`.
3. Run the verifier against an evidence bundle produced by an
   OracleGuard-backed disbursement.
4. Read the verifier report. A clean report is evidence that the
   public crates fully explain the disbursement. A failure report
   names which of the four checks above failed, and at which record.

The implementation landing zone for this workflow is
`crates/oracleguard-verifier`. Cluster 1 does not implement any of the
checks above; it only reserves the surface and documents the intent.
