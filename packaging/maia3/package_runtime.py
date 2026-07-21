#!/usr/bin/env python3
"""Create a deterministic ZIP or tar.zst Maia3 runtime asset."""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path, PurePosixPath


FIXED_TIME = (2020, 1, 1, 0, 0, 0)


def members(stage: Path) -> list[Path]:
    if not (stage / "maia3-runtime").is_dir():
        raise ValueError("stage must contain maia3-runtime/")
    extras = [path.name for path in stage.iterdir() if path.name != "maia3-runtime"]
    if extras:
        raise ValueError(f"stage has unexpected top-level entries: {extras}")
    stage_resolved = stage.resolve()
    result = sorted(stage.rglob("*"), key=lambda path: path.relative_to(stage).as_posix())
    for path in result:
        relative = PurePosixPath(path.relative_to(stage).as_posix())
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"unsafe archive path: {relative}")
        if path.is_symlink():
            try:
                target = (path.parent / os.readlink(path)).resolve(strict=True)
            except FileNotFoundError as error:
                raise ValueError(f"runtime contains a broken symlink: {relative}") from error
            try:
                target.relative_to(stage_resolved)
            except ValueError as error:
                raise ValueError(f"symlink escapes runtime root: {relative}") from error
            if target.is_dir():
                raise ValueError(f"runtime may not contain directory symlinks: {relative}")
            if not target.is_file():
                raise ValueError(f"runtime symlink does not resolve to a regular file: {relative}")
        elif not path.is_dir() and not path.is_file():
            raise ValueError(f"runtime contains a special filesystem entry: {relative}")
    return result


def create_zip(stage: Path, output: Path, paths: list[Path]) -> None:
    if any(path.is_symlink() for path in paths):
        raise ValueError("Windows runtime ZIP may not contain symlinks")
    with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in paths:
            relative = path.relative_to(stage).as_posix()
            if path.is_dir():
                info = zipfile.ZipInfo(f"{relative}/", FIXED_TIME)
                info.external_attr = (stat.S_IFDIR | 0o755) << 16
                archive.writestr(info, b"")
                continue
            info = zipfile.ZipInfo(relative, FIXED_TIME)
            mode = 0o755 if os.access(path, os.X_OK) else 0o644
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, path.read_bytes(), compresslevel=9)


def normalize_tar_info(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    info.mtime = 0
    if info.isdir():
        info.mode = 0o755
    elif info.isfile():
        info.mode = 0o755 if info.mode & 0o111 else 0o644
    return info


def create_tar(stage: Path, output: Path, paths: list[Path]) -> None:
    with tarfile.open(output, "x", format=tarfile.PAX_FORMAT) as archive:
        # Duplicate hard-linked files as regular members. The installer rejects
        # both tar hard links and symbolic links.
        archive.dereference = True
        for path in paths:
            relative = path.relative_to(stage).as_posix()
            if path.is_symlink():
                # UCI Grabber rejects every link during extraction. Flatten a
                # previously validated in-root file link into a regular member.
                target = path.resolve(strict=True)
                info = tarfile.TarInfo(relative)
                info.size = target.stat().st_size
                info.mode = 0o755 if target.stat().st_mode & 0o111 else 0o644
                normalize_tar_info(info)
                with target.open("rb") as contents:
                    archive.addfile(info, contents)
            else:
                archive.add(
                    path,
                    arcname=relative,
                    recursive=False,
                    filter=normalize_tar_info,
                )


def create_tar_zst(stage: Path, output: Path, paths: list[Path]) -> None:
    temporary = output.with_suffix(output.suffix + ".tar.part")
    if temporary.exists():
        raise ValueError(f"temporary archive already exists: {temporary}")
    try:
        create_tar(stage, temporary, paths)
        subprocess.run(
            ["zstd", "-19", "-T1", "--no-progress", "-o", str(output), str(temporary)],
            check=True,
        )
    finally:
        temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stage", type=Path, required=True)
    parser.add_argument("--format", choices=("zip", "tar.zst"), required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("output already exists")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    paths = members(args.stage)
    if args.format == "zip":
        create_zip(args.stage, args.output, paths)
    else:
        create_tar_zst(args.stage, args.output, paths)
    if not args.output.is_file() or args.output.stat().st_size == 0:
        raise SystemExit("runtime archive was not created")


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        raise SystemExit(f"Runtime packaging failed: {error}") from error
