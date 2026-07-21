#!/usr/bin/env python3
"""Authenticate exact catalog bytes, then apply strict schema validation."""

from __future__ import annotations

import argparse
import base64
import binascii
import re
import subprocess
import sys
import tempfile
from pathlib import Path

from validate_catalog import load_and_validate


ROOT = Path(__file__).resolve().parent


def decode_signature(raw: bytes) -> bytes:
    if len(raw) == 64:
        return raw
    trimmed = raw.strip()
    if len(trimmed) == 128 and re.fullmatch(rb"[0-9A-Fa-f]{128}", trimmed):
        return bytes.fromhex(trimmed.decode("ascii"))
    try:
        decoded = base64.b64decode(trimmed, validate=True)
    except binascii.Error as error:
        raise ValueError("signature is not raw, hex, or standard base64 Ed25519 data") from error
    if len(decoded) != 64:
        raise ValueError("Ed25519 signature must decode to exactly 64 bytes")
    return decoded


def verify_signature(catalog: Path, signature: bytes, public_key: Path) -> None:
    # Close the file before OpenSSL opens it. Windows does not allow another
    # process to reopen a default NamedTemporaryFile while its handle is live.
    with tempfile.TemporaryDirectory(prefix="uci-grabber-catalog-") as directory:
        signature_path = Path(directory) / "catalog.sig"
        signature_path.write_bytes(signature)
        result = subprocess.run(
            [
                "openssl", "pkeyutl", "-verify", "-pubin", "-inkey", str(public_key),
                "-rawin", "-in", str(catalog), "-sigfile", str(signature_path),
            ],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    if result.returncode != 0:
        raise ValueError(f"catalog signature failed: {result.stderr.strip()}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--signature", type=Path, required=True)
    parser.add_argument("--public-key", type=Path, default=ROOT / "catalog-public-key.pem")
    parser.add_argument("--bootstrap", action="store_true")
    args = parser.parse_args()
    raw_signature = args.signature.read_bytes()
    if len(raw_signature) > 4096:
        parser.error("signature exceeds 4 KiB")
    signature = decode_signature(raw_signature)
    # Untrusted JSON is intentionally not parsed before this succeeds.
    verify_signature(args.catalog, signature, args.public_key)
    load_and_validate(args.catalog, args.bootstrap)
    print("Catalog signature and schema are valid.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"Catalog verification failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
