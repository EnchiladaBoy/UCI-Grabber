#!/usr/bin/env python3
"""Validate checked-in Maia3 release inputs without network access."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parent
METADATA = ROOT / "component-metadata.json"
SOURCE_POLICY = ROOT / "corresponding-source-policy.json"
HEX_40 = re.compile(r"^[0-9a-f]{40}$")
HEX_64 = re.compile(r"^[0-9a-f]{64}$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")
EXPECTED_PLATFORMS = {
    "windows-x86_64": ("zip", ".zip", ".exe"),
    "macos-aarch64": ("tar.zst", ".tar.zst", ""),
    "linux-x86_64": ("tar.zst", ".tar.zst", ""),
    "linux-aarch64": ("tar.zst", ".tar.zst", ""),
}
EXPECTED_MODELS = {
    "maia3-5m": (
        "UofTCSSLab/Maia3-5M",
        "b6559de2398d7140b985f28fd2c19fb5e47ddabe",
        20_968_049,
        "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f",
    ),
    "maia3-23m": (
        "UofTCSSLab/Maia3-23M",
        "51a0145a8178046f7de23119160b136672deeb2b",
        91_799_307,
        "bce6cd1af5f0399ac7eed33fabb7a6a2ef6193662c2740f262bf93af7bfb3569",
    ),
    "maia3-79m": (
        "UofTCSSLab/Maia3-79M",
        "a107d6ceb7b298cb04ae1da4edffe2939858b894",
        315_651_851,
        "3fc6181d5db789b45a15305732148757ae74efa3e0028e81ba335b462dac45c2",
    ),
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def safe_relative_template(value: object, field: str) -> None:
    require(isinstance(value, str), f"{field} must be a string")
    rendered = value.replace("{model}", "maia3-5m")
    path = PurePosixPath(rendered)
    require(not path.is_absolute(), f"{field} must be relative")
    require(".." not in path.parts and "\\" not in rendered, f"{field} is unsafe")
    require(value.count("{model}") == 1, f"{field} must contain one model placeholder")


def validate_source_policy() -> None:
    policy = json.loads(SOURCE_POLICY.read_text(encoding="utf-8"))
    require(
        set(policy) == {"schema", "publication", "distribution", "required_written_review"},
        "source policy has unknown or missing top-level fields",
    )
    require(policy["schema"] == 1, "source policy schema must be 1")
    require(
        policy["publication"]
        == {
            "default": "blocked_pending_written_corresponding_source_review",
            "gate_environment": "MAIA3_CORRESPONDING_SOURCE_REVIEW",
            "gate_value": "sha256(source-release-review-v3: source-review-v2 inputs plus four reviewed wheelhouse digests; see review_digest.py)",
        },
        "corresponding-source publication must remain fail-closed",
    )
    distribution = policy["distribution"]
    require(
        set(distribution)
        == {
            "form",
            "agpl_component",
            "corresponding_source_asset",
            "corresponding_source_availability",
            "included_source_materials",
            "not_bundled_as_source",
        },
        "source policy distribution has unknown or missing fields",
    )
    require(
        distribution["agpl_component"].endswith(
            "1e13597c42d4858b7cfd7cfdae01e297263364b2"
        ),
        "source policy must bind the reviewed Maia3 commit",
    )
    require(
        distribution["corresponding_source_asset"] == "maia3-corresponding-source.tar.gz",
        "source policy must name the immutable release source asset",
    )
    require(
        "same GitHub release" in distribution["corresponding_source_availability"]
        and "without additional charge" in distribution["corresponding_source_availability"],
        "source policy must require same-release source availability",
    )
    require(
        distribution["not_bundled_as_source"]
        == [
            "CPython 3.12.10",
            "PyTorch 2.11.0 CPU",
            "NumPy 2.2.6",
            "PyInstaller 6.21.0 bootloader and other reviewed wheel dependencies",
        ],
        "source policy must enumerate source materials not bundled by default",
    )
    require(
        distribution["included_source_materials"]
        == [
            "exact Maia3 upstream checkout",
            "exact chess 1.11.2 source distribution",
            "UCI Grabber Maia3 build and packaging definitions",
            "exact UCI Grabber release and wheelhouse-review workflow definitions",
        ],
        "source policy must enumerate every digest-bound source/build input class",
    )
    review = policy["required_written_review"]
    require(
        isinstance(review, list)
        and len(review) == 4
        and all(isinstance(item, str) and item.strip() for item in review),
        "source policy must preserve all written-review questions",
    )


def validate() -> dict[str, object]:
    validate_source_policy()
    data = json.loads(METADATA.read_text(encoding="utf-8"))
    require(
        set(data) == {"schema", "publication", "component", "runtimes", "models"},
        "metadata has unknown or missing top-level fields",
    )
    require(data["schema"] == 1, "metadata schema must be 1")
    publication = data["publication"]
    require(
        publication
        == {
            "default": "excluded_pending_checkpoint_download_use_redistribution_review",
            "gate_environment": "MAIA3_MODEL_LICENSE_REVIEW",
            "gate_value": "sha256(component-metadata.json)",
        },
        "Maia3 publication must remain fail-closed behind the reviewed digest",
    )

    component = data["component"]
    require(
        set(component)
        == {
            "version",
            "minimum_fisheye_version",
            "upstream_repository",
            "upstream_commit",
            "corresponding_source_asset",
            "notices_asset",
        },
        "component has unknown or missing fields",
    )
    require(VERSION.fullmatch(component["version"]) is not None, "invalid component version")
    require(
        component["minimum_fisheye_version"] == "1.8.0",
        "FishEye CLI handoff requires FishEye 1.8.0 or newer",
    )
    require(
        component["upstream_repository"] == "https://github.com/CSSLab/maia3",
        "unexpected Maia3 upstream",
    )
    require(
        HEX_40.fullmatch(component["upstream_commit"]) is not None,
        "upstream commit must be an immutable full SHA-1",
    )
    require(
        component["corresponding_source_asset"] == "maia3-corresponding-source.tar.gz",
        "unexpected corresponding-source asset name",
    )
    require(component["notices_asset"] == "MAIA3-NOTICES.txt", "unexpected notices asset name")

    runtimes = data["runtimes"]
    require(set(runtimes) == set(EXPECTED_PLATFORMS), "runtime platform set changed")
    seen_assets: set[str] = set()
    for platform, (archive, suffix, executable_suffix) in EXPECTED_PLATFORMS.items():
        runtime = runtimes[platform]
        require(
            set(runtime)
            == {"asset", "archive", "executable_template", "model_destination_template"},
            f"{platform} runtime has unknown or missing fields",
        )
        require(runtime["archive"] == archive, f"unexpected {platform} archive type")
        require(runtime["asset"].endswith(suffix), f"unexpected {platform} asset suffix")
        require(runtime["asset"] not in seen_assets, "runtime asset names must be unique")
        seen_assets.add(runtime["asset"])
        safe_relative_template(runtime["executable_template"], "executable_template")
        safe_relative_template(runtime["model_destination_template"], "model destination")
        rendered = runtime["executable_template"].format(model="maia3-5m")
        require(rendered.endswith(executable_suffix), f"unexpected {platform} executable suffix")

    models = data["models"]
    require(set(models) == set(EXPECTED_MODELS), "reviewed model set changed")
    for model_id, (repository, revision, size, digest) in EXPECTED_MODELS.items():
        model = models[model_id]
        require(
            set(model)
            == {
                "display_name",
                "description",
                "repository",
                "revision",
                "filename",
                "bytes",
                "sha256",
            },
            f"{model_id} has unknown or missing fields",
        )
        require(model["repository"] == repository, f"unexpected {model_id} repository")
        require(model["revision"] == revision, f"unexpected {model_id} revision")
        require(model["filename"] == f"{model_id}.pt", f"unexpected {model_id} filename")
        require(model["bytes"] == size, f"unexpected {model_id} byte count")
        require(model["sha256"] == digest, f"unexpected {model_id} digest")
        require(HEX_40.fullmatch(model["revision"]) is not None, "model revision is mutable")
        require(HEX_64.fullmatch(model["sha256"]) is not None, "model digest is invalid")
    return data


if __name__ == "__main__":
    try:
        validate()
    except (OSError, ValueError, json.JSONDecodeError, TypeError) as error:
        print(f"Maia3 metadata validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    print("Maia3 component metadata is valid and publication remains review-gated.")
