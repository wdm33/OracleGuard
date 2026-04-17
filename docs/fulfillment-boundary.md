# OracleGuard Fulfillment Boundary

This document is the single reviewer-facing explanation of how
OracleGuard executes an already-authorized disbursement on Cardano.
It is the Cluster 7 companion to `docs/authority-routing.md` (Cluster
6) and `docs/authorization-gates.md` (Cluster 5).

If the code ever disagrees with this document, the code is the source
of truth; update the document to match — never the other way around.

## 1. Authorized-effect input surface

Settlement consumes exactly one typed input:
[`oracleguard_schemas::effect::AuthorizedEffectV1`]. The struct is
canonical (fixed-width, postcard-round-tripped, golden-fixture-pinned)
and is the only execution-command surface the public repo recognizes.

A receipt from Ziranity consensus carries the effect inside
`IntentReceiptV1.output == AuthorizationResult::Authorized { effect }`.
Fulfillment treats the embedded effect as already-authoritative — it
runs no gate, no evaluator, and no destination or subject recheck.

A denied receipt carries `AuthorizationResult::Denied { reason, gate }`
instead. Fulfillment must not execute a Cardano transaction for any
denied receipt; see Section 4.

## 2. Exact field copy-through

The fulfillment mapping is pure:

| `AuthorizedEffectV1` field     | Cardano transaction parameter |
|--------------------------------|-------------------------------|
| `destination`                  | Output address (byte-identical, including `length`) |
| `asset`                        | Output asset (byte-identical `policy_id`, `asset_name`, `asset_name_len`) |
| `authorized_amount_lovelace`   | Output amount in lovelace |

`policy_ref`, `allocation_id`, `requester_id`, and
`release_cap_basis_points` are authorization-provenance fields. They
MUST NOT appear in the Cardano transaction output. They are available
to the evidence bundle (Cluster 8), never to the settlement path.

The mapping is materialized by
`oracleguard_adapter::settlement::settlement_request_from_effect`. The
function is BLUE-pure by construction: no I/O, no math, no
normalization. Integration tests in
`crates/oracleguard-adapter/tests/fulfillment_boundary.rs` sweep
destination, asset, and amount mutations and assert byte-identical
copy-through on every case.

### Hard prohibitions

- No operator-edited effect fields. If the operator wants a different
  amount, they submit a new intent — the authorization boundary
  decides, not the fulfillment shell.
- No silent address normalization (bech32 canonicalization,
  payload-length trimming, etc.). Bytes go through verbatim.
- No silent asset conversion. See Section 3.
- No wallet-side "helpful" substitution of destination, amount, or
  asset.

## 3. ADA-only MVP asset guard

For the hackathon MVP, the fulfillment path supports exactly one
asset: ADA, represented by the canonical
`AssetIdV1::ADA = { policy_id: [0; 28], asset_name: [0; 32],
asset_name_len: 0 }`.

`oracleguard_adapter::settlement::guard_mvp_asset` returns
`Err(FulfillmentRejection::NonAdaAsset)` for any `AuthorizedEffectV1`
whose `asset` is not byte-identical to `AssetIdV1::ADA`. The
fulfillment entrypoint short-circuits on this rejection without
touching the settlement backend.

Multi-asset support is a **canonical-byte version-boundary event**.
It requires updating the registry gate's asset-matching rule, the
fulfillment guard, and the evidence verifier in lockstep, and is out
of scope for the MVP.

## 4. Deny / no-transaction closure

`oracleguard_adapter::settlement::fulfill` is the single public entry
point that consumes an `IntentReceiptV1` and a `SettlementBackend`.
Its branch structure is the mechanical no-tx closure:

```
match receipt.output {
    Denied { reason, gate }       -> DeniedUpstream { reason, gate }    // no backend call
    Authorized { effect } if Pending receipt -> RejectedAtFulfillment { ReceiptNotCommitted }
    Authorized { effect } if non-ADA         -> RejectedAtFulfillment { NonAdaAsset }
    Authorized { effect } otherwise           -> backend.submit(request) -> Settled { tx_hash }
}
```

The `backend` reference is **only reachable** from the last branch.
A structural audit test in `settlement.rs` guarantees the production
source does not call `authorize_disbursement`, `run_anchor_gate`,
`run_registry_gate`, `run_grant_gate`, or `evaluate_disbursement`.

### Hard prohibitions

- No transaction on deny.
- No "audit tx" or side-effectful Cardano action on deny in the MVP
  path.
- No retry with a reduced amount. The authorization decision is final;
  corrections are new intents.

## 5. Transaction-hash capture contract

The allow-path produces a typed settlement reference:

```rust
pub struct CardanoTxHashV1(pub [u8; 32]);
```

A 32-byte BLAKE2b-256 Cardano transaction id, fixed-width and
byte-identical. The `SettlementBackend::submit` trait is the
hash-capture boundary: production uses
`CardanoCliSettlementBackend` (shell) and tests use deterministic
fixture backends (no-mocks compliant: seeded substitute, not a stub
of the functional core).

The tx hash is the single reference Cluster 8 (Evidence Bundle and
Offline Verification) will record for an allow-path outcome. It is
captured at submission time; block-finality observation and explorer
integration are out of scope for this cluster.

## 6. Summary of public surface

All names below live in `oracleguard-adapter::settlement` unless
otherwise noted.

| Symbol                                   | Role                                  |
|------------------------------------------|---------------------------------------|
| `AuthorizedEffectV1` *(schemas)*         | Only typed execution-command input    |
| `SettlementRequest`                      | Exact field-copy wrapper              |
| `settlement_request_from_effect`         | Pure mapping                          |
| `FulfillmentRejection`                   | Fulfillment-side rejection kinds      |
| `FulfillmentOutcome`                     | Typed execution-outcome surface       |
| `SettlementBackend` (trait)              | Submission boundary                   |
| `CardanoTxHashV1`                        | 32-byte settlement reference          |
| `CardanoCliSettlementBackend`            | RED shell submission implementation   |
| `fulfill`                                | Single public fulfillment entry point |

Evidence (Cluster 8) consumes the `FulfillmentOutcome` surface.
Nothing in the evidence layer redefines or re-derives the authorized
effect, the settlement request, the ADA guard, the deny branch, or
the tx hash.
