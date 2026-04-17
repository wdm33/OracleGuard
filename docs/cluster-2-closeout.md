# Cluster 2 Closeout — Canonical Policy Identity and Intent Schema

This document is the explicit handoff from Cluster 2 to Cluster 3. It
exists to prove that the canonical semantic input surface is stable
enough that oracle-normalization work can proceed without reopening
policy identity, intent shape, or canonical-byte ambiguity.

## Cluster 2 mechanical completion standard

| Exit criterion (from Cluster 2 overview §6)                                            | Satisfied | Evidence                                                                                                                                   |
|----------------------------------------------------------------------------------------|-----------|--------------------------------------------------------------------------------------------------------------------------------------------|
| Canonical policy serialization is fixed                                                | yes       | `crates/oracleguard-schemas/src/policy.rs::canonicalize_policy_json`; `fixtures/policy_v1.canonical.bytes`; `docs/policy-canonicalization.md` |
| `policy_ref` derivation is fixed and hash-stable                                       | yes       | `policy::derive_policy_ref`; `PolicyRef([u8;32])`; golden `policy_ref` constant in `policy::tests`; `sha256sum fixtures/policy_v1.canonical.bytes` reproduces it |
| `DisbursementIntentV1` is defined in public schemas                                    | yes       | `crates/oracleguard-schemas/src/intent.rs`; supporting types in `oracle.rs` and `effect.rs`                                                |
| Intent field ordering and canonical encoding are fixed                                 | yes       | `encoding::encode_intent` (postcard, pinned `=1.0.10`); `docs/canonical-encoding.md`                                                       |
| Encode → decode → encode round-trip tests yield identical bytes                        | yes       | `encoding::tests::encode_decode_encode_round_trip_is_identity`                                                                             |
| The same semantic input yields byte-identical payload bytes                            | yes       | `encoding::tests::encode_is_stable_across_calls`; `encoded_bytes_match_golden`                                                             |
| Any semantic field change that should affect identity does affect identity             | yes       | 10 per-field mutation tests in `encoding::tests` covering every identity-bearing field                                                     |
| Versioning rules are explicit and enforced at the schema boundary                      | yes       | `INTENT_VERSION_V1`, `DisbursementIntentV1::new_v1`, `validate_intent_version`, `UnsupportedVersionError`; `docs/canonical-encoding.md` §Version boundary |
| No provenance-only or shell-only concern is accidentally included in semantic identity | yes       | `DisbursementIntentIdentityV1` projection omits `oracle_provenance`; 3 non-interference tests; `docs/semantic-identity.md`                 |

## Readiness questions (Cluster 2 → Cluster 3)

### Is `policy_ref` fixed and publicly derivable?

**Yes.** `derive_policy_ref` takes canonical policy bytes produced by
`canonicalize_policy_json` and returns `PolicyRef([u8; 32])`. The MVP
fixture's `policy_ref` is pinned as a test constant and is independently
reproducible with `sha256sum fixtures/policy_v1.canonical.bytes`.

### Is `DisbursementIntentV1` fixed and versioned?

**Yes.** The struct exposes only fixed-width fields; the destructuring
test in `intent::tests` catches silent field addition. `intent_version`
is a `u16` pinned to `INTENT_VERSION_V1 = 1`, and `validate_intent_version`
is the only sanctioned path to accept v1 vs. reject anything else.

### Are canonical bytes pinned?

**Yes.** `encode_intent` uses `postcard` pinned at `=1.0.10`; the golden
fixture `fixtures/intent_v1_golden.postcard` is matched byte-for-byte
by `encoded_bytes_match_golden`. The decoder rejects trailing bytes and
truncated input, so ambiguous decoding is structurally impossible.

### Are field-sensitivity expectations documented and tested?

**Yes.** `docs/semantic-identity.md` lists every identity-bearing field
and every excluded field with the reason for each. Every row in both
tables has a corresponding mutation or non-interference test in
`encoding::tests`.

### Can Cluster 3 add oracle normalization without changing request identity rules retroactively?

**Yes.** `OracleFactEvalV1` and `OracleFactProvenanceV1` already exist
in `crates/oracleguard-schemas/src/oracle.rs` with the eval/provenance
split baked into the intent. Cluster 3 refines the normalization
semantics (Charli3 → canonical microusd, source canonicalization,
deterministic rejection of malformed inputs) without changing the
identity-bearing field list.

## Handoff targets for Cluster 3

Cluster 3 — **Oracle Fact Normalization and Provenance Separation** —
should land its work at the following explicit locations:

- Refinement of `OracleFactEvalV1` / `OracleFactProvenanceV1` semantics
  → `crates/oracleguard-schemas/src/oracle.rs`
- Asset-pair and source canonical-mapping helpers →
  `crates/oracleguard-schemas/src/oracle.rs` (pure) or a new module
  under `oracleguard-schemas` if the surface grows
- Charli3 response → canonical microusd normalization shell →
  `crates/oracleguard-adapter/src/charli3.rs`
- Deterministic rejection reason codes → extend
  `crates/oracleguard-schemas/src/reason.rs`
- Provenance non-interference tests (policy-level) →
  `crates/oracleguard-policy/src/evaluate.rs` once evaluator logic
  lands (Cluster 4), plus the existing schema-level non-interference
  tests in `oracleguard-schemas::encoding`

## What Cluster 3 must NOT do based on Cluster 2 state

- It must not change the field list or width of `DisbursementIntentV1`.
  `validate_intent_version` and the golden fixture make that a
  version-boundary crossing.
- It must not re-define `OracleFactEvalV1` or `OracleFactProvenanceV1`
  in any non-schemas crate. The public shape is in
  `oracleguard-schemas`; the adapter and policy crates consume it.
- It must not let oracle provenance fields influence authorization.
  The identity projection in `encoding` already structurally excludes
  them, and Cluster 3 must keep that excluded set intact.
- It must not introduce a second canonical encoding or a second
  intent-id path. A single `postcard`-based bytes path and a single
  domain-separated BLAKE3 path are the authoritative surfaces.

## Closeout signature

All mechanical criteria in this document are satisfied on the commit
that closes Cluster 2. Cluster 3 begins from a clean CI, a pinned
golden intent, a pinned golden `policy_ref`, and a workspace in which
every remaining ambiguity about canonical policy identity or intent
identity has been answered in writing and in code.
