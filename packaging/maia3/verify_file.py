#!/usr/bin/env python3
"""Verify one immutable release input by byte count and SHA-256."""

from __future__ import annotations

import argparse
import hashlib
import re
from pathlib import Path


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("path", type=Path)
    parser.add_argument("--bytes", type=int, required=True)
    parser.add_argument("--sha256", required=True)
    args = parser.parse_args()
    if args.bytes <= 0:
        parser.error("--bytes must be positive")
    if re.fullmatch(r"[0-9a-f]{64}", args.sha256) is None:
        parser.error("--sha256 must be lowercase hexadecimal")
    if not args.path.is_file():
        parser.error("path is not a file")
    actual_bytes = args.path.stat().st_size
    if actual_bytes != args.bytes:
        raise SystemExit(f"size mismatch: expected {args.bytes}, found {actual_bytes}")
    actual_digest = digest(args.path)
    if actual_digest != args.sha256:
        raise SystemExit(f"SHA-256 mismatch: expected {args.sha256}, found {actual_digest}")
    print(f"Verified {args.path}: {actual_bytes} bytes, SHA-256 {actual_digest}")


if __name__ == "__main__":
    main()
