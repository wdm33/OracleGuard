# OracleGuard Evidence Bundle and Offline Verifier

This document is the judge-facing public explanation of how
OracleGuard records the outcome of a governed disbursement and how an
outside reviewer can verify it without trusting the operator, the
Ziranity runtime, or the Cardano node.

Everything described here is mechanically enforced by the public
crates in this repo. Prose does not add claims; the tests do.

## 1. What evidence is

Every OracleGuard disbursement — allow, upstream deny, or
fulfillment-side refusal — produces a single typed record:

```
oracleguard_schemas::evidence::DisbursementEvidenceV1
```

The record links five surfaces into one inspectable bundle:

| Surface | Field(s) | Role |
|---|---|---|
| Policy identity | `intent.policy_ref` | Katiba-anchored policy the request was evaluated against. |
| Canonical request | `intent`, `intent_id` | The `DisbursementIntentV1` bytes the runtime evaluated, plus the recomputed 32-byte identity hash. |
| Evaluator inputs | `allocation_basis_lovelace`, `now_unix_ms` | The caller-supplied values the public evaluator needs to replay the decision deterministically. |
| Consensus outcome | `committed_height`, `authorization` | The block height the decision committed at, and the typed three-gate verdict. |
| Execution outcome | `execution` | The typed fulfillment result — either `Settled { tx_hash }`, `DeniedUpstream { reason, gate }`, or `RejectedAtFulfillment { kind }`. |

The record is `postcard`-canonical. Bytes on disk are
byte-identically reproducible from the in-memory value via
`oracleguard_schemas::evidence::encode_evidence` / `decode_evidence`,
both of which reject trailing bytes so one byte sequence cannot mean
two things.

## 2. Evidence is emitted for every outcome

OracleGuard does not privilege successful execution. The evidence
schema distinguishes three non-equivalent outcome categories, and
the MVP ships a golden fixture for each:

| Scenario | Fixture | `authorization` | `execution` |
|---|---|---|---|
| Allow — 700 ADA under the 75% band | `fixtures/evidence/allow_700_ada_bundle.postcard` | `Authorized { effect }` | `Settled { tx_hash }` |
| Deny — 900 ADA exceeds the 75% cap | `fixtures/evidence/deny_900_ada_bundle.postcard` | `Denied { ReleaseCapExceeded, Grant }` | `DeniedUpstream { ReleaseCapExceeded, Grant }` |
| Refusal — non-ADA allocation | `fixtures/evidence/reject_non_ada_bundle.postcard` | `Authorized { effect }` | `RejectedAtFulfillment { NonAdaAsset }` |
| Refusal — pending receipt | `fixtures/evidence/reject_pending_bundle.postcard` | `Authorized { effect }` | `RejectedAtFulfillment { ReceiptNotCommitted }` |

A denial is an upstream authorization verdict. A refusal is a
fulfillment-side guard tripping after authorization allowed the
effect. Both produce no Cardano transaction, but the two categories
stay structurally distinct so a reviewer can always tell them apart.

The `deny_and_refusal_fixtures_carry_distinct_execution_surfaces`
test and the cross-variant consistency check in the verifier
(`integrity.rs`) mechanically pin that no outcome collapses into an
ambiguous bucket.

## 3. What the verifier checks

`oracleguard-verifier` runs four stages in a fixed order and appends
typed findings to a [`VerifierReport`]; it never short-circuits, so a
tampered bundle surfaces every inconsistency in one run.

1. **Bundle load** (`bundle::load_bundle`) — read bytes from disk and
   decode through the canonical schemas path. Trailing bytes are
   rejected; I/O and decode errors surface distinctly.
2. **Version pin** (`integrity::check_integrity`) —
   `evidence.evidence_version` and `evidence.intent.intent_version`
   must both equal their v1 constants.
3. **Intent-id recomputation** — recompute
   `oracleguard_schemas::encoding::intent_id(&evidence.intent)` and
   compare to `evidence.intent_id`. Any mutation of an
   identity-bearing intent field produces an `IntentIdMismatch`
   finding.
4. **Cross-field consistency** —
   - When `authorization == Authorized { effect }`, every
     identity-bearing effect field (`policy_ref`, `allocation_id`,
     `requester_id`, `destination`, `asset`,
     `authorized_amount_lovelace`) must byte-match the intent.
   - When `authorization == Denied { reason, gate }`, the
     `gate == reason.gate()` partition from
     `oracleguard_schemas::reason` must hold.
   - The `authorization` × `execution` variant pair must be one of
     the allowed combinations; the disallowed pairings
     (`Authorized` with `DeniedUpstream`, `Denied` with `Settled`,
     `Denied` with `RejectedAtFulfillment`, `Denied` with mismatched
     `reason` or `gate`) produce typed inconsistency findings.
5. **Replay equivalence** (`replay::check_replay_equivalence`) —
   re-run
   `oracleguard_policy::evaluate::evaluate_disbursement(&evidence.intent, evidence.allocation_basis_lovelace)`
   and compare against the evaluator projection of the recorded
   authorization snapshot. For pre-evaluator gate denials
   (`PolicyNotFound`, `AllocationNotFound`, `SubjectNotAuthorized`,
   `AssetMismatch`, `OracleStale`) the evaluator was never called and
   replay is a no-op — the verifier does not invent a mismatch it
   cannot reproduce.

Every finding variant is a closed typed value (no free-form
strings). That keeps reports byte-identically reproducible so CI can
diff them across runs.

## 4. What the verifier does NOT check

The verifier is scoped deliberately:

- It does **not** adjudicate policy correctness. Katiba is the
  policy authority; the verifier only checks that evidence is
  internally consistent with the canonical types and that replay
  reproduces the recorded decision.
- It does **not** re-execute Cardano. The `tx_hash` field on
  `Settled` is recorded provenance — the chain is out of scope.
  Mutating `tx_hash` changes the bundle bytes (CI catches that via
  the fixture goldens) but does not produce a verifier finding.
- It does **not** re-implement the evaluator or any semantic type.
  Every check imports meaning from `oracleguard-schemas` and
  `oracleguard-policy`.
- It does **not** require the Ziranity runtime, network access, or
  any private material. The bundle file and the public crates are
  the only inputs.

## 5. Running the verifier offline

```bash
git clone https://github.com/vadrant-labs/OracleGuard
cd OracleGuard
cargo build --workspace

# Integration suite (all four MVP fixtures, 20 checks):
./scripts/check_evidence_determinism.sh

# Per-fixture programmatic verification:
cargo test -p oracleguard-verifier --test verification_surface \
    -- --test-threads=1
```

The verifier's library entry points are `verify_bundle(path)` (I/O +
decode + full pipeline) and `verify_evidence(&DisbursementEvidenceV1)`
(value-in, full pipeline). A clean `VerifierReport` is evidence that
the public crates fully explain the disbursement; a report with
findings names which check failed and at which record.

## 6. Determinism contract

For a given evidence bundle and a pinned version of the public
crates, the verifier MUST produce byte-identical output on every
run. Non-determinism is a bug; judges rely on reproducibility to
detect tampering.

The integration suite in
`crates/oracleguard-verifier/tests/verification_surface.rs` pins
this by running eight verifier passes per fixture and asserting
`assert_eq!` across every pair. CI runs the suite under
`--test-threads=1` via `scripts/check_evidence_determinism.sh`.
