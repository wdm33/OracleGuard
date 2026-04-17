# Authority Routing

This document defines the public routing boundary for OracleGuard
disbursement intents in the MVP. It is the authoritative reference for
the fixed disbursement authority OCU identifier and the single-authority
routing rule.

If this document and the code in `oracleguard-schemas::routing` ever
disagree, the code is the source of truth; update this document to
match.

## Primary invariant

> Every `DisbursementIntentV1` routes to exactly one authoritative OCU
> in the MVP. The target does not depend on destination, oracle source,
> payout request, or any other dynamic field.

Routing is domain-based, not data-driven. Future sharding (by treasury,
policy namespace, or any other axis) is explicitly out of scope for v1
and would be a canonical-byte version-boundary change.

## Fixed disbursement OCU identifier

The well-known disbursement authority OCU is a fixed 32-byte identifier:

```
a465606bb1b1cd4a7014550431 1b673b400e6c2dd2b0d0a83c9f46c2c11867c4
```

(Without spaces, the hex fixture is at
`fixtures/routing/disbursement_ocu_id.hex`.)

The Rust constant is
`oracleguard_schemas::routing::DISBURSEMENT_OCU_ID` and is referenced
verbatim by:

- the public routing rule (`oracleguard_schemas::routing::route`)
- the adapter CLI submission wrapper
  (`oracleguard_adapter::ziranity_submit` — lands in Cluster 6 Slice 04)
- the private Ziranity runtime binding for the disbursement domain
- the offline verifier, when it replays a bundle and checks that the
  bundle's declared authority matches the OCU canonicalized here

### Derivation recipe

The identifier is derived deterministically as a BLAKE3 keyed hash:

| Parameter | Value |
|-----------|-------|
| BLAKE3 derive-key domain | `ORACLEGUARD_DISBURSEMENT_OCU_V1` |
| Input seed | `OCU/MVP/v1` (ASCII) |
| Output | 32-byte BLAKE3 hash |

`oracleguard_schemas::routing::derive_disbursement_ocu_id()` performs
this derivation. The baked literal constant is pinned to the derivation
by a non-ignored test (`ocu_id_matches_documented_derivation`); if the
literal or the recipe drifts, CI fails.

### Why a fixed constant

1. **Legibility.** Reviewers and judges see one authority target, not a
   dispatch table or dynamic derivation.
2. **Replay safety.** A constant is trivially stable across runs and
   across reconstructions of the adapter.
3. **Domain separation.** The OCU-identity domain string is distinct
   from the intent-identity domain
   (`ORACLEGUARD_DISBURSEMENT_V1`, see
   `oracleguard_schemas::encoding::ORACLEGUARD_DISBURSEMENT_DOMAIN`), so
   the two 32-byte spaces cannot collide even on coincident inputs.

### What the constant is NOT

- **Not a policy reference.** Policy identity lives on the
  `DisbursementIntentV1.policy_ref` field and is validated by the
  anchor gate (`oracleguard_policy::authorize::run_anchor_gate`).
- **Not a destination routing key.** Destination authorization is
  enforced by the registry gate against the allocation's
  `approved_destinations` set.
- **Not a sharding discriminator.** There is no per-treasury,
  per-asset, or per-requester dispatch in the MVP. All MVP disbursement
  intents land at the same OCU.

## Routing rule

The public routing function is:

```rust
pub fn route(intent: &DisbursementIntentV1) -> [u8; 32];
```

It always returns `DISBURSEMENT_OCU_ID`. The intent is taken by
reference so call-sites read as "this intent routes to …", but the
body deliberately does not inspect its fields: routing is
domain-based, not data-driven.

Cluster 6 Slice 02 extends the test coverage to prove routing is
invariant under mutation of every non-authority field (destination,
oracle source, provenance, requested amount, asset, requester id).

## What routing does NOT decide

- Whether the intent is authorized. That is the authorization
  closure's job (`authorize_disbursement`, Cluster 5).
- Which envelope or signing scheme the Ziranity CLI uses. That is a
  private Ziranity responsibility.
- Which network transport is used. Also private Ziranity.
- Which Cardano wallet or key signs the eventual settlement
  transaction. That is the Cardano settlement shell's concern and is
  downstream of authorized-effect consumption.

## Change policy

Adding, removing, or renaming any of the following is a canonical-byte
event and therefore a version-boundary change:

- `DISBURSEMENT_OCU_DOMAIN`
- `DISBURSEMENT_OCU_SEED`
- `DISBURSEMENT_OCU_ID`
- the return type or signature of `route`

Such changes require Katiba sign-off and cannot land silently on the
`v1` boundary.
