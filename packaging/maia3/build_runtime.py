#!/usr/bin/env python3
"""Build and stage one native, model-free Maia3 component runtime."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import os
import platform
import shutil
import subprocess
import sys
import sysconfig
from pathlib import Path


ROOT = Path(__file__).resolve().parent
SPEC = ROOT / "maia3.spec"
EXPECTED_PYTHON = "3.12.10"
MACOS_DEPLOYMENT_TARGET = "12.3"
EXPECTED_DISTRIBUTIONS = {
    "chess": "1.11.2",
    "maia3": "0.1.0",
    "numpy": "2.2.6",
    "pyinstaller": "6.21.0",
    "torch": "2.11.0",
}
MODELS = ("maia3-5m", "maia3-23m", "maia3-79m")


def installed_version(name: str) -> str:
    try:
        return importlib.metadata.version(name)
    except importlib.metadata.PackageNotFoundError as error:
        raise SystemExit(f"required distribution is not installed: {name}") from error


def verify_build_environment() -> dict[str, str]:
    if platform.python_version() != EXPECTED_PYTHON:
        raise SystemExit(
            f"component builds require CPython {EXPECTED_PYTHON}; found {platform.python_version()}"
        )
    versions = {name: installed_version(name) for name in EXPECTED_DISTRIBUTIONS}
    for name, expected in EXPECTED_DISTRIBUTIONS.items():
        actual = versions[name]
        valid = actual in ({expected, f"{expected}+cpu"} if name == "torch" else {expected})
        if not valid:
            raise SystemExit(f"{name} must be {expected}; found {actual}")
    return versions


def license_inventory() -> str:
    lines = [
        "Python and packaged dependency notices",
        "======================================",
        "",
        "Generated from the exact environment in WHEELHOUSE.lock.json.",
        "The complete Maia3 AGPL text is carried separately.",
        "",
    ]
    for distribution in sorted(
        importlib.metadata.distributions(),
        key=lambda item: (item.metadata.get("Name") or "").lower(),
    ):
        name = distribution.metadata.get("Name") or "unknown"
        license_name = (
            distribution.metadata.get("License-Expression")
            or distribution.metadata.get("License")
            or "see included metadata/files"
        )
        lines.extend([f"--- {name} {distribution.version} ---", f"License: {license_name}", ""])
        seen: set[Path] = set()
        for relative in distribution.files or ():
            basename = Path(str(relative)).name.lower()
            if not any(token in basename for token in ("license", "copying", "notice")):
                continue
            located = Path(distribution.locate_file(relative))
            if located in seen or not located.is_file() or located.stat().st_size > 2 * 1024 * 1024:
                continue
            seen.add(located)
            contents = located.read_text(encoding="utf-8", errors="replace")
            lines.extend([f"[{relative}]", contents.rstrip(), ""])
    candidates = (
        Path(sys.base_prefix) / "LICENSE.txt",
        Path(sysconfig.get_paths()["data"]) / "LICENSE.txt",
    )
    for candidate in candidates:
        if candidate.is_file():
            lines.extend(["--- CPython license ---", candidate.read_text().rstrip(), ""])
            break
    else:
        raise SystemExit("could not locate the CPython license text")
    return "\n".join(lines).rstrip() + "\n"


def corresponding_source_notice(source_asset: str) -> str:
    return f"""Maia3 runtime source and licensing notice
==========================================

Maia3 code in this runtime is distributed under the GNU Affero General Public
License version 3. Packaged dependencies retain their respective licenses; see
PYTHON-THIRD-PARTY-LICENSES.txt and WHEELHOUSE.lock.json.

The same UCI Grabber GitHub release that provides this runtime archive also
provides the reviewed source/build archive below, without additional charge:

  {source_asset}

Open the GitHub release page from which this runtime was obtained and select
that exact filename. The signed UCI Grabber catalog's Source link points to the
same-tag asset directly.

The archive identifies the source materials and any offers selected by the
release review for this build. This notice is not a legal conclusion about the
classification of every packaged dependency.
"""


def write_macos_plist(app: Path, component_version: str) -> None:
    marketing = component_version.split("-", 1)[0]
    plist = f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>UCI Grabber Maia3 Runtime</string>
  <key>CFBundleExecutable</key><string>maia3-5m</string>
  <key>CFBundleIdentifier</key><string>io.github.enchiladaboy.ucigrabber.maia3</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>UCI-Grabber-Maia3</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{marketing}</string>
  <key>CFBundleVersion</key><string>{marketing}</string>
  <key>LSMinimumSystemVersion</key><string>{MACOS_DEPLOYMENT_TARGET}</string>
</dict></plist>
"""
    (app / "Contents" / "Info.plist").write_text(plist, encoding="utf-8", newline="\n")


