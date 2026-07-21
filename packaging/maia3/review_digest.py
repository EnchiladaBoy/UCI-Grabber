#!/usr/bin/env python3
"""Compute and enforce digest-bound Maia3 release reviews."""

from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
HEX_64 = re.compile(r"^[0-9a-f]{64}$")
SOURCE_REVIEW_FILES = (
    "build_runtime.py",
    "build_wheel_lock.py",
    "component-metadata.json",
    "corresponding-source-policy.json",
    "maia3.spec",
    "maia3_entry.py",
    "make_source_bundle.py",
    "package_runtime.py",
    "requirements-direct.txt",
    "review_digest.py",
    "smoke_runtime.py",
    "validate_metadata.py",
    "verify_file.py",
)
WHEELHOUSE_PLATFORMS = (
    "windows-x86_64",
    "macos-aarch64",
    "linux-x86_64",
    "linux-aarch64",
)


def file_digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def source_review_digest() -> str:
    result = hashlib.sha256(b"uci-grabber-maia3-source-review-v1\0")
    for relative in SOURCE_REVIEW_FILES:
        path = ROOT / relative
        if not path.is_file():
            raise ValueError(f"source-review input is missing: {relative}")
        result.update(relative.encode("utf-8"))
        result.update(b"\0")
        result.update(bytes.fromhex(file_digest(path)))
    return result.hexdigest()


def parse_wheelhouses(values: list[str]) -> dict[str, str]:
    result: dict[str, str] = {}
    for value in values:
        platform, separator, digest = value.partition("=")
        if not separator or platform not in WHEELHOUSE_PLATFORMS:
            raise ValueError(f"invalid reviewed wheelhouse assignment: {value}")
        if platform in result:
            raise ValueError(f"duplicate reviewed wheelhouse assignment: {platform}")
        if HEX_64.fullmatch(digest) is None:
            raise ValueError(f"{platform} wheelhouse review digest is missing or invalid")
        result[platform] = digest
    if set(result) != set(WHEELHOUSE_PLATFORMS):
        missing = sorted(set(WHEELHOUSE_PLATFORMS) - set(result))
        raise ValueError(f"reviewed wheelhouse digest set is incomplete: {missing}")
    return result


def source_release_review_digest(wheelhouses: dict[str, str]) -> str:
    if set(wheelhouses) != set(WHEELHOUSE_PLATFORMS):
        raise ValueError("source release review requires all four wheelhouse digests")
    result = hashlib.sha256(b"uci-grabber-maia3-source-release-review-v2\0")
    result.update(bytes.fromhex(source_review_digest()))
    for platform in WHEELHOUSE_PLATFORMS:
        digest = wheelhouses[platform]
        if HEX_64.fullmatch(digest) is None:
            raise ValueError(f"{platform} wheelhouse review digest is missing or invalid")
        result.update(platform.encode("ascii"))
        result.update(b"\0")
        result.update(bytes.fromhex(digest))
    return result.hexdigest()


def verify_expected(actual: str, expected: str, label: str) -> None:
    if HEX_64.fullmatch(expected) is None:
        raise ValueError(f"{label} review digest is missing or is not lowercase SHA-256")
    if actual != expected:
        raise ValueError(f"{label} review is stale: expected {expected}, resolved {actual}")
    print(f"Verified reviewed {label}: SHA-256 {actual}")


def verify(path: Path, expected: str, label: str) -> None:
    verify_expected(file_digest(path), expected, label)


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("source", help="print the complete source-review input-set digest")
    release_parser = subparsers.add_parser(
        "source-release", help="bind the source review to all reviewed native wheelhouses"
    )
    release_parser.add_argument("--wheelhouse", action="append", required=True)
    source_parser = subparsers.add_parser(
        "verify-source", help="require the reviewed source input-set digest"
    )
    source_parser.add_argument("--expected", required=True)
    source_parser.add_argument("--wheelhouse", action="append", required=True)
    verify_parser = subparsers.add_parser("verify", help="require a reviewed file digest")
    verify_parser.add_argument("path", type=Path)
    verify_parser.add_argument("--expected", required=True)
    verify_parser.add_argument("--label", required=True)
    args = parser.parse_args()
    if args.command == "source":
        print(source_review_digest())
    elif args.command == "source-release":
        print(source_release_review_digest(parse_wheelhouses(args.wheelhouse)))
    elif args.command == "verify-source":
        verify_expected(
            source_release_review_digest(parse_wheelhouses(args.wheelhouse)),
            args.expected,
            "corresponding-source policy and reviewed wheelhouses",
        )
    else:
        verify(args.path, args.expected, args.label)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(f"Maia3 release review failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
