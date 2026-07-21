#!/usr/bin/env python3
"""Validate checked-in Maia3 release inputs without network access."""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path, PurePosixPath
from urllib.parse import urlsplit


ROOT = Path(__file__).resolve().parent
METADATA = ROOT / "component-metadata.json"
DIRECT_DOWNLOADS = ROOT / "direct-downloads.json"
HEX_40 = re.compile(r"^[0-9a-f]{40}$")
HEX_64 = re.compile(r"^[0-9a-f]{64}$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")
EXPECTED_PLATFORMS = {
    "windows-x86_64": (
        "zip",
        "uci-grabber-maia3-launcher-windows-x86_64.zip",
        "maia3-launcher.exe",
    ),
    "macos-aarch64": (
        "tar.gz",
        "uci-grabber-maia3-launcher-macos-aarch64.tar.gz",
        "maia3-launcher",
    ),
    "linux-x86_64": (
        "tar.gz",
        "uci-grabber-maia3-launcher-linux-x86_64.tar.gz",
        "maia3-launcher",
    ),
    "linux-aarch64": (
        "tar.gz",
        "uci-grabber-maia3-launcher-linux-aarch64.tar.gz",
        "maia3-launcher",
    ),
}
DIRECT_DOWNLOADS_SHA256 = "bf4dd026518d45fd31b782fc85aa45568a21c9e2005a8ca88cf9ca945254f715"
RUNTIME_PACKAGES = (
    "filelock",
    "fsspec",
    "jinja2",
    "markupsafe",
    "mpmath",
    "networkx",
    "numpy",
    "setuptools",
    "sympy",
    "torch",
    "typing-extensions",
)
COMMON_RUNTIME_PACKAGES = {
    "filelock",
    "fsspec",
    "jinja2",
    "mpmath",
    "networkx",
    "setuptools",
    "sympy",
    "typing-extensions",
}
PLATFORM_RUNTIME_PACKAGES = {"markupsafe", "numpy", "torch"}
DIRECT_ARTIFACT_FIELDS = {"url", "byte_count", "sha256", "format", "destination"}
DIRECT_DOWNLOAD_HOSTS = {
    "github.com",
    "codeload.github.com",
    "files.pythonhosted.org",
    "download-r2.pytorch.org",
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


def safe_relative(value: object, field: str) -> str:
    require(isinstance(value, str), f"{field} must be a string")
    path = PurePosixPath(value)
    require(not path.is_absolute(), f"{field} must be relative")
    require(".." not in path.parts and "\\" not in value, f"{field} is unsafe")
    require(value not in {"", "."}, f"{field} must name a file or directory")
    return value


def safe_relative_template(value: object, field: str) -> None:
    require(isinstance(value, str), f"{field} must be a string")
    require(value.count("{model}") == 1, f"{field} must contain one model placeholder")
    safe_relative(value.replace("{model}", "maia3-5m"), field)


def direct_artifact(value: object, label: str) -> dict[str, object]:
    require(isinstance(value, dict), f"{label} must be an object")
    require(set(value) == DIRECT_ARTIFACT_FIELDS, f"{label} has unknown or missing fields")
    url = value["url"]
    require(isinstance(url, str), f"{label} URL must be a string")
    parsed = urlsplit(url)
    require(
        parsed.scheme == "https"
        and parsed.hostname in DIRECT_DOWNLOAD_HOSTS
        and parsed.username is None
        and parsed.password is None
        and not parsed.query
        and not parsed.fragment,
        f"{label} URL must be an approved direct HTTPS download",
    )
    require(
        type(value["byte_count"]) is int and 0 < value["byte_count"] <= 1024 * 1024 * 1024,
        f"{label} byte count is invalid",
    )
    require(
        isinstance(value["sha256"], str) and HEX_64.fullmatch(value["sha256"]) is not None,
        f"{label} SHA-256 is invalid",
    )
    require(value["format"] in {"zip", "tar.gz"}, f"{label} format is invalid")
    safe_relative(value["destination"], f"{label} destination")
    return value


def validate_direct_downloads(path: Path = DIRECT_DOWNLOADS) -> dict[str, object]:
    raw = path.read_bytes()
    require(
        hashlib.sha256(raw).hexdigest() == DIRECT_DOWNLOADS_SHA256,
        "direct-downloads.json does not match the reviewed exact artifact manifest",
    )
    data = json.loads(raw)
    require(
        isinstance(data, dict) and set(data) == {"schema", "python", "sources", "packages"},
        "direct downloads have unknown or missing top-level fields",
    )
    require(
        data["schema"] == "uci-grabber-direct-downloads/v1",
        "direct downloads schema is unsupported",
    )

    python = data["python"]
    require(
        isinstance(python, dict)
        and set(python) == {"distribution", "version", "release", "platforms"},
        "portable Python has unknown or missing fields",
    )
    require(
        (python["distribution"], python["version"], python["release"])
        == ("python-build-standalone", "3.12.13", "20260510"),
        "portable Python release changed",
    )
    require(
        isinstance(python["platforms"], dict)
        and set(python["platforms"]) == set(EXPECTED_PLATFORMS),
        "portable Python platform set changed",
    )
    python_artifacts = {}
    for platform, value in python["platforms"].items():
        artifact = direct_artifact(value, f"{platform} portable Python")
        require(artifact["format"] == "tar.gz", "portable Python must be a tar.gz")
        require(
            artifact["destination"] == "package/python-runtime",
            "portable Python destination changed",
        )
        require(
            urlsplit(artifact["url"]).hostname == "github.com",
            "portable Python must come directly from python-build-standalone",
        )
        python_artifacts[platform] = artifact

    sources = data["sources"]
    require(isinstance(sources, dict) and set(sources) == {"maia3", "chess"},
            "direct source set changed")
    source_destinations = {
        "maia3": ("package/maia-source", "codeload.github.com"),
        "chess": ("package/chess-source", "files.pythonhosted.org"),
    }
    source_artifacts = {}
    for name, (destination, hostname) in source_destinations.items():
        artifact = direct_artifact(sources[name], f"{name} source")
        require(artifact["format"] == "tar.gz", f"{name} source must be a tar.gz")
        require(artifact["destination"] == destination, f"{name} source destination changed")
        require(urlsplit(artifact["url"]).hostname == hostname, f"{name} source host changed")
        source_artifacts[name] = artifact

    packages = data["packages"]
    require(
        isinstance(packages, dict) and set(packages) == {"required", "common", "platforms"},
        "runtime packages have unknown or missing fields",
    )
    require(packages["required"] == list(RUNTIME_PACKAGES), "required runtime package set changed")
    common = packages["common"]
    platforms = packages["platforms"]
    require(isinstance(common, dict) and set(common) == COMMON_RUNTIME_PACKAGES,
            "common runtime package set changed")
    require(isinstance(platforms, dict) and set(platforms) == set(EXPECTED_PLATFORMS),
            "platform runtime package set changed")

    all_platform_packages: dict[str, list[dict[str, object]]] = {}
    for platform in EXPECTED_PLATFORMS:
        variants = platforms[platform]
        require(isinstance(variants, dict) and set(variants) == PLATFORM_RUNTIME_PACKAGES,
                f"{platform} runtime package set changed")
        artifacts = []
        destinations = set()
        for package in RUNTIME_PACKAGES:
            artifact = direct_artifact(
                common.get(package, variants.get(package)),
                f"{platform} {package} package",
            )
            require(artifact["format"] == "zip", f"{package} must be extracted as a wheel")
            require(
                artifact["destination"] == f"package/packages/{package}",
                f"{package} package destination changed",
            )
            require(artifact["destination"] not in destinations,
                    f"{platform} package destinations are not unique")
            destinations.add(artifact["destination"])
            hostname = urlsplit(artifact["url"]).hostname
            expected_host = (
                "download-r2.pytorch.org"
                if package == "torch" and platform != "macos-aarch64"
                else "files.pythonhosted.org"
            )
            require(hostname == expected_host, f"{platform} {package} download host changed")
            filename = urlsplit(artifact["url"]).path.rsplit("/", 1)[-1]
            require(filename.endswith(".whl"), f"{platform} {package} is not a wheel URL")
            artifacts.append(artifact)
        require(len(artifacts) == 11, f"{platform} must use eleven runtime dependency wheels")
        all_platform_packages[platform] = artifacts

    for platform in EXPECTED_PLATFORMS:
        destinations = {
            python_artifacts[platform]["destination"],
            *(artifact["destination"] for artifact in source_artifacts.values()),
            *(artifact["destination"] for artifact in all_platform_packages[platform]),
        }
        require(len(destinations) == 14, f"{platform} direct artifact destinations overlap")
    return data


def validate() -> dict[str, object]:
    validate_direct_downloads()
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
            "default": "excluded_unless_checked_release_review_verifies",
            "gate_file": "release-review.json",
            "gate_value": "exact tag, this file's SHA-256, direct-retrieval decision, and pinned checkpoint facts",
        },
        "Maia3 publication must remain fail-closed behind the checked release review",
    )

    component = data["component"]
    require(
        set(component)
        == {
            "version",
            "minimum_fisheye_version",
            "upstream_repository",
            "upstream_commit",
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
        component["notices_asset"] == "MAIA3-DIRECT-DOWNLOAD-NOTICES.txt",
        "unexpected notices asset name",
    )

    runtimes = data["runtimes"]
    require(set(runtimes) == set(EXPECTED_PLATFORMS), "runtime platform set changed")
    seen_assets: set[str] = set()
    for platform, (archive, asset, executable) in EXPECTED_PLATFORMS.items():
        runtime = runtimes[platform]
        require(
            set(runtime)
            == {"asset", "archive", "executable_template", "model_destination_template"},
            f"{platform} runtime has unknown or missing fields",
        )
        require(runtime["archive"] == archive, f"unexpected {platform} archive type")
        require(runtime["asset"] == asset, f"unexpected {platform} launcher asset")
        require(runtime["asset"] not in seen_assets, "runtime asset names must be unique")
        seen_assets.add(runtime["asset"])
        safe_relative(runtime["executable_template"], "executable_template")
        safe_relative_template(runtime["model_destination_template"], "model destination")
        require(
            runtime["executable_template"] == executable,
            f"unexpected {platform} launcher executable",
        )
        require(
            runtime["model_destination_template"] == "models/{model}.pt",
            f"unexpected {platform} model destination",
        )

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
