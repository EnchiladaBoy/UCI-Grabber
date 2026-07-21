#!/usr/bin/env python3
"""Verify the tag-bound Maia3 direct-retrieval release review."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parent
REPOSITORY_ROOT = ROOT.parents[1]
HEX_64 = re.compile(r"^[0-9a-f]{64}$")
RELEASE_TAG = "v0.2.0"
RELEASE_REVIEW_PATH = ROOT / "release-review.json"
COMPONENT_METADATA_PATH = "packaging/maia3/component-metadata.json"
DIRECT_DOWNLOADS_PATH = "packaging/maia3/direct-downloads.json"

# These are the files that can alter what the signed Maia recipe downloads,
# how the private local package is assembled, or which Maia-related bytes the
# GitHub release publishes.  release-review.json is intentionally excluded so
# it can carry the resulting digest without a self-reference.
RELEASE_INPUTS = (
    ".gitattributes",
    ".github/workflows/release.yml",
    "Cargo.lock",
    "Cargo.toml",
    "LICENSE",
    "README.md",
    "docs/assets/uci-grabber-hero.webp",
    "catalog/catalog-public-key.pem",
    "catalog/catalog.json",
    "catalog/catalog.pub",
    "catalog/catalog.sig",
    "catalog/generate_catalog.py",
    "catalog/schema/catalog-v1.schema.json",
    "catalog/schema/recipe-v1.schema.json",
    "catalog/validate_catalog.py",
    "catalog/verify_catalog.py",
    "packaging/app/package_app.py",
    "packaging/licenses/about.hbs",
    "packaging/licenses/about.toml",
    "packaging/maia3/DIRECT-DOWNLOAD-NOTICES.txt",
    "packaging/maia3/component-metadata.json",
    "packaging/maia3/direct-downloads.json",
    "packaging/maia3/maia3_entry.py",
    "packaging/maia3/package_launcher.py",
    "packaging/maia3/review_digest.py",
    "packaging/maia3/smoke_fake_python.rs",
    "packaging/maia3/smoke_personalized_launcher.py",
    "packaging/maia3/validate_metadata.py",
    "packaging/verify_macos_deployment.py",
    "src/app.rs",
    "src/bin/uci-grabber-gui.rs",
    "src/bin/uci-grabber-maia3-launcher.rs",
    "src/catalog.rs",
    "src/download.rs",
    "src/error.rs",
    "src/extract.rs",
    "src/handoff.rs",
    "src/install.rs",
    "src/lib.rs",
    "src/main.rs",
    "src/recipes.rs",
    "src/registry.rs",
    "src/schema.rs",
    "src/uci.rs",
)

CHECKPOINT_DISTRIBUTION_REVIEW = {
    "decision": "approved_direct_user_retrieval_only",
    "delivery": "end_user_download_from_pinned_hugging_face_revision",
    "github_release_distributes_checkpoints": False,
    "upstream_facts": {
        "maia3-5m": {
            "model_card_statement": (
                "CC BY 4.0 applies to the paper; see the Maia3 repository for the "
                "code and weights license"
            ),
            "independent_license_file": False,
        },
        "maia3-23m": {
            "model_card_statement": (
                "CC BY 4.0 applies to the paper; see the Maia3 repository for the "
                "code and weights license"
            ),
            "independent_license_file": False,
        },
        "maia3-79m": {"model_card_statement": "AGPLv3"},
        "maia3-code": {
            "repository": "https://github.com/CSSLab/maia3",
            "license_statement": "AGPL-3.0",
        },
    },
    "relicenses_upstream_material": False,
}

DIRECT_RETRIEVAL_REVIEW = {
    "decision": "approved_direct_user_retrieval_and_local_assembly",
    "github_release_distributes": {
        "maia3_code": False,
        "cpython": False,
        "python_dependencies": False,
        "maia_checkpoints": False,
    },
    "github_release_maia_assets": [
        "MAIA3-DIRECT-DOWNLOAD-NOTICES.txt",
        "uci-grabber-maia3-launcher-linux-aarch64.tar.gz",
        "uci-grabber-maia3-launcher-linux-x86_64.tar.gz",
        "uci-grabber-maia3-launcher-macos-aarch64.tar.gz",
        "uci-grabber-maia3-launcher-windows-x86_64.zip",
    ],
    "retrieval": "user_initiated_from_immutable_upstream_urls",
    "assembly": "private_local_portable_package",
    "launcher_relationship": "apache_2_0_process_launcher_invokes_local_python",
    "repackages_upstream_material": False,
    "not_legal_advice": True,
}

RELEASE_REVIEW_KEYS = {
    "schema",
    "release_tag",
    "component_metadata",
    "direct_downloads",
    "checkpoint_distribution_review",
    "direct_retrieval_review",
    "release_inputs_sha256",
}


def file_digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def strict_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"JSON object contains duplicate key: {key}")
        result[key] = value
    return result


def reject_json_constant(value: str) -> None:
    raise ValueError(f"JSON contains non-standard numeric constant: {value}")


def load_strict_json(path: Path, label: str) -> Any:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} is missing or is not a regular file: {path}")
    return json.loads(
        path.read_text(encoding="utf-8"),
        object_pairs_hook=strict_object,
        parse_constant=reject_json_constant,
    )


def require_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise ValueError(f"{label} must be a JSON object")
    actual_keys = set(value)
    if actual_keys != keys:
        raise ValueError(
            f"{label} has invalid keys; missing={sorted(keys - actual_keys)}, "
            f"unexpected={sorted(actual_keys - keys)}"
        )
    return value


def require_literal(value: Any, expected: Any, label: str) -> None:
    if type(expected) is dict:
        actual = require_object(value, set(expected), label)
        for key, child in expected.items():
            require_literal(actual[key], child, f"{label}.{key}")
        return
    if type(expected) is list:
        if type(value) is not list or len(value) != len(expected):
            raise ValueError(f"{label} must be the exact reviewed list")
        for index, (actual, child) in enumerate(zip(value, expected)):
            require_literal(actual, child, f"{label}[{index}]")
        return
    if type(value) is not type(expected) or value != expected:
        raise ValueError(f"{label} must be the reviewed value {expected!r}")


def require_sha256(value: Any, label: str) -> str:
    if type(value) is not str or HEX_64.fullmatch(value) is None:
        raise ValueError(f"{label} is missing or is not lowercase SHA-256")
    return value


def verify_expected(actual: str, expected: str, label: str) -> None:
    require_sha256(expected, f"{label} review digest")
    if actual != expected:
        raise ValueError(f"{label} review is stale: expected {expected}, resolved {actual}")
    print(f"Verified reviewed {label}: SHA-256 {actual}")


def release_inputs_digest() -> str:
    result = hashlib.sha256(b"uci-grabber-maia3-direct-release-inputs-v1\0")
    for relative in RELEASE_INPUTS:
        path = REPOSITORY_ROOT / relative
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"release-review input is missing: {relative}")
        result.update(relative.encode("utf-8"))
        result.update(b"\0")
        result.update(bytes.fromhex(file_digest(path)))
    return result.hexdigest()


def verify_reviewed_file(value: Any, expected_path: str, label: str) -> None:
    reviewed = require_object(value, {"path", "sha256"}, label)
    require_literal(reviewed["path"], expected_path, f"{label}.path")
    expected = require_sha256(reviewed["sha256"], f"{label}.sha256")
    path = REPOSITORY_ROOT / expected_path
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} is missing or is not a regular file")
    verify_expected(file_digest(path), expected, label)


def verify_release(tag: str, review_path: Path = RELEASE_REVIEW_PATH) -> None:
    require_literal(tag, RELEASE_TAG, "requested release tag")
    review = require_object(
        load_strict_json(review_path, "release review"),
        RELEASE_REVIEW_KEYS,
        "release review",
    )
    require_literal(review["schema"], 2, "release review.schema")
    require_literal(review["release_tag"], RELEASE_TAG, "release review.release_tag")
    require_literal(review["release_tag"], tag, "release review.release_tag")
    verify_reviewed_file(
        review["component_metadata"],
        COMPONENT_METADATA_PATH,
        "component metadata",
    )
    verify_reviewed_file(
        review["direct_downloads"],
        DIRECT_DOWNLOADS_PATH,
        "direct-download manifest",
    )
    require_literal(
        review["checkpoint_distribution_review"],
        CHECKPOINT_DISTRIBUTION_REVIEW,
        "release review.checkpoint_distribution_review",
    )
    require_literal(
        review["direct_retrieval_review"],
        DIRECT_RETRIEVAL_REVIEW,
        "release review.direct_retrieval_review",
    )
    expected_inputs = require_sha256(
        review["release_inputs_sha256"], "release review.release_inputs_sha256"
    )
    verify_expected(release_inputs_digest(), expected_inputs, "direct-release inputs")

    # The manifest validator performs the structural, allow-list, candidate
    # inventory, and destination checks without touching the network.
    from validate_metadata import validate

    validate()
    print(f"Verified fail-closed Maia3 direct-retrieval review for {tag}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("inputs", help="print the reviewed release-input digest")
    verify_inputs = subparsers.add_parser(
        "verify-inputs", help="require the reviewed release-input digest"
    )
    verify_inputs.add_argument("--expected", required=True)
    verify_file = subparsers.add_parser("verify", help="require a reviewed file digest")
    verify_file.add_argument("path", type=Path)
    verify_file.add_argument("--expected", required=True)
    verify_file.add_argument("--label", required=True)
    verify_release_parser = subparsers.add_parser(
        "verify-release", help="verify the checked direct-retrieval release review"
    )
    verify_release_parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    if args.command == "inputs":
        print(release_inputs_digest())
    elif args.command == "verify-inputs":
        verify_expected(release_inputs_digest(), args.expected, "direct-release inputs")
    elif args.command == "verify-release":
        verify_release(args.tag)
    else:
        verify_expected(file_digest(args.path), args.expected, args.label)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Maia3 release review failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
