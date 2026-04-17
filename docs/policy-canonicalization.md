# Policy Canonicalization

This document specifies the canonical byte form used to derive
`policy_ref` for OracleGuard. It is the authoritative reference for
what bytes are hashed when OracleGuard talks about "policy identity".

## What gets canonicalized

The MVP policy is a JSON document. Its canonical bytes are produced by
`oracleguard_schemas::policy::canonicalize_policy_json`, which:

1. Parses the input as strict JSON.
2. Rejects any numeric value parsed as floating-point, including values
   that happen to be whole numbers when expressed with a decimal point
   or an exponent (e.g. `1.0` or `1e2`).
3. Recursively sorts every object's keys lexicographically by key bytes.
4. Emits the result with no insignificant whitespace.

String values and integer numbers retain their input representation.
Only object key order and whitespace are normalized.

## Why these rules

- **Sorted keys.** Two human-readable policies with the same content but
  different key order must produce identical canonical bytes. Editor
  conventions, diff tools, and merges must not change identity.
- **No whitespace.** Pretty-printing preferences must not change
  identity.
- **No floats.** The authoritative surface forbids floating-point
  arithmetic (`#![deny(clippy::float_arithmetic)]`). Float textual
  round-tripping is a long-standing source of non-determinism, and
  permitting floats in policy bytes would silently import that
  non-determinism into `policy_ref`.

## Inputs and outputs

- Input: the MVP fixture `fixtures/policy_v1.json` (pretty-printed for
  review).
- Canonical bytes: produced by `canonicalize_policy_json` at runtime.
- Golden fixture: `fixtures/policy_v1.canonical.bytes` pins those bytes
  so CI detects any drift in the canonicalization.

The golden fixture is the single byte sequence a reader of the public
repo may feed into a hash to check `policy_ref` by hand.

## Relationship to `policy_ref`

Slice 02 of Cluster 2 defines `policy_ref = SHA-256(canonical_bytes)`.
This module is the only public path from a JSON policy document to the
bytes that feed that hash. No other OracleGuard crate may produce
`policy_ref`-bearing bytes by an independent route; the adapter and
verifier consume `policy_ref` values, they do not derive them.

## Versioning

The canonicalization rule above is the v1 surface. Any future change —
switching to CBOR, changing the key-sort ordering, admitting floats, or
anything else that could change the bytes fed to SHA-256 for the same
semantic policy — is a version boundary crossing. It requires a new
canonicalization module, a distinct `PolicyRef` derivation path, and an
explicit compatibility statement. The v1 surface is frozen until then.
