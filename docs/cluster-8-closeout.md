# Cluster 8 Closeout — Evidence Bundle and Offline Verification

This document closes Cluster 8 and, with it, the initial OracleGuard
MVP invariant-cluster sequence. It exists to prove that the public
evidence and verification surface is stable enough for judges and
external reviewers to inspect an OracleGuard-mediated disbursement
without private trust.

## Cluster 8 mechanical completion standard

| Exit criterion (Cluster 8 overview §6) | Satisfied | Evidence |
|---|---|---|
| A public evidence schema exists for disbursement outcomes | yes | `oracleguard_schemas::evidence::DisbursementEvidenceV1` is a fixed-width, postcard-canonical struct with per-field docstrings. Supporting types `AuthorizationSnapshotV1`, `ExecutionOutcomeV1`, and `FulfillmentRejectionKindV1` pin the three outcome-side surfaces; `EVIDENCE_VERSION_V1` pins the schema version. Unit tests in `crates/oracleguard-schemas/src/evidence.rs` include postcard round-trips for every variant, destructuring guards, discriminant pins, and byte-identity mutation sensitivity. |
| Evidence includes policy reference, oracle fact reference, request identity, decision result, and execution outcome | yes | The struct embeds the full canonical `DisbursementIntentV1` (which carries `policy_ref`, `allocation_id`, `requester_id`, `oracle_fact`, `oracle_provenance`, `destination`, `asset`, and `requested_amount_lovelace`), the recomputed `intent_id`, the caller-supplied evaluator inputs (`allocation_basis_lovelace`, `now_unix_ms`), the `committed_height`, the three-gate `AuthorizationSnapshotV1`, and the typed `ExecutionOutcomeV1`. The `allow_700_ada_golden_covers_full_governed_action_chain` test destructures every major field on the allow fixture. |
| Denial and failure cases emit evidence, not silence | yes | Four fixtures ship: `allow_700_ada_bundle.postcard` (Settled), `deny_900_ada_bundle.postcard` (DeniedUpstream ReleaseCapExceeded/Grant), `reject_non_ada_bundle.postcard` (RejectedAtFulfillment NonAdaAsset), `reject_pending_bundle.postcard` (RejectedAtFulfillment ReceiptNotCommitted). Test `deny_and_refusal_fixtures_carry_distinct_execution_surfaces` pins that all three non-success outcomes surface distinctly; `check_cross_variant_consistency` in `integrity.rs` forbids the disallowed pairings (Settled on deny, DeniedUpstream on allow, reason/gate mismatch). |
| Offline verifier logic exists in the public repo | yes | `crates/oracleguard-verifier` is fleshed out across four modules: `bundle.rs` (load + decode), `integrity.rs` (four structural checks), `replay.rs` (pure-evaluator re-run), `report.rs` (closed typed finding enum). Public entry points are `verify_bundle(path)` and `verify_evidence(value)` exposed from `lib.rs`. |
| The verifier checks bundle integrity and stable semantic linkage between evidence components | yes | `check_integrity` enforces version pins, `intent_id` recomputation against the embedded intent, byte-identity between every `AuthorizedEffectV1` field and the intent, the `gate == reason.gate()` partition on denials, and the cross-variant authorization/execution matrix. 22 unit tests in `integrity.rs` cover every finding variant; 12 integration tests in `tests/verification_surface.rs` sweep mutations of authoritative fields and assert at least one typed finding for each. |
| Replay-equivalence path is documented and testable for the public decision surface | yes | `replay::check_replay_equivalence` re-runs `oracleguard_policy::evaluate::evaluate_disbursement(&evidence.intent, evidence.allocation_basis_lovelace)` and compares against the evaluator projection of the recorded authorization snapshot. Pre-evaluator gate denials (PolicyNotFound/AllocationNotFound/SubjectNotAuthorized/AssetMismatch/OracleStale) make replay a no-op — the verifier never invents mismatches. Tests `public_evaluator_reproduces_allow_fixture` and `public_evaluator_reproduces_deny_fixture` assert equivalence end-to-end; `replay_flags_divergence_when_amount_mutated` proves the check actually runs. |
| Allow and deny example bundles exist | yes | `fixtures/evidence/allow_700_ada_bundle.postcard` (602 bytes) and `fixtures/evidence/deny_900_ada_bundle.postcard` (352 bytes), both generated via `#[ignore]` regeneration tests (`regenerate_allow_700_ada_golden_fixture`, `regenerate_deny_900_ada_golden_fixture`). Golden tests assert byte equality on encode, exact decode, and full governed-action-chain linkage. |
| Public docs explain clearly what an external verifier can check without private runtime access | yes | `docs/evidence-bundle.md` is the single judge-facing explanation: schema field roles, the four outcome categories with their fixtures, the five verifier checks, explicit non-goals (no policy adjudication, no Cardano re-execution), the offline workflow, and the determinism contract. `docs/cluster-8-closeout.md` (this document) covers mechanical completion and MVP closeout. `docs/verifier.md` remains the earlier landing-zone doc pointing at the same public crate. |
| The verifier consumes public semantics rather than redefining them | yes | `oracleguard-verifier/Cargo.toml` depends only on `oracleguard-schemas` and `oracleguard-policy`; `scripts/check_deps.sh` forbids a verifier → adapter edge. Every check in `integrity.rs` and `replay.rs` imports types from schemas/policy rather than redeclaring them. The adapter's evidence-assembly module (`artifacts.rs`) provides `From` impls into the canonical schemas types so no adapter-defined type leaks into the bundle bytes. |

## Readiness questions (Cluster 8 → MVP closeout)

