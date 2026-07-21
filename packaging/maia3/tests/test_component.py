from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


def load_module(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


entry = load_module("maia3_entry", "maia3_entry.py")
build_runtime = load_module("build_runtime", "build_runtime.py")
make_source_bundle = load_module("make_source_bundle", "make_source_bundle.py")
package_runtime = load_module("package_runtime", "package_runtime.py")
review_digest = load_module("review_digest", "review_digest.py")


class ComponentTests(unittest.TestCase):
    def test_checked_in_metadata_is_valid_and_gated(self) -> None:
        result = subprocess.run(
            [sys.executable, str(ROOT / "validate_metadata.py")],
            check=True,
            stdout=subprocess.PIPE,
            text=True,
        )
        self.assertIn("publication remains review-gated", result.stdout)
        metadata = json.loads((ROOT / "component-metadata.json").read_text())
        self.assertEqual(metadata["component"]["minimum_fisheye_version"], "1.8.0")

    def test_entry_rejects_arguments_before_loading_model(self) -> None:
        with self.assertRaisesRegex(SystemExit, "do not accept"):
            entry.managed_main(["--checkpoint", "anything"], Path("maia3-5m"))

    def test_entry_resolves_models_outside_macos_app(self) -> None:
        executable = Path("/tmp/maia3-runtime/UCI-Grabber-Maia3.app/Contents/MacOS/maia3-5m")
        self.assertEqual(
            entry.model_path(executable, "maia3-5m"),
            Path("/tmp/maia3-runtime/models/maia3-5m.pt"),
        )

    def test_entry_gives_each_launcher_a_distinct_uci_name(self) -> None:
        for model, expected in (
            ("maia3-5m", "id name Maia3 5M\n"),
            ("maia3-23m", "id name Maia3 23M\n"),
            ("maia3-79m", "id name Maia3 79M\n"),
        ):
            output = io.StringIO()
            proxy = entry.VariantNameOutput(output, model)
            proxy.write("id name Maia3\n")
            proxy.flush()
            self.assertEqual(output.getvalue(), expected)

    def test_entry_rejects_wrong_checkpoint_size(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "maia3-5m"
            executable.write_bytes(b"launcher")
            models = root / "models"
            models.mkdir()
            (models / "maia3-5m.pt").write_bytes(b"wrong")
            with self.assertRaisesRegex(SystemExit, "size mismatch"):
                entry.managed_main([], executable)

    def test_runtime_and_release_notices_locate_same_release_source(self) -> None:
        metadata = json.loads((ROOT / "component-metadata.json").read_text(encoding="utf-8"))
        source_asset = metadata["component"]["corresponding_source_asset"]
        runtime_notice = build_runtime.corresponding_source_notice(source_asset)
        release_notice = make_source_bundle.notice_text(
            metadata, "a" * 64, "fixture AGPL text\n"
        )
        for notice in (runtime_notice, release_notice):
            normalized = " ".join(notice.lower().split())
            self.assertIn(source_asset, normalized)
            self.assertIn("same", normalized)
            self.assertIn("release", normalized)
            self.assertIn("without additional charge", normalized)
            self.assertIn("not a legal", normalized)
        self.assertIn("UCI Grabber does not relicense them", release_notice)
        for model in metadata["models"].values():
            self.assertIn(model["revision"], release_notice)
            self.assertIn(f"/{model['revision']}/README.md", release_notice)
            self.assertIn(model["sha256"], release_notice)

    def test_zip_runtime_packaging_is_reproducible(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / "stage"
            runtime = stage / "maia3-runtime"
            (runtime / "models").mkdir(parents=True)
            executable = runtime / "maia3-5m.exe"
            executable.write_bytes(b"fixture executable\n")
            output_one = root / "one.zip"
            output_two = root / "two.zip"
            paths = package_runtime.members(stage)
            package_runtime.create_zip(stage, output_one, paths)
            package_runtime.create_zip(stage, output_two, paths)
            self.assertEqual(hashlib.sha256(output_one.read_bytes()).digest(),
                             hashlib.sha256(output_two.read_bytes()).digest())
            with zipfile.ZipFile(output_one) as archive:
                self.assertEqual(archive.namelist(), sorted(archive.namelist()))
                self.assertIn("maia3-runtime/models/", archive.namelist())

    @unittest.skipUnless(hasattr(os, "symlink"), "symlinks are unavailable")
    def test_runtime_packaging_rejects_escaping_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / "stage"
            runtime = stage / "maia3-runtime"
            runtime.mkdir(parents=True)
            (root / "outside").write_bytes(b"outside")
            os.symlink("../../outside", runtime / "escape")
            with self.assertRaisesRegex(ValueError, "escapes runtime root"):
                package_runtime.members(stage)

    @unittest.skipUnless(hasattr(os, "symlink"), "symlinks are unavailable")
    def test_runtime_packaging_rejects_broken_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary) / "stage"
            runtime = stage / "maia3-runtime"
            runtime.mkdir(parents=True)
            os.symlink("missing", runtime / "broken")
            with self.assertRaisesRegex(ValueError, "broken symlink"):
                package_runtime.members(stage)

    @unittest.skipUnless(hasattr(os, "symlink"), "symlinks are unavailable")
    def test_tar_flattens_safe_file_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / "stage"
            runtime = stage / "maia3-runtime"
            runtime.mkdir(parents=True)
            target = runtime / "library.so.1"
            target.write_bytes(b"shared library fixture")
            target.chmod(0o755)
            os.symlink("library.so.1", runtime / "library.so")
            archive_path = root / "runtime.tar"
            package_runtime.create_tar(stage, archive_path, package_runtime.members(stage))
            with tarfile.open(archive_path) as archive:
                self.assertTrue(all(member.isdir() or member.isreg() for member in archive))
                alias = archive.getmember("maia3-runtime/library.so")
                self.assertTrue(alias.isreg())
                self.assertEqual(archive.extractfile(alias).read(), b"shared library fixture")

    @unittest.skipUnless(hasattr(os, "symlink"), "symlinks are unavailable")
    def test_runtime_packaging_rejects_directory_symlinks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary) / "stage"
            runtime = stage / "maia3-runtime"
            (runtime / "real-directory").mkdir(parents=True)
            os.symlink("real-directory", runtime / "directory-link")
            with self.assertRaisesRegex(ValueError, "directory symlinks"):
                package_runtime.members(stage)

    def test_wheel_lock_hashes_every_input(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            wheelhouse = root / "wheels"
            wheelhouse.mkdir()
            (wheelhouse / "alpha-1.0-py3-none-any.whl").write_bytes(b"alpha")
            (wheelhouse / "beta_pkg-2.0-py3-none-any.whl").write_bytes(b"beta")
            requirements = root / "requirements.txt"
            inventory = root / "inventory.json"
            command = [
                sys.executable,
                str(ROOT / "build_wheel_lock.py"),
                "--wheelhouse", str(wheelhouse),
                "--platform", "linux-x86_64",
            ]
            subprocess.run(
                [*command, "--requirements", str(requirements), "--inventory", str(inventory)],
                check=True,
            )
            lines = requirements.read_text().splitlines()
            self.assertEqual(len(lines), 2)
            self.assertTrue(all("--hash=sha256:" in line for line in lines))
            data = json.loads(inventory.read_text())
            self.assertEqual(data["schema"], 2)
            self.assertEqual(data["platform"], "linux-x86_64")
            self.assertEqual(data["python"], "CPython 3.12.10")
            self.assertEqual(len(data["files"]), 2)
            second_requirements = root / "requirements-second.txt"
            second_inventory = root / "inventory-second.json"
            subprocess.run(
                [
                    *command,
                    "--requirements", str(second_requirements),
                    "--inventory", str(second_inventory),
                ],
                check=True,
            )
            self.assertEqual(requirements.read_bytes(), second_requirements.read_bytes())
            self.assertEqual(inventory.read_bytes(), second_inventory.read_bytes())

    def test_release_review_digests_fail_closed(self) -> None:
        source_digest = review_digest.source_review_digest()
        self.assertRegex(source_digest, r"^[0-9a-f]{64}$")
        review_digest.verify_expected(source_digest, source_digest, "source")
        with self.assertRaisesRegex(ValueError, "stale"):
            review_digest.verify_expected(source_digest, "0" * 64, "source")
        wheelhouses = {
            platform: hashlib.sha256(platform.encode()).hexdigest()
            for platform in review_digest.WHEELHOUSE_PLATFORMS
        }
        release_digest = review_digest.source_release_review_digest(wheelhouses)
        self.assertRegex(release_digest, r"^[0-9a-f]{64}$")
        changed = dict(wheelhouses)
        changed["linux-x86_64"] = "f" * 64
        self.assertNotEqual(
            release_digest,
            review_digest.source_release_review_digest(changed),
        )
        with self.assertRaisesRegex(ValueError, "all four"):
            review_digest.source_release_review_digest({"linux-x86_64": "a" * 64})
        with tempfile.TemporaryDirectory() as temporary:
            fixture = Path(temporary) / "reviewed"
            fixture.write_bytes(b"reviewed bytes")
            digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
            review_digest.verify(fixture, digest, "fixture")
            with self.assertRaisesRegex(ValueError, "stale"):
                review_digest.verify(fixture, "0" * 64, "fixture")
            with self.assertRaisesRegex(ValueError, "missing"):
                review_digest.verify(fixture, "", "fixture")

    def test_every_published_packaging_source_is_review_bound(self) -> None:
        self.assertEqual(
            set(make_source_bundle.INCLUDED_PACKAGING_FILES),
            set(review_digest.SOURCE_REVIEW_FILES),
        )
        self.assertEqual(
            set(make_source_bundle.INCLUDED_REPOSITORY_FILES),
            set(review_digest.SOURCE_REVIEW_REPOSITORY_FILES),
        )

    def test_release_workflow_reviews_every_wheelhouse_before_install(self) -> None:
        workflow = (ROOT.parents[1] / ".github" / "workflows" / "release.yml").read_text()
        self.assertLess(
            workflow.index("Require the reviewed wheelhouse before installation"),
            workflow.index("Install only the reviewed wheelhouse"),
        )
        for variable in (
            "MAIA3_WHEELHOUSE_REVIEW_WINDOWS_X86_64",
            "MAIA3_WHEELHOUSE_REVIEW_MACOS_AARCH64",
            "MAIA3_WHEELHOUSE_REVIEW_LINUX_X86_64",
            "MAIA3_WHEELHOUSE_REVIEW_LINUX_AARCH64",
            "MAIA3_CORRESPONDING_SOURCE_REVIEW",
        ):
            self.assertIn(variable, workflow)
        candidate_workflow = (
            ROOT.parents[1] / ".github" / "workflows" / "maia-wheelhouse-review.yml"
        ).read_text()
        self.assertIn("python -m pip download", candidate_workflow)
        self.assertNotIn("python -m pip install", candidate_workflow)

    def test_verify_file_detects_tamper(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fixture = Path(temporary) / "fixture"
            fixture.write_bytes(b"verified")
            digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
            command = [
                sys.executable, str(ROOT / "verify_file.py"), str(fixture),
                "--bytes", str(fixture.stat().st_size), "--sha256", digest,
            ]
            subprocess.run(command, check=True)
            fixture.write_bytes(b"tampered")
            self.assertNotEqual(subprocess.run(command, check=False).returncode, 0)


if __name__ == "__main__":
    unittest.main()
