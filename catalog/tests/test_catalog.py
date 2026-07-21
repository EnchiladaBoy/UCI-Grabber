from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = ROOT.parent
MAIA_METADATA = REPOSITORY_ROOT / "packaging" / "maia3" / "component-metadata.json"
sys.path.insert(0, str(ROOT))
from validate_catalog import load_and_validate, validate_recipe  # noqa: E402
from verify_catalog import decode_signature  # noqa: E402


class CatalogTests(unittest.TestCase):
    def test_raw_signature_bytes_are_not_trimmed(self) -> None:
        leading_whitespace = b"\n" + bytes(range(63))
        trailing_whitespace = b"x" * 63 + b" "
        self.assertEqual(decode_signature(leading_whitespace), leading_whitespace)
        self.assertEqual(decode_signature(trailing_whitespace), trailing_whitespace)

    def test_bootstrap_catalog_signature_and_raw_key_match(self) -> None:
        subprocess.run(
            [
                sys.executable,
                str(ROOT / "verify_catalog.py"),
                "--catalog",
                str(ROOT / "catalog.json"),
                "--signature",
                str(ROOT / "catalog.sig"),
                "--bootstrap",
            ],
            check=True,
        )
        der = subprocess.run(
            [
                "openssl", "pkey", "-pubin", "-in", str(ROOT / "catalog-public-key.pem"),
                "-outform", "DER",
            ],
            check=True,
            stdout=subprocess.PIPE,
        ).stdout
        self.assertEqual((ROOT / "catalog.pub").read_text().strip(), der[-32:].hex())
        self.assertEqual(load_and_validate(ROOT / "catalog.json")["recipes"], [])

    def test_bootstrap_tamper_fails_authentication(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            tampered = Path(temporary) / "catalog.json"
            tampered.write_bytes((ROOT / "catalog.json").read_bytes().replace(b"recipes", b"recipez"))
            result = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "verify_catalog.py"),
                    "--catalog", str(tampered),
                    "--signature", str(ROOT / "catalog.sig"),
                    "--bootstrap",
                ],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"signature failed", result.stderr)

    def test_default_generator_is_valid_and_empty(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "catalog.json"
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "generate_catalog.py"),
                    "--assets-dir", str(root),
                    "--repository", "EnchiladaBoy/UCI-Grabber",
                    "--tag", "v1.2.3",
                    "--generated-at", "2026-07-21T00:00:00Z",
                    "--expires-at", "2026-07-21T23:59:59Z",
                    "--output", str(output),
                ],
                check=True,
            )
            catalog = load_and_validate(output)
            self.assertEqual(catalog["recipes"], [])

    def test_maia_generation_requires_exact_review_digest(self) -> None:
        metadata = json.loads(MAIA_METADATA.read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for index, runtime in enumerate(metadata["runtimes"].values()):
                (root / runtime["asset"]).write_bytes(f"runtime-{index}\n".encode())
            rejected = root / "rejected.json"
            common = [
                sys.executable,
                str(ROOT / "generate_catalog.py"),
                "--assets-dir", str(root),
                "--repository", "EnchiladaBoy/UCI-Grabber",
                "--tag", "v1.2.3",
                "--generated-at", "2026-07-21T00:00:00Z",
                "--expires-at", "2026-07-21T23:59:59Z",
                "--include-maia3",
            ]
            failed = subprocess.run(
                [*common, "--maia3-license-review-digest", "0" * 64, "--output", str(rejected)],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            self.assertNotEqual(failed.returncode, 0)
            self.assertFalse(rejected.exists())

            output = root / "reviewed.json"
            digest = hashlib.sha256(MAIA_METADATA.read_bytes()).hexdigest()
            subprocess.run(
                [*common, "--maia3-license-review-digest", digest, "--output", str(output)],
                check=True,
            )
            catalog = load_and_validate(output)
            self.assertEqual(len(catalog["recipes"]), 1)
            self.assertEqual([recipe["id"] for recipe in catalog["recipes"]], ["maia3"])
            recipe = catalog["recipes"][0]
            self.assertEqual(recipe["minimum_fisheye_version"], "1.8.0")
            self.assertEqual(
                recipe["license"],
                {
                    "spdx": "LicenseRef-Maia3-Composite-Terms",
                    "name": (
                        "Composite installation: Maia3 code under AGPL-3.0; packaged "
                        "dependencies and checkpoints retain their respective terms"
                    ),
                    "url": (
                        "https://github.com/EnchiladaBoy/UCI-Grabber/releases/download/"
                        "v1.2.3/MAIA3-NOTICES.txt"
                    ),
                    "source_url": (
                        "https://github.com/EnchiladaBoy/UCI-Grabber/releases/download/"
                        "v1.2.3/maia3-corresponding-source.tar.gz"
                    ),
                },
            )
            self.assertEqual(
                [model["id"] for model in recipe["models"]],
                ["maia3-5m", "maia3-23m", "maia3-79m"],
            )
            expected_model_urls = {
                model_id: (
                    f"https://huggingface.co/{model['repository']}/resolve/"
                    f"{model['revision']}/{model['filename']}"
                )
                for model_id, model in metadata["models"].items()
            }
            for model in recipe["models"]:
                self.assertEqual(len(model["packages"]), 4)
                for package in model["packages"]:
                    artifacts = package["artifacts"]
                    self.assertEqual(
                        [item["kind"] for item in artifacts], ["runtime", "model"]
                    )
                    runtime = metadata["runtimes"][package["platform"]]
                    self.assertEqual(
                        artifacts[0]["url"],
                        "https://github.com/EnchiladaBoy/UCI-Grabber/releases/download/"
                        f"v1.2.3/{runtime['asset']}",
                    )
                    self.assertEqual(artifacts[1]["url"], expected_model_urls[model["id"]])
                    self.assertRegex(
                        artifacts[1]["url"],
                        r"^https://huggingface\.co/[^/]+/[^/]+/resolve/[0-9a-f]{40}/[^/]+$",
                    )

    def test_shared_parity_fixture_is_valid(self) -> None:
        fixture = json.loads((ROOT / "tests" / "fixtures" / "valid-recipe.json").read_text())
        validate_recipe(fixture)

    def test_url_credentials_are_rejected(self) -> None:
        fixture = json.loads((ROOT / "tests" / "fixtures" / "valid-recipe.json").read_text())
        fixture["publisher"]["url"] = "https://user:password@example.test/publisher"
        with self.assertRaisesRegex(ValueError, "without credentials"):
            validate_recipe(fixture)
        pattern = json.loads((ROOT / "schema" / "recipe-v1.schema.json").read_text())[
            "$defs"
        ]["httpsUrl"]["pattern"]
        self.assertIsNone(re.fullmatch(pattern, fixture["publisher"]["url"]))

    def test_windows_nonportable_recipe_paths_are_rejected(self) -> None:
        rejected = [
            "runtime/engine:stream",
            "runtime/bad<name",
            "runtime/bad>name",
            'runtime/bad"name',
            "runtime/bad|name",
            "runtime/bad?name",
            "runtime/bad*name",
            "runtime/trailing.",
            "runtime/trailing ",
            "runtime/CON",
            "runtime/aux.txt",
            "runtime/CoM1.bin",
            "runtime/lPt9",
            "runtime/COM¹.log",
            "runtime/CONIN$.txt",
            "runtime/CLOCK$.cfg",
            "runtime/./engine",
            "runtime/../engine",
            "runtime/",
            "C:/engine",
        ]
        schema = json.loads((ROOT / "schema" / "recipe-v1.schema.json").read_text())
        path_schema = schema["$defs"]["relativePath"]

        for path in rejected:
            with self.subTest(path=path):
                fixture = json.loads(
                    (ROOT / "tests" / "fixtures" / "valid-recipe.json").read_text()
                )
                fixture["models"][0]["packages"][0]["artifacts"][0]["destination"] = path
                with self.assertRaisesRegex(ValueError, "not portable|unsafe separators"):
                    validate_recipe(fixture)
                self.assertFalse(
                    any(
                        branch.get("const") == path
                        or (
                            "pattern" in branch
                            and re.fullmatch(branch["pattern"], path) is not None
                        )
                        for branch in path_schema["oneOf"]
                    )
                )

    def test_package_contract_requires_executable_parent_and_cumulative_limit(self) -> None:
        fixture = json.loads((ROOT / "tests" / "fixtures" / "valid-recipe.json").read_text())
        package = fixture["models"][0]["packages"][0]
        package["working_directory"] = "."
        with self.assertRaisesRegex(ValueError, "executable parent"):
            validate_recipe(fixture)

        fixture = json.loads((ROOT / "tests" / "fixtures" / "valid-recipe.json").read_text())
        package = fixture["models"][0]["packages"][0]
        for index in range(2):
            package["artifacts"].append(
                {
                    "kind": "other",
                    "url": f"https://example.test/support-{index}",
                    "byte_count": 1024 * 1024 * 1024,
                    "sha256": str(index + 1) * 64,
                    "format": "raw",
                    "destination": f"support-{index}",
                }
            )
        with self.assertRaisesRegex(ValueError, "download bytes"):
            validate_recipe(fixture)

    def test_json_schema_documents_are_well_formed(self) -> None:
        for name in ("recipe-v1.schema.json", "catalog-v1.schema.json"):
            schema = json.loads((ROOT / "schema" / name).read_text(encoding="utf-8"))
            self.assertEqual(schema["$schema"], "https://json-schema.org/draft/2020-12/schema")
            self.assertFalse(schema["additionalProperties"])


if __name__ == "__main__":
    unittest.main()
