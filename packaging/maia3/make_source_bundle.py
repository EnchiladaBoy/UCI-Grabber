#!/usr/bin/env python3
"""Create deterministic Maia3 source/build-material and notice assets."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import tarfile
from pathlib import Path, PurePosixPath

from review_digest import HEX_64, source_review_digest


ROOT = Path(__file__).resolve().parent
REPOSITORY_ROOT = ROOT.parents[1]
CHESS_SOURCE = {
    "filename": "chess-1.11.2.tar.gz",
    "bytes": 6_131_385,
    "sha256": "a8b43e5678fdb3000695bdaa573117ad683761e5ca38e591c4826eba6d25bb39",
}
INCLUDED_PACKAGING_FILES = (
    "README.md",
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
INCLUDED_REPOSITORY_FILES = (
    ".github/workflows/maia-wheelhouse-review.yml",
    ".github/workflows/release.yml",
)


def normalized(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = info.gid = 0
    info.uname = info.gname = "root"
    info.mtime = 0
    info.mode = 0o755 if info.isdir() or info.mode & 0o111 else 0o644
    return info


def add_bytes(archive: tarfile.TarFile, name: str, contents: bytes) -> None:
    info = tarfile.TarInfo(name)
    info.size = len(contents)
    info.mode = 0o644
    normalized(info)
    archive.addfile(info, io.BytesIO(contents))


def source_paths(upstream: Path) -> list[Path]:
    result = []
    for path in upstream.rglob("*"):
        relative = path.relative_to(upstream)
        if ".git" in relative.parts or "__pycache__" in relative.parts:
            continue
        pure = PurePosixPath(relative.as_posix())
        if pure.is_absolute() or ".." in pure.parts:
            raise ValueError(f"unsafe source path: {relative}")
        result.append(path)
    return sorted(result, key=lambda path: path.relative_to(upstream).as_posix())


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def checkpoint_terms_text(models: dict[str, object]) -> str:
    lines = []
    for model_id, model in models.items():
        repository = model["repository"]
        revision = model["revision"]
        filename = model["filename"]
        lines.extend(
            [
                f"  {model_id}",
                f"    Repository: https://huggingface.co/{repository}",
                f"    Reviewed revision: {revision}",
                (
                    "    Pinned model card / terms source: "
                    f"https://huggingface.co/{repository}/blob/{revision}/README.md"
                ),
                (
                    "    Checkpoint download: "
                    f"https://huggingface.co/{repository}/resolve/{revision}/{filename}"
                ),
                f"    Bytes: {model['bytes']}",
                f"    SHA-256: {model['sha256']}",
            ]
        )
    return "\n".join(lines)


def notice_text(metadata: dict[str, object], review_digest: str, license_text: str) -> str:
    component = metadata["component"]
    source_asset = component["corresponding_source_asset"]
    checkpoints = checkpoint_terms_text(metadata["models"])
    return f"""UCI Grabber separately distributed Maia3 component notices
===========================================================

The optional model-free runtime is not part of the Apache-2.0 UCI Grabber
application. It runs Maia3 through a UCI process boundary.

Maia3
  Project: {component['upstream_repository']}
  Reviewed commit: {component['upstream_commit']}
  Version: 0.1.0
  License: GNU Affero General Public License v3.0
  Reviewed source/build archive: {source_asset}
  Availability: same GitHub release as every Maia3 runtime archive, without
                additional charge

The signed catalog's Source link points directly to the exact same-tag
{source_asset} asset. The archive contains the exact Maia3 checkout, chess
1.11.2 source, UCI Grabber's component build definitions, and the release
workflows that control them. It does not automatically carry source archives
for CPython, PyTorch, NumPy, PyInstaller, or every transitive wheel. Publication
is separately gated on the digest-bound written review in
corresponding-source-policy.json; that review identifies any additional source
archives or durable offers required for a given release.
This notice records the release materials and review result; it is not a legal
conclusion about the classification of every packaged dependency.

Reviewed source/build-and-wheel digest: {review_digest}

Checkpoint bytes are downloaded directly from immutable Hugging Face revisions
and are not UCI Grabber release assets. UCI Grabber does not relicense them.
The exact model-card pages reviewed for checkpoint download, use, and
redistribution are listed below alongside the pinned bytes:

