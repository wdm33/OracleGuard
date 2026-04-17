# Canonical Domain Encoding

This document pins the byte-level encoding rules for OracleGuard
domain payloads. The only authoritative encoding is the one defined
here; every downstream check (intent-id hashing, submission, evidence
replay, verifier re-encoding) depends on it.

## Encoding choice

- **Format:** `postcard` — a compact, deterministic binary encoder
  for `serde`-enabled types.
- **Pinned major version:** `postcard = "=1.0.10"` (exact) in
  `crates/oracleguard-schemas/Cargo.toml`. The pinned major version is
  part of the encoding contract. A postcard major version bump would
  change the canonical bytes and therefore must be treated as a
  schema-version boundary change.

## What is encoded

The canonical encoding covers public semantic types defined in
`oracleguard-schemas`:

- `DisbursementIntentV1` (and its fixed-width nested types:
  `OracleFactEvalV1`, `OracleFactProvenanceV1`, `CardanoAddressV1`,
  `AssetIdV1`)

Future versioned types (e.g. `DisbursementIntentV2`) will get their own
canonical encoding path and their own golden fixtures; v1 remains
frozen.

## Round-trip contract

For any valid value `x`:

```
decode(encode(x)) == x
encode(decode(encode(x))) == encode(x)
```

The decoder is strict:
- Trailing bytes after a complete decode are rejected with
  `CanonicalDecodeError::TrailingBytes`, so one byte sequence cannot
  decode to two distinct meanings.
- Truncated or malformed inputs are rejected with
  `CanonicalDecodeError::Malformed`. There is no partial or
  best-effort decode path.

## Golden fixtures

- `fixtures/intent_v1_golden.postcard` — canonical bytes of the MVP
  fixture intent defined in `encoding::tests::fixture_intent`.

The golden fixture is regenerated with:

```
cargo test -p oracleguard-schemas -- --ignored regenerate
```

The CI test `encoded_bytes_match_golden` fails if a change to the
schema, the serde derivations, or the pinned postcard version would
alter the bytes. That failure is the correct signal that a
canonical-byte change is about to ship and must be treated as a
schema-version boundary crossing.

## Version boundary

### What v1 covers

- Field set: `intent_version`, `policy_ref`, `allocation_id`,
  `requester_id`, `oracle_fact`, `oracle_provenance`,
  `requested_amount_lovelace`, `destination`, `asset`.
- Version field: `intent_version: u16 = 1` (pinned as
  `INTENT_VERSION_V1`).
- Sanctioned constructor: `DisbursementIntentV1::new_v1` — hard-codes
  the version so callers cannot silently emit an off-version intent.
- Validator: `validate_intent_version` — accepts only
  `INTENT_VERSION_V1` and returns `UnsupportedVersionError` otherwise,
  so a wrong-version intent is a typed error, never a silent
  pass-through.
- Canonical bytes: pinned by `fixtures/intent_v1_golden.postcard`. The
  `encoded_bytes_match_golden` test fails on drift.

### What a v2 must do

Any of the following is a canonical-byte change and therefore a
version-boundary crossing:

- adding, removing, or reordering a field in `DisbursementIntentV1`;
- changing the width or type of a fixed-width field;
- changing the serde representation attributes;
- bumping the pinned postcard major version;
- replacing postcard with a different encoder.

A v2 must:

- introduce a new struct `DisbursementIntentV2` alongside v1 rather
  than mutating v1;
- pick a new domain separator (e.g. `ORACLEGUARD_DISBURSEMENT_V2`) so
  v1 and v2 intent-ids cannot collide;
- add its own golden fixture and round-trip tests;
- extend `validate_intent_version` with an explicit compatibility rule.

The v1 surface — type, encoding, golden bytes, domain separator — is
frozen until a v2 crosses this boundary deliberately.
