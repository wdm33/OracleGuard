#!/usr/bin/env python3
"""OracleGuard Cardano disbursement settlement helper.

Builds, signs, and submits the on-chain disbursement transaction for
an OracleGuard authorized effect. Invoked by the Rust shell wrapper
at `oracleguard_adapter::cardano_disburse::PyCardanoDisburseBackend`.

The Rust side supplies the authorized-effect fields
(destination, amount, intent_id) as CLI arguments. This script does
the Cardano-specific work:

  1. Derive the pool wallet's signing key from POOL_MNEMONIC (BIP-44
     Shelley payment path m/1852'/1815'/0'/0/0).
  2. Query the pool wallet's UTxOs via Ogmios.
  3. Build a transaction: input = pool UTxO, output = destination
     receives the authorized amount, change = back to pool, fees
     auto-calculated.
  4. Attach the OracleGuard intent_id as tx metadata (label 674 —
     a common convention for attribution metadata).
  5. Sign with the pool signing key; submit via Ogmios.
  6. Print the 64-char lowercase hex tx_id to stdout on success.

On any failure, prints a human-readable error to stderr and exits
non-zero. The Rust caller surfaces stdout/stderr verbatim; this
script does not attempt to encode error details in its exit code
beyond "non-zero means failed."

Install (operator side, one-time):

    pip install pycardano ogmios

Usage (invoked by Rust — not typically run by hand):

    POOL_MNEMONIC="word1 word2 ... word24" \\
    python3 scripts/cardano_disburse.py \\
        --ogmios-url http://35.209.192.203:1337 \\
        --pool-address addr_test1qz4f2vac8nn7tp802mxj3cu40a7xhhzc3... \\
        --destination-bytes-hex 000ee03e45b05d6225eb7143d2be23a3b8... \\
        --destination-length 57 \\
        --amount-lovelace 700000000 \\
        --intent-id 77777777777777777777777777777777... # 64 hex chars
"""
import argparse
import os
import sys
from urllib.parse import urlparse


EXIT_OK = 0
EXIT_ARGS = 2
EXIT_MNEMONIC = 3
EXIT_IMPORT = 4
EXIT_DECODE = 5
EXIT_BUILD = 6
EXIT_SUBMIT = 7
EXIT_UNEXPECTED = 10


def _parse_args(argv):
    p = argparse.ArgumentParser(
        description="OracleGuard Cardano disbursement settlement helper",
    )
    p.add_argument(
        "--ogmios-url",
        required=True,
        help="Ogmios v6 HTTP endpoint (e.g. http://35.209.192.203:1337)",
    )
    p.add_argument(
        "--pool-address",
        required=True,
        help="Bech32 address of the pool wallet (tx_in source + change address)",
    )
    p.add_argument(
        "--destination-bytes-hex",
        required=True,
        help="CardanoAddressV1.bytes as hex (up to 114 chars = 57 bytes)",
    )
    p.add_argument(
        "--destination-length",
        type=int,
        required=True,
        help="CardanoAddressV1.length — semantic byte count (1..=57)",
    )
    p.add_argument(
        "--amount-lovelace",
        type=int,
        required=True,
        help="Authorized disbursement amount, in lovelace",
    )
    p.add_argument(
        "--intent-id",
        required=True,
        help="32-byte OracleGuard intent_id as hex — embedded in tx metadata",
    )
    return p.parse_args(argv)


def _parse_ogmios_url(url):
    parsed = urlparse(url)
    host = parsed.hostname
    if not host:
        raise ValueError(f"no host in ogmios url: {url!r}")
    secure = parsed.scheme in ("https", "wss")
    port = parsed.port or (443 if secure else 80)
    path = parsed.path or ""
    return host, port, path, secure