### Is the public evidence typed and complete?

**Yes.** `DisbursementEvidenceV1` is a fixed-width, postcard-canonical
struct whose field list covers policy reference, canonical intent
bytes (via embedding), request identity (via recomputed `intent_id`),
evaluator inputs (`allocation_basis_lovelace`, `now_unix_ms`),
consensus provenance (`committed_height`), the three-gate
authorization verdict (`AuthorizationSnapshotV1`), and the typed
execution outcome (`ExecutionOutcomeV1`). Destructuring guards in the
schemas crate fail the compile if any field is added, removed, or
reordered without a deliberate schema-version bump.

### Do allow, deny, and applicable refusal cases emit evidence?

**Yes.** All four MVP outcome categories have pinned fixtures and
golden tests:

- `fixtures/evidence/allow_700_ada_bundle.postcard` — 700 ADA allow
  under the 75% band, `Settled { tx_hash }`.
- `fixtures/evidence/deny_900_ada_bundle.postcard` — 900 ADA deny
  for `ReleaseCapExceeded`/`Grant`, `DeniedUpstream`.
- `fixtures/evidence/reject_non_ada_bundle.postcard` — non-ADA
  allocation refusal at the fulfillment boundary,
  `RejectedAtFulfillment { NonAdaAsset }`.
- `fixtures/evidence/reject_pending_bundle.postcard` — pending
  receipt refusal, `RejectedAtFulfillment { ReceiptNotCommitted }`.

The `deny_and_refusal_fixtures_carry_distinct_execution_surfaces`
test pins that deny, non-ADA refusal, and pending refusal surface as
three non-equivalent outcomes.

### Does the offline verifier exist and check more than superficial parsing?

**Yes.** Five checks run on every bundle:

1. evidence/intent version pins,
2. `intent_id` recomputation,
3. `Authorized`-effect byte-identity against the intent (six fields),
4. `Denied`-snapshot `gate == reason.gate()` partition,
5. authorization/execution cross-variant consistency (five
   disallowed pairings, each with its own typed inconsistency
   variant), plus
6. pure-evaluator replay-equivalence against the recorded
   authorization snapshot.

Every check is exercised by at least one positive and one negative
test; the integration suite in `tests/verification_surface.rs`
sweeps mutations of every authoritative field and asserts at least
one finding per mutation.

### Are replay-equivalence claims tested for the public decision path?

**Yes.** `public_evaluator_reproduces_allow_fixture` and
`public_evaluator_reproduces_deny_fixture` re-run
`evaluate_disbursement` over the canonical inputs in the bundle and
assert the result matches the recorded authorization snapshot.
`replay_flags_divergence_when_amount_mutated` proves the check
actually runs by feeding it a mutated evidence record and asserting
it produces a `ReplayDiverged` finding. CI runs
`check_evidence_determinism.sh` under `--test-threads=1` so reports
are byte-identically reproducible across runs.

### Can an outside reviewer follow policy → oracle → intent → decision → execution without private runtime access?

**Yes.** The bundle embeds the full canonical intent (which carries
the policy reference and the oracle fact), the recomputed
`intent_id`, the three-gate verdict, and the typed execution outcome.
A reviewer runs:

```bash
git clone https://github.com/vadrant-labs/OracleGuard
cd OracleGuard
cargo build --workspace
./scripts/check_evidence_determinism.sh
```

and gets a clean report without ever contacting the Ziranity
runtime, Kupo, Charli3, or a Cardano node. `docs/evidence-bundle.md`
is the single document that explains what each check means and what
it does not check (chain re-execution and policy adjudication are
explicitly out of scope).

## Handoff targets — cluster sequence completion

Cluster 8 closes the initial OracleGuard MVP cluster sequence:

1. Public Repo Setup and Semantic Boundary Lock *(completed)*
2. Canonical Policy Identity and Intent Schema *(completed)*
3. Oracle Fact Normalization and Provenance Separation *(completed)*
4. Release-Cap Evaluation Logic *(completed)*
5. Three-Gate Authorization Closure *(completed)*
6. Authority Routing and Submission Boundary *(completed)*
7. Authorized Effect and Cardano Fulfillment *(completed)*
8. Evidence Bundle and Offline Verification *(this cluster)*

With Cluster 8 closed, OracleGuard is presentable as a governed-
action system with an independently inspectable verification surface.
Work beyond this sequence — production genesis, operational
hardening, multi-asset support, alternative settlement backends —
requires its own invariant-cluster sequence.

## What Cluster 8 did NOT do

- It did **not** build a second decision engine. The verifier calls
  the real `evaluate_disbursement` function exported by
  `oracleguard-policy`; there is no shadow evaluator.
- It did **not** build a second receipt engine. Evidence assembly
  consumes `FulfillmentOutcome` from the adapter verbatim.
- It did **not** re-run Cardano. The recorded `tx_hash` is
  provenance; the chain is out of scope for offline verification.
- It did **not** add wall-clock reads to any authoritative path.
  The `now_unix_ms` field in evidence is carried as provenance,
  unused by the evaluator or the integrity check.
- It did **not** introduce a second execution-command surface.
  `SettlementRequest` remains the only typed settlement input;
  evidence carries a mirror of `FulfillmentOutcome`, not an
  alternative.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 8. CI is green on `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`scripts/check_deps.sh`, `scripts/deterministic_test.sh`,
`scripts/check_fulfillment_determinism.sh`,
`scripts/check_routing_determinism.sh`, and the new
`scripts/check_evidence_determinism.sh`. OracleGuard's MVP
invariant-cluster sequence is complete.
