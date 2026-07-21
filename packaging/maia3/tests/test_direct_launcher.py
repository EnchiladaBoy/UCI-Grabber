from __future__ import annotations

import hashlib
import importlib.util
import os
import stat
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def load_module(name: str, filename: str):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


entry = load_module("direct_maia3_entry", "maia3_entry.py")
package_launcher = load_module("package_launcher", "package_launcher.py")


class DirectLauncherTests(unittest.TestCase):
    def make_layout(self, root: Path, model: str = "maia3-5m") -> Path:
        launcher = root / "launcher"
        (launcher / "models").mkdir(parents=True)
        (launcher / "models" / f"{model}.pt").write_bytes(b"reviewed fixture")
        (root / "packages" / "torch-wheel").mkdir(parents=True)
        (root / "packages" / "numpy-wheel").mkdir()
        (root / "maia-source" / f"maia3-{entry.MAIA3_COMMIT}").mkdir(parents=True)
        (root / "chess-source" / "chess-1.11.2").mkdir(parents=True)
        return launcher

    def test_direct_layout_adds_only_reviewed_import_roots(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            launcher = self.make_layout(root)
            original_path = list(sys.path)
            try:
                configured = entry.direct_configuration(
                    launcher,
                    {
                        "UCI_GRABBER_MODEL": "maia3-5m",
                        "UCI_GRABBER_INSTALL_ROOT": str(root),
                    },
                )
                self.assertIsNotNone(configured)
                model, checkpoint, configured_root = configured
                self.assertEqual(model, "maia3-5m")
                self.assertEqual(checkpoint, launcher / "models" / "maia3-5m.pt")
                self.assertEqual(configured_root, root)
                expected = [
                    root / "maia-source" / f"maia3-{entry.MAIA3_COMMIT}",
                    root / "chess-source" / "chess-1.11.2",
                    root / "packages" / "numpy-wheel",
                    root / "packages" / "torch-wheel",
                ]
                self.assertEqual(sys.path[:4], [str(path) for path in expected])
            finally:
                sys.path[:] = original_path

    def test_direct_layout_rejects_partial_environment_and_bad_model(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            launcher = self.make_layout(root)
            with self.assertRaisesRegex(SystemExit, "incomplete"):
                entry.direct_configuration(launcher, {"UCI_GRABBER_MODEL": "maia3-5m"})
            with self.assertRaisesRegex(SystemExit, "UCI_GRABBER_MODEL"):
                entry.direct_configuration(
                    launcher,
                    {
                        "UCI_GRABBER_MODEL": "maia3-unknown",
                        "UCI_GRABBER_INSTALL_ROOT": str(root),
                    },
                )

    def test_direct_layout_rejects_launcher_outside_exact_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            launcher = self.make_layout(root)
            renamed = root / "other-launcher"
            launcher.rename(renamed)
            with self.assertRaisesRegex(SystemExit, "install root's launcher"):
                entry.direct_configuration(
                    renamed,
                    {
                        "UCI_GRABBER_MODEL": "maia3-5m",
                        "UCI_GRABBER_INSTALL_ROOT": str(root),
                    },
                )

    @unittest.skipUnless(hasattr(os, "symlink"), "symlinks are unavailable")
    def test_direct_layout_rejects_escaping_package_symlink(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            launcher = self.make_layout(root)
            outside = root / "outside"
            outside.mkdir()
            os.symlink(outside, root / "packages" / "escape")
            with self.assertRaisesRegex(SystemExit, "unexpected Python package path"):
                entry.direct_configuration(
                    launcher,
                    {
                        "UCI_GRABBER_MODEL": "maia3-5m",
                        "UCI_GRABBER_INSTALL_ROOT": str(root),
                    },
                )

    def fixture_inputs(self, root: Path) -> tuple[Path, Path, Path, Path, Path]:
        binary = root / "launcher-build"
        binary.write_bytes(
            b"launcher fixture\n"
            + package_launcher.PAYLOAD_REVIEW_PLACEHOLDER
            + b"\nlauncher suffix\n"
        )
        binary.chmod(0o755)
        entry_point = root / "entry.py"
        entry_point.write_bytes(b"print('fixture')\n")
        license_file = root / "LICENSE.input"
        license_file.write_bytes(b"Apache License fixture\n")
        notices = root / "notices.input"
        notices.write_bytes(b"Direct download fixture\n")
        third_party = root / "third-party.input"
        third_party.write_bytes(b"Rust dependency license fixture\n")
        return binary, entry_point, license_file, notices, third_party

    def test_windows_launcher_zip_is_reproducible_and_flat(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = self.fixture_inputs(root)
            files = package_launcher.payload("windows-x86_64", *inputs)
            one = root / "one.zip"
            two = root / "two.zip"
            package_launcher.create_zip(one, files)
            package_launcher.create_zip(two, files)
            self.assertEqual(hashlib.sha256(one.read_bytes()).digest(), hashlib.sha256(two.read_bytes()).digest())
            with zipfile.ZipFile(one) as archive:
                self.assertEqual(
                    archive.namelist(),
                    [
                        "DIRECT-DOWNLOAD-NOTICES.txt",
                        "LICENSE",
                        "THIRD-PARTY-LICENSES.txt",
                        "maia3-launcher.exe",
                        "maia3_entry.py",
                    ],
                )
                launcher = archive.getinfo("maia3-launcher.exe")
                self.assertEqual((launcher.external_attr >> 16) & 0o777, 0o755)

    def test_unix_launcher_tar_gz_is_reproducible_and_flat(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = self.fixture_inputs(root)
            files = package_launcher.payload("linux-x86_64", *inputs)
            one = root / "one.tar.gz"
            two = root / "two.tar.gz"
            package_launcher.create_tar_gz(one, files)
            package_launcher.create_tar_gz(two, files)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            with tarfile.open(one) as archive:
                members = archive.getmembers()
                self.assertEqual(
                    [member.name for member in members],
                    [
                        "DIRECT-DOWNLOAD-NOTICES.txt",
                        "LICENSE",
                        "THIRD-PARTY-LICENSES.txt",
                        "maia3-launcher",
                        "maia3_entry.py",
                    ],
                )
                self.assertTrue(all(member.isfile() and not member.issym() for member in members))
                launcher = archive.getmember("maia3-launcher")
                self.assertEqual(stat.S_IMODE(launcher.mode), 0o755)


if __name__ == "__main__":
    unittest.main()