def _load_pycardano():
    """Import pycardano lazily so missing-install produces a helpful error."""
    try:
        from pycardano import (  # noqa: F401
            Address,
            AuxiliaryData,
            ExtendedSigningKey,
            HDWallet,
            Metadata,
            Network,
            PaymentExtendedSigningKey,
            TransactionBuilder,
            TransactionOutput,
        )
        from pycardano.backend.ogmios_v6 import OgmiosV6ChainContext

        return {
            "Address": Address,
            "AuxiliaryData": AuxiliaryData,
            "HDWallet": HDWallet,
            "Metadata": Metadata,
            "Network": Network,
            "PaymentExtendedSigningKey": PaymentExtendedSigningKey,
            "TransactionBuilder": TransactionBuilder,
            "TransactionOutput": TransactionOutput,
            "OgmiosV6ChainContext": OgmiosV6ChainContext,
        }
    except ImportError as e:
        raise RuntimeError(
            f"pycardano import failed: {e}\n"
            "Install the operator deps: pip install pycardano ogmios"
        ) from e


def _derive_pool_signing_key(mnemonic, pc):
    """Derive the Shelley payment signing key at m/1852'/1815'/0'/0/0."""
    hdwallet = pc["HDWallet"].from_mnemonic(mnemonic)
    payment_hd = hdwallet.derive_from_path("m/1852'/1815'/0'/0/0")
    return pc["PaymentExtendedSigningKey"].from_hdwallet(payment_hd)


def _build_destination(pc, dest_hex, dest_length):
    if dest_length < 1 or dest_length > 57:
        raise ValueError(
            f"destination length {dest_length} out of range 1..=57"
        )
    full = bytes.fromhex(dest_hex)
    if len(full) > 57:
        raise ValueError(
            f"destination bytes_hex decoded to {len(full)} bytes, max 57"
        )
    trimmed = full[:dest_length]
    return pc["Address"].from_primitive(trimmed)


def _build_and_submit(args, pc):
    host, port, path, secure = _parse_ogmios_url(args.ogmios_url)

    pool_addr = pc["Address"].from_primitive(args.pool_address)
    destination = _build_destination(pc, args.destination_bytes_hex, args.destination_length)

    context = pc["OgmiosV6ChainContext"](
        host=host,
        port=port,
        path=path,
        secure=secure,
        network=pc["Network"].TESTNET,
    )

    builder = pc["TransactionBuilder"](context)
    builder.add_input_address(pool_addr)
    builder.add_output(pc["TransactionOutput"](destination, args.amount_lovelace))

    # Embed the OracleGuard intent_id in tx metadata.
    # Label 674 is a common convention for general metadata; the
    # label choice is independent of OracleGuard's canonical bytes —
    # it exists purely for on-chain attribution and is not consulted
    # by the evaluator, the verifier, or consensus.
    builder.auxiliary_data = pc["AuxiliaryData"](
        data=pc["Metadata"]({674: {"oracleguard_intent_id": args.intent_id}})
    )

    signing_key = _derive_pool_signing_key(os.environ["POOL_MNEMONIC"], pc)
    signed_tx = builder.build_and_sign([signing_key], change_address=pool_addr)
    context.submit_tx(signed_tx)
    return signed_tx.id.payload.hex()


def main(argv=None):
    args = _parse_args(argv)

    mnemonic = os.environ.get("POOL_MNEMONIC")
    if not mnemonic:
        print("POOL_MNEMONIC env var not set", file=sys.stderr)
        return EXIT_MNEMONIC

    try:
        pc = _load_pycardano()
    except RuntimeError as e:
        print(str(e), file=sys.stderr)
        return EXIT_IMPORT

    try:
        tx_id_hex = _build_and_submit(args, pc)
    except ValueError as e:
        print(f"input decode error: {e}", file=sys.stderr)
        return EXIT_DECODE
    except Exception as e:  # noqa: BLE001 — surface anything pycardano throws
        # TransactionBuilder / OgmiosV6ChainContext raise a variety of
        # pycardano/ogmios exception types on build and submit failures.
        # Catching broadly here and surfacing verbatim on stderr gives
        # the operator the full diagnostic without this script
        # interpreting the failure.
        print(f"build/submit failed: {type(e).__name__}: {e}", file=sys.stderr)
        # Can't distinguish build vs submit without tighter types; EXIT_BUILD
        # covers both for the Rust caller (which surfaces stderr anyway).
        return EXIT_BUILD

    # Output the tx_id (64 lowercase hex chars, trailing newline from print)
    # so the Rust caller's parse_cli_txid_stdout accepts it verbatim.
    print(tx_id_hex)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
