# Evaluator Golden Fixtures

Two golden fixtures pin the evaluator's MVP allow and deny outputs as
canonical byte sequences. The tests in
`crates/oracleguard-policy/tests/golden_evaluator.rs` run the evaluator
on the demo intents from Implementation Plan §16 and assert that the
resulting `EvaluationResult` postcard-encodes to exactly these bytes,
on every run, on every supported platform.

## Fixture files

| Path                                                 | Scenario            | Bytes |
|------------------------------------------------------|---------------------|-------|
| `fixtures/eval/allow_700_ada_result.postcard`        | 700 ADA allow       | 8     |
| `fixtures/eval/deny_900_ada_result.postcard`         | 900 ADA deny        | 2     |

Both bundles encode the crate-local `EvaluationResult` enum defined in
`oracleguard_policy::error`. Postcard's variant-index encoding is part
of the canonical byte surface: `Allow` is variant `0` and `Deny` is
variant `1`.

## Byte anatomy

### Allow: `00 80 ce e4 cd 02 cc 3a`

| Offset | Bytes             | Meaning                                       |
|--------|-------------------|-----------------------------------------------|
| 0      | `00`              | `EvaluationResult::Allow` variant discriminant|
| 1–5    | `80 ce e4 cd 02`  | `authorized_amount_lovelace = 700_000_000` (postcard varint) |
| 6–7    | `cc 3a`           | `release_cap_basis_points = 7_500` (postcard varint) |

### Deny: `01 05`

| Offset | Bytes | Meaning                                             |
|--------|-------|-----------------------------------------------------|
| 0      | `01`  | `EvaluationResult::Deny` variant discriminant       |
| 1      | `05`  | `DisbursementReasonCode::ReleaseCapExceeded` (pinned `repr(u8) = 5`) |

The deny byte matches the discriminant locked in
`oracleguard_schemas::reason` (`reason::tests::repr_u8_discriminants_are_stable`).
If either the result enum or the reason enum is reshuffled, both these
bytes move — and the integration tests fail.

## Narrative mapping (Implementation Plan §16)

| Scenario   | Allocation   | Oracle price   | Band         | Request      | Cap          | Result                           |
|------------|--------------|----------------|--------------|--------------|--------------|----------------------------------|
| Allow      | 1_000 ADA    | $0.45 (450k μUSD) | 7_500 bps | 700 ADA      | 750 ADA      | `Allow { 700 ADA, 7_500 bps }`  |
| Deny       | 1_000 ADA    | $0.45 (450k μUSD) | 7_500 bps | 900 ADA      | 750 ADA      | `Deny { ReleaseCapExceeded }`    |

The allow scenario deliberately sits below the cap; the deny scenario
deliberately sits above. Both scenarios share allocation, oracle
inputs, and everything about the intent except
`requested_amount_lovelace`, so a diff of the two byte sequences is a
diff of exactly one semantic choice: the request amount.

## Regeneration

Regeneration is manual and rare. The canonical bytes should move only
when the evaluator's result shape itself changes — any such change is a
canonical-byte version boundary and requires a Cluster 4 closeout
update.

```bash
cargo test -p oracleguard-policy --test golden_evaluator -- \
    --ignored regenerate
```

The regenerate test writes fresh bytes directly into
`fixtures/eval/*.postcard`. After regeneration, re-run the full
test suite to confirm the non-regenerate assertions still pass:

```bash
bash scripts/deterministic_test.sh
```

Commits that touch the golden fixtures must include a note explaining
why the bytes moved.