def create_launchers(directory: Path, windows: bool) -> None:
    suffix = ".exe" if windows else ""
    source = directory / f"maia3-engine{suffix}"
    if not source.is_file():
        raise SystemExit(f"PyInstaller output is missing {source.name}")
    for model in MODELS:
        launcher = directory / f"{model}{suffix}"
        shutil.copy2(source, launcher)
        if not windows:
            launcher.chmod(0o755)
    source.unlink()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--platform",
        required=True,
        choices=("windows-x86_64", "macos-aarch64", "linux-x86_64", "linux-aarch64"),
    )
    parser.add_argument("--upstream", type=Path, required=True)
    parser.add_argument("--wheel-inventory", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--stage", type=Path, required=True)
    args = parser.parse_args()
    if args.platform == "macos-aarch64" and os.environ.get("MACOSX_DEPLOYMENT_TARGET") != MACOS_DEPLOYMENT_TARGET:
        raise SystemExit(
            f"macOS component builds require MACOSX_DEPLOYMENT_TARGET={MACOS_DEPLOYMENT_TARGET}"
        )
    versions = verify_build_environment()
    metadata = json.loads((ROOT / "component-metadata.json").read_text(encoding="utf-8"))
    if not (args.upstream / "maia3" / "uci.py").is_file() or not (args.upstream / "LICENSE").is_file():
        parser.error("upstream path is not a complete Maia3 checkout")
    if args.work.exists() or args.stage.exists():
        parser.error("work and stage paths must not already exist")
    args.work.mkdir(parents=True)
    args.stage.mkdir(parents=True)
    subprocess.run(
        [
            sys.executable,
            "-m",
            "PyInstaller",
            "--clean",
            "--noconfirm",
            "--distpath",
            str(args.work / "dist"),
            "--workpath",
            str(args.work / "build"),
            str(SPEC),
        ],
        cwd=ROOT,
        check=True,
    )
    built = args.work / "dist" / "maia3-engine"
    runtime_root = args.stage / "maia3-runtime"
    if args.platform == "macos-aarch64":
        app = runtime_root / "UCI-Grabber-Maia3.app"
        macos = app / "Contents" / "MacOS"
        macos.parent.mkdir(parents=True)
        shutil.copytree(built, macos)
        create_launchers(macos, False)
        write_macos_plist(app, metadata["component"]["version"])
    else:
        shutil.copytree(built, runtime_root)
        create_launchers(runtime_root, args.platform == "windows-x86_64")
    (runtime_root / "models").mkdir()
    shutil.copy2(args.upstream / "LICENSE", runtime_root / "COPYING-Maia3-AGPL-3.0.txt")
    (runtime_root / "PYTHON-THIRD-PARTY-LICENSES.txt").write_text(
        license_inventory(), encoding="utf-8", newline="\n"
    )
    source_asset = metadata["component"]["corresponding_source_asset"]
    (runtime_root / "CORRESPONDING-SOURCE.txt").write_text(
        corresponding_source_notice(source_asset), encoding="utf-8", newline="\n"
    )
    inventory = json.loads(args.wheel_inventory.read_text(encoding="utf-8"))
    shutil.copy2(args.wheel_inventory, runtime_root / "WHEELHOUSE.lock.json")
    build_info = {
        "schema": 1,
        "component_version": metadata["component"]["version"],
        "platform": args.platform,
        "python": platform.python_version(),
        "distributions": versions,
        "upstream_repository": metadata["component"]["upstream_repository"],
        "upstream_commit": metadata["component"]["upstream_commit"],
        "models_included": False,
        "network_policy": "local checkpoint only; CPU; AMP disabled; Hugging Face offline",
        "corresponding_source": {
            "asset": source_asset,
            "availability": (
                "same GitHub release as this runtime archive, without additional charge"
            ),
        },
        "wheelhouse": inventory,
    }
    (runtime_root / "BUILDINFO.json").write_text(
        json.dumps(build_info, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )


if __name__ == "__main__":
    main()