{checkpoints}

Complete Maia3 license text
---------------------------

{license_text.rstrip()}
"""


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--upstream", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--dependency-source", type=Path, action="append", default=[])
    parser.add_argument("--source-review-digest", required=True)
    args = parser.parse_args()
    metadata = json.loads((ROOT / "component-metadata.json").read_text(encoding="utf-8"))
    component = metadata["component"]
    if args.output.exists() or args.notices.exists():
        parser.error("output paths must not already exist")
    if HEX_64.fullmatch(args.source_review_digest) is None:
        parser.error("--source-review-digest must be lowercase SHA-256")
    if args.output.name != component["corresponding_source_asset"]:
        parser.error(
            "output filename must match the reviewed corresponding-source release asset"
        )
    if args.notices.name != component["notices_asset"]:
        parser.error("notices filename must match the reviewed composite notice asset")
    if not (args.upstream / "LICENSE").is_file() or not (args.upstream / "maia3").is_dir():
        parser.error("upstream path is not a complete Maia3 checkout")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.notices.parent.mkdir(parents=True, exist_ok=True)
    sources = [
        {"filename": path.name, "bytes": path.stat().st_size, "sha256": sha256(path)}
        for path in args.dependency_source
        if path.is_file() and path.name == Path(path.name).name
    ]
    if sources != [CHESS_SOURCE]:
        parser.error("source/build bundle must include the exact reviewed chess 1.11.2 sdist")

    build_info = {
        "schema": 1,
        "component_version": component["version"],
        "upstream_repository": component["upstream_repository"],
        "upstream_commit": component["upstream_commit"],
        "python": "CPython 3.12.10",
        "torch": "2.11.0 CPU wheel from https://download.pytorch.org/whl/cpu",
        "models_included": False,
        "build_definitions": "uci-grabber-packaging/ and uci-grabber-repository/",
        "per_platform_wheel_hashes": "WHEELHOUSE.lock.json in each runtime",
        "source_material_scope": (
            "Maia3, chess 1.11.2, and build definitions are included; see "
            "corresponding-source-policy.json for reviewed dependency classification"
        ),
        "source_review_input_digest": source_review_digest(),
        "source_release_review_digest": args.source_review_digest,
        "dependency_sources": sources,
    }
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for path in source_paths(args.upstream):
            archive.add(
                path,
                arcname=f"maia3-source/upstream/{path.relative_to(args.upstream).as_posix()}",
                recursive=False,
                filter=normalized,
            )
        for filename in INCLUDED_PACKAGING_FILES:
            path = ROOT / filename
            if not path.is_file():
                raise ValueError(f"missing source-build input: {path}")
            archive.add(
                path,
                arcname=f"maia3-source/uci-grabber-packaging/{filename}",
                recursive=False,
                filter=normalized,
            )
        for filename in INCLUDED_REPOSITORY_FILES:
            path = REPOSITORY_ROOT / filename
            if not path.is_file():
                raise ValueError(f"missing repository build input: {path}")
            archive.add(
                path,
                arcname=f"maia3-source/uci-grabber-repository/{filename}",
                recursive=False,
                filter=normalized,
            )
        for source in args.dependency_source:
            archive.add(
                source,
                arcname=f"maia3-source/dependency-sources/{source.name}",
                recursive=False,
                filter=normalized,
            )
        add_bytes(
            archive,
            "maia3-source/BUILDINFO.json",
            (json.dumps(build_info, indent=2, sort_keys=True) + "\n").encode(),
        )
    raw.seek(0)
    with args.output.open("xb") as destination:
        with gzip.GzipFile(filename="", mode="wb", fileobj=destination, mtime=0, compresslevel=9) as zipped:
            zipped.write(raw.read())

    upstream_license = (args.upstream / "LICENSE").read_text(encoding="utf-8")
    notices = notice_text(metadata, args.source_review_digest, upstream_license)
    args.notices.write_text(notices, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    try:
        main()
    except ValueError as error:
        raise SystemExit(f"Source bundle failed: {error}") from error
