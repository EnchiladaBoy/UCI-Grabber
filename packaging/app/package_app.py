#!/usr/bin/env python3
"""Create deterministic native UCI Grabber application archives."""

from __future__ import annotations

import argparse
import gzip
import io
import re
import stat
import tarfile
import zipfile
from pathlib import Path, PurePosixPath


FIXED_ZIP_TIME = (2020, 1, 1, 0, 0, 0)
VERSION = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
PLATFORMS = {
    "windows-x86_64",
    "macos-aarch64",
    "linux-x86_64",
    "linux-aarch64",
}


def macos_plist(version: str) -> bytes:
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleDisplayName</key><string>UCI Grabber</string>
  <key>CFBundleExecutable</key><string>uci-grabber</string>
  <key>CFBundleIdentifier</key><string>io.github.enchiladaboy.ucigrabber</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>UCI Grabber</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>LSMinimumSystemVersion</key><string>12.3</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
""".encode()


def payload(
    platform: str,
    version: str,
    binary: Path,
    license_file: Path,
    readme: Path,
    third_party: Path,
) -> list[tuple[str, bytes, int]]:
    documents = {
        "LICENSE": license_file.read_bytes(),
        "README.md": readme.read_bytes(),
        "THIRD-PARTY-LICENSES.txt": third_party.read_bytes(),
    }
    if platform == "macos-aarch64":
        root = "UCI Grabber.app/Contents"
        result = [
            (f"{root}/Info.plist", macos_plist(version), 0o644),
            (f"{root}/MacOS/uci-grabber", binary.read_bytes(), 0o755),
        ]
        result.extend((f"{root}/Resources/{name}", contents, 0o644)
                      for name, contents in documents.items())
        return result
    root = f"uci-grabber-{version}"
    executable = "uci-grabber.exe" if platform == "windows-x86_64" else "uci-grabber"
    result = [(f"{root}/{executable}", binary.read_bytes(), 0o755)]
    result.extend((f"{root}/{name}", contents, 0o644) for name, contents in documents.items())
    return result


def add_zip_directory(archive: zipfile.ZipFile, name: str) -> None:
    info = zipfile.ZipInfo(name.rstrip("/") + "/", FIXED_ZIP_TIME)
    info.external_attr = (stat.S_IFDIR | 0o755) << 16
    archive.writestr(info, b"")


def create_zip(output: Path, files: list[tuple[str, bytes, int]]) -> None:
    directories = sorted(
        {str(parent) for name, _contents, _mode in files for parent in PurePosixPath(name).parents
         if str(parent) != "."},
        key=lambda value: (value.count("/"), value),
    )
    with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for directory in directories:
            add_zip_directory(archive, directory)
        for name, contents, mode in sorted(files):
            info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, contents, compresslevel=9)


def tar_info(name: str, size: int, mode: int, directory: bool = False) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name.rstrip("/") + ("/" if directory else ""))
    info.type = tarfile.DIRTYPE if directory else tarfile.REGTYPE
    info.size = 0 if directory else size
    info.mode = mode
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    info.mtime = 0
    return info


def create_tar_gz(output: Path, files: list[tuple[str, bytes, int]]) -> None:
    directories = sorted(
        {str(parent) for name, _contents, _mode in files for parent in PurePosixPath(name).parents
         if str(parent) != "."},
        key=lambda value: (value.count("/"), value),
    )
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for directory in directories:
            archive.addfile(tar_info(directory, 0, 0o755, True))
        for name, contents, mode in sorted(files):
            archive.addfile(tar_info(name, len(contents), mode), io.BytesIO(contents))
    raw.seek(0)
    with output.open("xb") as destination:
        with gzip.GzipFile(filename="", mode="wb", fileobj=destination, mtime=0, compresslevel=9) as zipped:
            zipped.write(raw.read())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=sorted(PLATFORMS), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--license", dest="license_file", type=Path, required=True)
    parser.add_argument("--readme", type=Path, required=True)
    parser.add_argument("--third-party", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if VERSION.fullmatch(args.version) is None:
        parser.error("version must use MAJOR.MINOR.PATCH")
    for path in (args.binary, args.license_file, args.readme, args.third_party):
        if not path.is_file() or path.stat().st_size == 0:
            parser.error(f"required input is missing or empty: {path}")
    if args.output.exists():
        parser.error("output already exists")
    expected_suffix = ".zip" if args.platform in {"windows-x86_64", "macos-aarch64"} else ".tar.gz"
    if not args.output.name.endswith(expected_suffix):
        parser.error(f"{args.platform} output must end in {expected_suffix}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    files = payload(
        args.platform, args.version, args.binary, args.license_file, args.readme, args.third_party
    )
    if expected_suffix == ".zip":
        create_zip(args.output, files)
    else:
        create_tar_gz(args.output, files)
    if args.output.stat().st_size == 0:
        raise SystemExit("application archive is empty")


if __name__ == "__main__":
    main()
