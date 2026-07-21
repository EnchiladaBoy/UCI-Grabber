#!/usr/bin/env python3
"""Strict, dependency-free validation for UCI Grabber catalog JSON."""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit


MAX_MANIFEST_BYTES = 512 * 1024
MAX_PACKAGE_DOWNLOAD_BYTES = 2 * 1024 * 1024 * 1024
PLATFORMS = {
    "linux-x86_64",
    "linux-aarch64",
    "macos-x86_64",
    "macos-aarch64",
    "windows-x86_64",
    "windows-aarch64",
}
FORMATS = {"raw", "zip", "tar.gz", "tar.zst"}
KINDS = {"runtime", "model", "other"}
SLUG = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
SEMVER = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
FISHEYE_VERSION = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SPDX = re.compile(r"^(?:[A-Za-z0-9.+-]+|LicenseRef-[A-Za-z0-9.-]+)$")
WINDOWS_INVALID_PATH_CHARACTERS = frozenset('<>:"/\\|?*')
WINDOWS_RESERVED_DEVICES = {
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def exact_object(value: object, keys: set[str], label: str) -> dict[str, object]:
    require(isinstance(value, dict), f"{label} must be an object")
    require(set(value) == keys, f"{label} has unknown or missing fields")
    return value


def text(value: object, label: str, maximum: int) -> str:
    require(isinstance(value, str), f"{label} must be a string")
    require(value.strip() != "" and len(value) <= maximum, f"{label} is empty or too long")
    require(not any(ord(character) < 32 or ord(character) == 127 for character in value),
            f"{label} contains control characters")
    return value


def https(value: object, label: str) -> str:
    value = text(value, label, 2048)
    parsed = urlsplit(value)
    require(
        parsed.scheme == "https" and parsed.netloc != "" and parsed.username is None,
        f"{label} must be an HTTPS URL without credentials",
    )
    return value


def slug(value: object, label: str) -> str:
    value = text(value, label, 80)
    require(SLUG.fullmatch(value) is not None, f"{label} must be a lowercase ASCII slug")
    return value


def relative(value: object, label: str, allow_dot: bool) -> str:
    value = text(value, label, 512)
    if value == ".":
        require(allow_dot, f"{label} may not be dot")
        return value
    require(
        not value.startswith("/")
        and not value.endswith("/")
        and "\\" not in value
        and "//" not in value,
        f"{label} contains unsafe separators",
    )
    require(
        all(portable_path_component(component) for component in value.split("/")),
        f"{label} contains a path component that is not portable to Windows",
    )
    return value


def portable_path_component(component: str) -> bool:
    if (
        component in {"", ".", ".."}
        or component.endswith((".", " "))
        or any(
            ord(character) < 32
            or ord(character) == 127
            or character in WINDOWS_INVALID_PATH_CHARACTERS
            for character in component
        )
    ):
        return False
    stem = component.split(".", 1)[0].upper()
    if stem in WINDOWS_RESERVED_DEVICES:
        return False
    for prefix in ("COM", "LPT"):
        if stem.startswith(prefix) and stem[len(prefix):] in {
            "1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³",
        }:
            return False
    return True


def timestamp(value: object, label: str) -> datetime:
    value = text(value, label, 64)
    require(value.endswith("Z"), f"{label} must be canonical UTC RFC 3339")
    try:
        result = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)
    except ValueError as error:
        raise ValueError(f"{label} must be canonical UTC RFC 3339") from error
    return result


def validate_artifact(value: object, label: str) -> None:
    artifact = exact_object(
        value, {"kind", "url", "byte_count", "sha256", "format", "destination"}, label
    )
    require(artifact["kind"] in KINDS, f"{label} has unknown kind")
    https(artifact["url"], f"{label} URL")
    count = artifact["byte_count"]
    require(type(count) is int and 0 < count <= 1024 * 1024 * 1024,
            f"{label} byte_count is invalid")
    if artifact["kind"] == "model":
        require(count <= 400 * 1024 * 1024, f"{label} exceeds the model limit")
    require(isinstance(artifact["sha256"], str) and SHA256.fullmatch(artifact["sha256"]) is not None,
            f"{label} SHA-256 is invalid")
    require(artifact["format"] in FORMATS, f"{label} has unknown format")
    relative(artifact["destination"], f"{label} destination", False)


