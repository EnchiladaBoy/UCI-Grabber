from __future__ import annotations

import hashlib
import importlib.util
import io
import os
import stat
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("package_app", ROOT / "package_app.py")
assert spec is not None and spec.loader is not None
package_app = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_app)


class ApplicationPackagingTests(unittest.TestCase):
    def inputs(self, root: Path) -> tuple[Path, Path, Path, Path]:
        binary = root / "uci-grabber"
        license_file = root / "LICENSE"
        readme = root / "README.md"
        notices = root / "THIRD-PARTY-LICENSES.txt"
        binary.write_bytes(b"native fixture\n")
        license_file.write_text("Apache-2.0 fixture\n")
        readme.write_text("UCI Grabber fixture\n")
        notices.write_text("Dependency notices fixture\n")
        return binary, license_file, readme, notices

    def test_windows_zip_is_reproducible_and_complete(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = self.inputs(root)
            gui = root / "uci-grabber-gui.exe"
            gui.write_bytes(b"native GUI fixture\n")
            files = package_app.payload("windows-x86_64", "1.2.3", *inputs, gui)
            first, second = root / "first.zip", root / "second.zip"
            package_app.create_zip(first, files)
            package_app.create_zip(second, files)
            self.assertEqual(hashlib.sha256(first.read_bytes()).digest(),
                             hashlib.sha256(second.read_bytes()).digest())
            with zipfile.ZipFile(first) as archive:
                names = archive.namelist()
                self.assertIn("uci-grabber-1.2.3/UCI-Grabber.exe", names)
                self.assertIn("uci-grabber-1.2.3/uci-grabber-cli.exe", names)
                self.assertIn("uci-grabber-1.2.3/THIRD-PARTY-LICENSES.txt", names)
                self.assertIn("uci-grabber-1.2.3/portable.flag", names)

    def test_linux_tar_is_reproducible_and_marks_binary_executable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = self.inputs(root)
            files = package_app.payload("linux-aarch64", "1.2.3", *inputs)
            first, second = root / "first.tar.gz", root / "second.tar.gz"
            package_app.create_tar_gz(first, files)
            package_app.create_tar_gz(second, files)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with tarfile.open(first, "r:gz") as archive:
                binary = archive.getmember("uci-grabber-1.2.3/uci-grabber")
                self.assertTrue(binary.mode & stat.S_IXUSR)

    def test_macos_bundle_has_plist_and_external_notices(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = package_app.payload("macos-aarch64", "1.2.3", *self.inputs(root))
            names = {name for name, _contents, _mode in files}
            self.assertIn("UCI Grabber.app/Contents/Info.plist", names)
            self.assertIn("UCI Grabber.app/Contents/MacOS/uci-grabber", names)
            self.assertIn(
                "UCI Grabber.app/Contents/Resources/THIRD-PARTY-LICENSES.txt", names
            )
            self.assertIn("UCI Grabber.app/Contents/Resources/portable.flag", names)

    def test_packaged_readme_pins_repository_links_to_release_tag(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inputs = self.inputs(root)
            inputs[2].write_text(
                "![Hero](docs/assets/hero.webp)\n"
                "[Security](docs/SECURITY.md)\n"
                "[License](LICENSE)\n",
                encoding="utf-8",
            )
            files = package_app.payload("linux-x86_64", "1.2.3", *inputs)
            readme = next(
                contents for name, contents, _mode in files if name.endswith("/README.md")
            ).decode("utf-8")
            self.assertIn(
                "https://github.com/EnchiladaBoy/UCI-Grabber/"
                "blob/v1.2.3/docs/SECURITY.md",
                readme,
            )
            self.assertIn(
                "https://raw.githubusercontent.com/EnchiladaBoy/"
                "UCI-Grabber/v1.2.3/docs/assets/hero.webp",
                readme,
            )
            self.assertIn("[License](LICENSE)", readme)


if __name__ == "__main__":
    unittest.main()
