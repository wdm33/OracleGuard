# Semantic Identity of a Disbursement Intent

This document pins which fields of `DisbursementIntentV1` participate
in intent identity and which do not. The distinction is enforced
structurally: `oracleguard_schemas::encoding::intent_id` hashes only
the identity projection, never the full intent.

## Rule

> Two intents with the same identity-bearing fields have the same
> `intent_id`, regardless of their provenance metadata.

## Fields that participate in identity

Any change to a field below changes `intent_id` and is therefore a
semantic change. Tests in `encoding::tests` prove this for each field.

| Field | Why it is part of identity |
|---|---|
| `intent_version` | Schema version is load-bearing; v1 and v2 must never share an identity. |
| `policy_ref` | Different governing policies must never collide. |
| `allocation_id` | Different allocations must never collide. |
| `requester_id` | Different subjects must never collide. |
| `oracle_fact.asset_pair` | Different asset pairs change the evaluation domain. |
| `oracle_fact.price_microusd` | The authoritative price feeds the release-cap math. |
| `oracle_fact.source` | Different oracle sources are different evaluations. |
| `requested_amount_lovelace` | Amount change is a request change. |
| `destination` | Different payees are different disbursements. |
| `asset` | Different assets are different disbursements. |

## Fields that do NOT participate in identity

Any change to a field below leaves `intent_id` unchanged. Tests in
`encoding::tests` prove this for each field.

| Field | Why it is excluded |
|---|---|
| `oracle_provenance.timestamp_unix` | Fetch-side metadata, audit-only. |
| `oracle_provenance.expiry_unix` | Freshness-rejection input, never a release-cap input. |
| `oracle_provenance.aggregator_utxo_ref` | Pointer to the on-chain source UTxO, audit-only. |

## Domain separator

`intent_id` is BLAKE3 keyed with the context string
`ORACLEGUARD_DISBURSEMENT_V1`. This separator is distinct from every
Ziranity tag (`ZIRANITY_INTENT_ID_V1`, `ZIRANITY_VERIFICATION_BUNDLE_V1`,
`ZIRANITY_REPLAY_GOLDEN_V1`) so that OracleGuard intent identity cannot
collide with any other domain's hash even if the hashed bytes happen
to coincide.

## Where this matters

- The adapter submits intents keyed by `intent_id` — replay protection
  depends on identity stability.
- Evaluator decisions are recorded against the `intent_id` that was
  evaluated.
- The offline verifier re-computes `intent_id` from the canonical
  bytes in an evidence bundle to confirm that the recorded decision
  applies to the intent the bundle claims.