def validate_recipe(value: object, label: str = "recipe") -> None:
    recipe = exact_object(
        value,
        {
            "schema", "id", "name", "version", "description", "publisher", "license",
            "homepage", "minimum_fisheye_version", "models",
        },
        label,
    )
    require(recipe["schema"] == "uci-grabber-recipe/v1", f"{label} has unsupported schema")
    slug(recipe["id"], f"{label} id")
    text(recipe["name"], f"{label} name", 256)
    require(isinstance(recipe["version"], str) and len(recipe["version"]) <= 80
            and SEMVER.fullmatch(recipe["version"]) is not None, f"{label} version is invalid")
    text(recipe["description"], f"{label} description", 4096)
    publisher = exact_object(recipe["publisher"], {"name", "url"}, f"{label} publisher")
    text(publisher["name"], f"{label} publisher name", 256)
    https(publisher["url"], f"{label} publisher URL")
    license_data = exact_object(
        recipe["license"], {"spdx", "name", "url", "source_url"}, f"{label} license"
    )
    require(isinstance(license_data["spdx"], str) and len(license_data["spdx"]) <= 128
            and SPDX.fullmatch(license_data["spdx"]) is not None, f"{label} SPDX is invalid")
    text(license_data["name"], f"{label} license name", 256)
    https(license_data["url"], f"{label} license URL")
    https(license_data["source_url"], f"{label} source URL")
    https(recipe["homepage"], f"{label} homepage")
    require(isinstance(recipe["minimum_fisheye_version"], str)
            and FISHEYE_VERSION.fullmatch(recipe["minimum_fisheye_version"]) is not None,
            f"{label} minimum FishEye version is invalid")
    models = recipe["models"]
    require(isinstance(models, list) and 1 <= len(models) <= 64, f"{label} model count is invalid")
    model_ids: set[str] = set()
    for model_index, model_value in enumerate(models):
        model_label = f"{label} model {model_index}"
        model = exact_object(model_value, {"id", "name", "description", "packages"}, model_label)
        model_id = slug(model["id"], f"{model_label} id")
        require(model_id not in model_ids, f"{label} has duplicate model id {model_id}")
        model_ids.add(model_id)
        text(model["name"], f"{model_label} name", 256)
        text(model["description"], f"{model_label} description", 4096)
        packages = model["packages"]
        require(isinstance(packages, list) and 1 <= len(packages) <= 6,
                f"{model_label} package count is invalid")
        platforms: set[str] = set()
        for package_index, package_value in enumerate(packages):
            package_label = f"{model_label} package {package_index}"
            package = exact_object(
                package_value, {"platform", "artifacts", "executable", "working_directory"},
                package_label,
            )
            require(package["platform"] in PLATFORMS, f"{package_label} has unknown platform")
            require(package["platform"] not in platforms, f"{model_label} has duplicate platform")
            platforms.add(package["platform"])
            executable = relative(package["executable"], f"{package_label} executable", False)
            working_directory = relative(
                package["working_directory"], f"{package_label} working directory", True
            )
            executable_parent = executable.rpartition("/")[0] or "."
            require(
                working_directory == executable_parent,
                f"{package_label} working directory must equal executable parent "
                f"{executable_parent}",
            )
            artifacts = package["artifacts"]
            require(isinstance(artifacts, list) and 1 <= len(artifacts) <= 16,
                    f"{package_label} artifact count is invalid")
            destinations: set[str] = set()
            kinds: list[str] = []
            download_bytes = 0
            for artifact_index, artifact in enumerate(artifacts):
                artifact_label = f"{package_label} artifact {artifact_index}"
                validate_artifact(artifact, artifact_label)
                download_bytes += artifact["byte_count"]
                require(
                    download_bytes <= MAX_PACKAGE_DOWNLOAD_BYTES,
                    f"{package_label} declares more than {MAX_PACKAGE_DOWNLOAD_BYTES} "
                    "download bytes",
                )
                destination = artifact["destination"]
                require(destination not in destinations, f"{package_label} duplicates destination")
                destinations.add(destination)
                kinds.append(artifact["kind"])
            require(kinds.count("runtime") == 1, f"{package_label} must have one runtime artifact")
            require(kinds.count("model") <= 1, f"{package_label} has multiple model artifacts")


def load_and_validate(path: Path, bootstrap: bool = False) -> dict[str, object]:
    raw = path.read_bytes()
    require(len(raw) <= MAX_MANIFEST_BYTES, "catalog exceeds 512 KiB")
    require(b"\r" not in raw and raw.endswith(b"\n"), "catalog must use LF and end in LF")
    data = json.loads(raw)
    catalog = exact_object(data, {"schema", "generated_at", "expires_at", "recipes"}, "catalog")
    require(catalog["schema"] == "uci-grabber-catalog/v1", "unsupported catalog schema")
    generated = timestamp(catalog["generated_at"], "generated_at")
    expires = timestamp(catalog["expires_at"], "expires_at")
    require(expires > generated, "catalog expiry must follow generation")
    recipes = catalog["recipes"]
    require(isinstance(recipes, list) and len(recipes) <= 1024, "catalog recipe count is invalid")
    ids: set[str] = set()
    for index, recipe in enumerate(recipes):
        validate_recipe(recipe, f"recipe {index}")
        require(recipe["id"] not in ids, f"duplicate recipe id {recipe['id']}")
        ids.add(recipe["id"])
    if bootstrap:
        require(not recipes, "the long-lived bootstrap catalog must remain empty")
    return catalog


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("catalog", type=Path)
    parser.add_argument("--bootstrap", action="store_true")
    args = parser.parse_args()
    load_and_validate(args.catalog, args.bootstrap)
    print(f"Validated {args.catalog}.")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"Catalog validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
