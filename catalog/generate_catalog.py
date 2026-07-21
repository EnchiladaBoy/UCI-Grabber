#!/usr/bin/env python3
"""Generate a canonical production catalog; Maia3 is excluded unless reviewed."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from urllib.parse import quote


ROOT = Path(__file__).resolve().parent
REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
TAG = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
sys.path.insert(0, str(ROOT.parent / "packaging" / "maia3"))
from validate_metadata import (  # noqa: E402
    METADATA,
    validate as validate_maia_metadata,
    validate_direct_downloads,
)
from validate_catalog import load_and_validate  # noqa: E402


def sha256(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def catalog_artifact(source: dict[str, object], kind: str) -> dict[str, object]:
    return {
        "kind": kind,
        "url": source["url"],
        "byte_count": source["byte_count"],
        "sha256": source["sha256"],
        "format": source["format"],
        "destination": source["destination"],
    }


def upstream_artifacts(
    direct_downloads: dict[str, object], platform: str
) -> list[dict[str, object]]:
    packages = direct_downloads["packages"]
    common = packages["common"]
    variants = packages["platforms"][platform]
    artifacts = [
        catalog_artifact(direct_downloads["python"]["platforms"][platform], "other"),
        catalog_artifact(direct_downloads["sources"]["maia3"], "other"),
        catalog_artifact(direct_downloads["sources"]["chess"], "other"),
    ]
    for package in packages["required"]:
        artifacts.append(catalog_artifact(common.get(package, variants.get(package)), "other"))
    if len(artifacts) != 14:
        raise ValueError(f"{platform} must have exactly 14 direct upstream artifacts")
    return artifacts


def maia_recipe(assets: Path, repository: str, tag: str) -> dict[str, object]:
    metadata = validate_maia_metadata()
    direct_downloads = validate_direct_downloads()
    component = metadata["component"]
    runtimes = metadata["runtimes"]
    models = []
    for model_id, model in metadata["models"].items():
        packages = []
        for platform, runtime in runtimes.items():
            asset = assets / runtime["asset"]
            if not asset.is_file() or asset.stat().st_size == 0:
                raise ValueError(f"missing Maia3 launcher asset: {asset}")
            if asset.stat().st_size > 1024 * 1024 * 1024:
                raise ValueError(f"Maia3 launcher asset exceeds 1 GiB: {asset}")
            launcher_url = (
                f"https://github.com/{repository}/releases/download/{tag}/{quote(runtime['asset'])}"
            )
            model_url = (
                f"https://huggingface.co/{model['repository']}/resolve/"
                f"{model['revision']}/{quote(model['filename'])}"
            )
            executable = "package/launcher/" + runtime["executable_template"]
            artifacts = [
                {
                    "kind": "runtime",
                    "url": launcher_url,
                    "byte_count": asset.stat().st_size,
                    "sha256": sha256(asset),
                    "format": runtime["archive"],
                    "destination": "package/launcher",
                },
                *upstream_artifacts(direct_downloads, platform),
                {
                    "kind": "model",
                    "url": model_url,
                    "byte_count": model["bytes"],
                    "sha256": model["sha256"],
                    "format": "raw",
                    "destination": (
                        "package/launcher/"
                        + runtime["model_destination_template"].format(model=model_id)
                    ),
                },
            ]
            if len(artifacts) != 16:
                raise ValueError(f"{platform} Maia3 package must have exactly 16 artifacts")
            packages.append(
                {
                    "platform": platform,
                    "artifacts": artifacts,
                    "executable": executable,
                    "working_directory": executable.rsplit("/", 1)[0],
                }
            )
        models.append(
            {
                "id": model_id,
                "name": model["display_name"],
                "description": model["description"],
                "packages": packages,
            }
        )
    notices_asset = component["notices_asset"]
    return {
        "schema": "uci-grabber-recipe/v1",
        "id": "maia3",
        "name": "Maia3",
        "version": component["version"],
        "description": (
            "Human-like chess UCI engine assembled locally from immutable direct "
            "upstream downloads and a selected Maia3 checkpoint."
        ),
        "publisher": {
            "name": "Computational Social Science Lab, University of Toronto",
            "url": "https://github.com/CSSLab",
        },
        "license": {
            "spdx": "LicenseRef-Maia3-Composite-Terms",
            "name": (
                "Composite installation: Maia3 code under AGPL-3.0; direct "
                "dependencies and checkpoints retain their respective terms"
            ),
            "url": (
                f"https://github.com/{repository}/releases/download/{tag}/"
                f"{quote(notices_asset)}"
            ),
            "source_url": (
                f"{component['upstream_repository']}/tree/{component['upstream_commit']}"
            ),
        },
        "homepage": "https://github.com/CSSLab/maia3",
        "minimum_fisheye_version": component["minimum_fisheye_version"],
        "models": models,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--assets-dir", type=Path, required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--generated-at", required=True)
    parser.add_argument("--expires-at", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--include-maia3", action="store_true")
    parser.add_argument("--maia3-license-review-digest")
    args = parser.parse_args()
    if REPOSITORY.fullmatch(args.repository) is None:
        parser.error("repository must be OWNER/REPO")
    if TAG.fullmatch(args.tag) is None:
        parser.error("tag must use stable vMAJOR.MINOR.PATCH form")
    if args.output.exists():
        parser.error("output already exists")
    if args.maia3_license_review_digest and not args.include_maia3:
        parser.error("a Maia3 review digest has no effect without --include-maia3")

    recipes = []
    if args.include_maia3:
        reviewed_digest = sha256(METADATA)
        if args.maia3_license_review_digest != reviewed_digest:
            parser.error(
                "Maia3 publication requires the exact reviewed component-metadata.json SHA-256"
            )
        recipes.append(maia_recipe(args.assets_dir, args.repository, args.tag))
    catalog = {
        "expires_at": args.expires_at,
        "generated_at": args.generated_at,
        "recipes": recipes,
        "schema": "uci-grabber-catalog/v1",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(catalog, indent=2, sort_keys=True, separators=(",", ": ")) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    try:
        load_and_validate(args.output)
    except Exception:
        args.output.unlink(missing_ok=True)
        raise


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Catalog generation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
