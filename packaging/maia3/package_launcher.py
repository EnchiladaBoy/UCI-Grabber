#!/usr/bin/env python3
"""Create a deterministic, source-visible Maia3 process-launcher archive."""

from __future__ import annotations

import argparse
import gzip
import io
import stat
import tarfile
import zipfile
from pathlib import Path


FIXED_ZIP_TIME = (2020, 1, 1, 0, 0, 0)
PLATFORMS = {
    "windows-x86_64",
    "macos-aarch64",
    "linux-x86_64",
    "linux-aarch64",
}
PAYLOAD_REVIEW_PLACEHOLDER = (
    b"\0UCI_GRABBER_PAYLOAD_REVIEW_V1\0"
    + b"X" * 64
    + b"X" * 16
    + b"X" * 16
)


def checked_contents(path: Path, description: str) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"{description} is missing or is not a regular file: {path}")
    contents = path.read_bytes()
    if not contents:
        raise ValueError(f"{description} is empty: {path}")
    return contents


def checked_launcher(path: Path) -> bytes:
    contents = checked_contents(path, "launcher binary")
    matches = contents.count(PAYLOAD_REVIEW_PLACEHOLDER)
    if matches != 1:
        raise ValueError(
            "launcher binary must contain exactly one unpersonalized payload field; "
            f"found {matches}"
        )
    return contents


def payload(
    platform: str,
    binary: Path,
    entry_point: Path,
    license_file: Path,
    notices: Path,
    third_party: Path,
) -> list[tuple[str, bytes, int]]:
    executable_name = "maia3-launcher.exe" if platform == "windows-x86_64" else "maia3-launcher"
    return [
        (executable_name, checked_launcher(binary), 0o755),
        ("maia3_entry.py", checked_contents(entry_point, "Python entry point"), 0o644),
        ("LICENSE", checked_contents(license_file, "Apache license"), 0o644),
        (
            "DIRECT-DOWNLOAD-NOTICES.txt",
            checked_contents(notices, "direct-download notices"),
            0o644,
        ),
        (
            "THIRD-PARTY-LICENSES.txt",
            checked_contents(third_party, "Rust dependency notices"),
            0o644,
        ),
    ]


def create_zip(output: Path, files: list[tuple[str, bytes, int]]) -> None:
    with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, contents, mode in sorted(files):
            info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, contents, compresslevel=9)


def tar_info(name: str, contents: bytes, mode: int) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.type = tarfile.REGTYPE
    info.size = len(contents)
    info.mode = mode
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    info.mtime = 0
    return info


def create_tar_gz(output: Path, files: list[tuple[str, bytes, int]]) -> None:
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, contents, mode in sorted(files):
            archive.addfile(tar_info(name, contents, mode), io.BytesIO(contents))
    with output.open("xb") as destination:
        with gzip.GzipFile(
            filename="",
            mode="wb",
            fileobj=destination,
            mtime=0,
            compresslevel=9,
        ) as compressed:
            compressed.write(raw.getvalue())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=sorted(PLATFORMS), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--entry", type=Path, required=True)
    parser.add_argument("--license", dest="license_file", type=Path, required=True)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--third-party", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    if args.output.exists():
        parser.error("output already exists")
    expected_suffix = ".zip" if args.platform == "windows-x86_64" else ".tar.gz"
    if not args.output.name.endswith(expected_suffix):
        parser.error(f"{args.platform} output must end in {expected_suffix}")
    try:
        files = payload(
            args.platform,
            args.binary,
            args.entry,
            args.license_file,
            args.notices,
            args.third_party,
        )
    except (OSError, ValueError) as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.platform == "windows-x86_64":
        create_zip(args.output, files)
    else:
        create_tar_gz(args.output, files)
    if not args.output.is_file() or args.output.stat().st_size == 0:
        raise SystemExit("launcher archive was not created")


if __name__ == "__main__":
    main()
