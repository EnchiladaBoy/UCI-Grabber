#!/usr/bin/env python3
"""Create a hash-enforced pip lock and provenance inventory from a wheelhouse."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


PLATFORMS = (
    "windows-x86_64",
    "macos-aarch64",
    "linux-x86_64",
    "linux-aarch64",
)
PYTHON_RUNTIME = "CPython 3.12.10"


def sha256(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--wheelhouse", type=Path, required=True)
    parser.add_argument("--requirements", type=Path, required=True)
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    args = parser.parse_args()
    wheels = sorted(args.wheelhouse.glob("*.whl"))
    if not wheels:
        parser.error("wheelhouse contains no wheels")
    extras = sorted(path.name for path in args.wheelhouse.iterdir() if path.suffix != ".whl")
    if extras:
        parser.error(f"wheelhouse contains non-wheel inputs: {extras}")

    by_distribution: dict[str, tuple[str, list[str]]] = {}
    inventory_files = []
    for wheel in wheels:
        parts = wheel.name[:-4].split("-")
        if len(parts) < 5:
            parser.error(f"invalid wheel filename: {wheel.name}")
        name = parts[0].replace("_", "-").replace(".", "-").lower()
        version = parts[1]
        digest = sha256(wheel)
        prior_version, hashes = by_distribution.get(name, (version, []))
        if prior_version != version:
            parser.error(f"multiple versions of {name} in wheelhouse")
        hashes.append(digest)
        by_distribution[name] = (version, hashes)
        inventory_files.append(
            {
                "filename": wheel.name,
                "bytes": wheel.stat().st_size,
                "sha256": digest,
                "tags": ["-".join(parts[-3:])],
            }
        )

    lines = []
    for name, (version, hashes) in sorted(by_distribution.items()):
        joined = " ".join(f"--hash=sha256:{digest}" for digest in sorted(hashes))
        lines.append(f"{name}=={version} {joined}")
    args.requirements.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    inventory = {
        "schema": 2,
        "platform": args.platform,
        "python": PYTHON_RUNTIME,
        "files": inventory_files,
    }
    args.inventory.write_text(
        json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    main()
