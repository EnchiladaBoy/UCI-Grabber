from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = ROOT.parents[1]
sys.path.insert(0, str(ROOT))


def load_module(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


entry = load_module("reviewed_maia3_entry", "maia3_entry.py")
review_digest = load_module("direct_review_digest", "review_digest.py")
validate_metadata = load_module("direct_validate_metadata", "validate_metadata.py")


class ComponentTests(unittest.TestCase):
    def test_checked_metadata_and_direct_manifest_are_valid(self) -> None:
        metadata = validate_metadata.validate()
        direct = validate_metadata.validate_direct_downloads()
        self.assertEqual(metadata["component"]["minimum_fisheye_version"], "1.8.0")
        self.assertEqual(direct["python"]["version"], "3.12.13")
        self.assertEqual(direct["packages"]["required"], list(validate_metadata.RUNTIME_PACKAGES))
        for platform in validate_metadata.EXPECTED_PLATFORMS:
            self.assertEqual(len(direct["packages"]["common"]), 8)
            self.assertEqual(len(direct["packages"]["platforms"][platform]), 3)

    def test_upstream_bytes_are_direct_and_not_uci_grabber_release_assets(self) -> None:
        direct = validate_metadata.validate_direct_downloads()
        urls = [artifact["url"] for artifact in direct["python"]["platforms"].values()]
        urls.extend(artifact["url"] for artifact in direct["sources"].values())
        urls.extend(artifact["url"] for artifact in direct["packages"]["common"].values())
        for variants in direct["packages"]["platforms"].values():
            urls.extend(artifact["url"] for artifact in variants.values())
        self.assertTrue(urls)
        self.assertFalse(any("EnchiladaBoy/UCI-Grabber/releases" in url for url in urls))
        self.assertTrue(all(url.startswith("https://") for url in urls))

    def test_entry_rejects_arguments_before_loading_a_model(self) -> None:
        with self.assertRaisesRegex(SystemExit, "do not accept"):
            entry.managed_main(["--checkpoint", "anything"], Path("maia3-5m"))

    def test_entry_rewrites_only_the_uci_identity(self) -> None:
        class Output:
            def __init__(self) -> None:
                self.value = ""

            def write(self, value: str) -> int:
                self.value += value
                return len(value)

            def flush(self) -> None:
                return None

        for model, expected in (
            ("maia3-5m", "id name Maia3 5M\n"),
            ("maia3-23m", "id name Maia3 23M\n"),
            ("maia3-79m", "id name Maia3 79M\n"),
        ):
            output = Output()
            entry.VariantNameOutput(output, model).write("id name Maia3\n")
            self.assertEqual(output.value, expected)

    def test_checked_release_review_verifies_and_rejects_stale_inputs(self) -> None:
        review_digest.verify_release("v0.2.0")
        checked = json.loads(review_digest.RELEASE_REVIEW_PATH.read_text(encoding="utf-8"))
        checked["release_inputs_sha256"] = "0" * 64
        with tempfile.TemporaryDirectory() as temporary:
            review_path = Path(temporary) / "release-review.json"
            review_path.write_text(json.dumps(checked), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "direct-release inputs review is stale"):
                review_digest.verify_release("v0.2.0", review_path)

    def test_direct_notice_records_checkpoint_ambiguity_and_scope(self) -> None:
        notice = (ROOT / "DIRECT-DOWNLOAD-NOTICES.txt").read_text(encoding="utf-8")
        self.assertIn("direct end-user retrieval", notice)
        self.assertGreaterEqual(notice.count("no independent weights"), 2)
        self.assertIn("The reviewed model card states AGPLv3", notice)
        self.assertIn("does not relicense", notice)
        self.assertIn("not legal advice", notice)

    def test_obsolete_frozen_runtime_publication_path_is_retired(self) -> None:
        for relative in (
            ".github/workflows/maia-wheelhouse-review.yml",
            "packaging/maia3/build_runtime.py",
            "packaging/maia3/corresponding-source-policy.json",
            "packaging/maia3/maia3.spec",
            "packaging/maia3/make_source_bundle.py",
            "packaging/maia3/package_runtime.py",
            "packaging/maia3/wheelhouse-review-candidate",
        ):
            self.assertFalse((REPOSITORY_ROOT / relative).exists(), relative)
        workflow = (REPOSITORY_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        self.assertNotIn("maia3-corresponding-source", workflow)
        self.assertNotIn("uci-grabber-maia3-runtime-", workflow)
        self.assertIn("uci-grabber-maia3-launcher-", workflow)
        self.assertIn("overwrite_files: true", workflow)
        self.assertIn("persist-credentials: false", workflow)


if __name__ == "__main__":
    unittest.main()
