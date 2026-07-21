#!/usr/bin/env python3
"""Reject Mach-O inputs whose declared minimum macOS exceeds the release floor."""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path


MACHO_MAGICS = {
    b"\xca\xfe\xba\xbe",
    b"\xca\xfe\xba\xbf",
    b"\xce\xfa\xed\xfe",
    b"\xcf\xfa\xed\xfe",
    b"\xbe\xba\xfe\xca",
    b"\xbf\xba\xfe\xca",
    b"\xfe\xed\xfa\xce",
    b"\xfe\xed\xfa\xcf",
}
VERSION = re.compile(r"^(?:minos|version)\s+([0-9]+(?:\.[0-9]+){0,2})$")


def version_tuple(value: str) -> tuple[int, int, int]:
    parts = [int(part) for part in value.split(".")]
    if len(parts) > 3:
        raise ValueError(f"invalid macOS version: {value}")
    return tuple((parts + [0, 0])[:3])  # type: ignore[return-value]


def deployment_versions(load_commands: str) -> list[str]:
    versions: list[str] = []
    wanted: str | None = None
    for raw_line in load_commands.splitlines():
        line = raw_line.strip()
        if line == "cmd LC_BUILD_VERSION":
            wanted = "minos"
            continue
        if line == "cmd LC_VERSION_MIN_MACOSX":
            wanted = "version"
            continue
        if line.startswith("cmd "):
            wanted = None
            continue
        match = VERSION.fullmatch(line)
        if wanted is not None and match is not None and line.startswith(wanted):
            versions.append(match.group(1))
            wanted = None
    return versions


def is_macho(path: Path) -> bool:
    try:
        with path.open("rb") as handle:
            return handle.read(4) in MACHO_MAGICS
    except OSError:
        return False


def files_under(paths: list[Path]) -> list[Path]:
    files: list[Path] = []
    for path in paths:
        if path.is_file():
            files.append(path)
        elif path.is_dir():
            files.extend(candidate for candidate in path.rglob("*") if candidate.is_file())
        else:
            raise ValueError(f"deployment-check input does not exist: {path}")
    return sorted(set(files))


def verify(paths: list[Path], maximum: str) -> int:
    ceiling = version_tuple(maximum)
    inspected = 0
    violations: list[str] = []
    for path in files_under(paths):
        if not is_macho(path):
            continue
        inspected += 1
        result = subprocess.run(
            ["otool", "-l", str(path)],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        versions = deployment_versions(result.stdout)
        if not versions:
            violations.append(f"{path}: no macOS deployment command")
            continue
        too_new = sorted({value for value in versions if version_tuple(value) > ceiling})
        if too_new:
            violations.append(f"{path}: {', '.join(too_new)} > {maximum}")
    if inspected == 0:
        raise ValueError("deployment check found no Mach-O files")
    if violations:
        raise ValueError("macOS deployment floor violations:\n" + "\n".join(violations))
    return inspected


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("paths", type=Path, nargs="+")
    parser.add_argument("--maximum", default="12.3")
    args = parser.parse_args()
    inspected = verify(args.paths, args.maximum)
    print(f"Verified {inspected} Mach-O files require macOS {args.maximum} or earlier.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        raise SystemExit(f"macOS deployment check failed: {error}") from error
