"""Zero-argument, fail-closed entry point for packaged Maia3 UCI engines."""

from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path
from typing import Mapping


MODELS = {
    "maia3-5m": (20_968_049, "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f"),
    "maia3-23m": (91_799_307, "bce6cd1af5f0399ac7eed33fabb7a6a2ef6193662c2740f262bf93af7bfb3569"),
    "maia3-79m": (315_651_851, "3fc6181d5db789b45a15305732148757ae74efa3e0028e81ba335b462dac45c2"),
}
MAIA3_COMMIT = "1e13597c42d4858b7cfd7cfdae01e297263364b2"
DIRECT_ENVIRONMENT = ("UCI_GRABBER_MODEL", "UCI_GRABBER_INSTALL_ROOT")


class VariantNameOutput:
    """Rewrite only Maia3's UCI identity while forwarding the wrapped stream API."""

    def __init__(self, stream, model: str) -> None:
        self.stream = stream
        self.name = f"Maia3 {model.removeprefix('maia3-').upper()}"

    def write(self, text: str) -> int:
        return self.stream.write(text.replace("id name Maia3", f"id name {self.name}"))

    def flush(self) -> None:
        self.stream.flush()

    def __getattr__(self, name: str):
        return getattr(self.stream, name)


def sha256(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def model_path(executable: Path, model: str) -> Path:
    # macOS launchers live in Foo.app/Contents/MacOS; models deliberately live
    # outside the app bundle so it stays self-contained and replaceable.
    if executable.parent.name == "MacOS" and executable.parent.parent.name == "Contents":
        runtime_root = executable.parent.parent.parent.parent
    else:
        runtime_root = executable.parent
    return runtime_root / "models" / f"{model}.pt"


def _resolved_directory(path: Path, description: str) -> Path:
    if path.is_symlink() or not path.is_dir():
        raise SystemExit(f"{description} is missing or is not a real directory: {path}")
    try:
        return path.resolve(strict=True)
    except OSError as error:
        raise SystemExit(f"could not resolve {description}: {path}: {error}") from error


def direct_configuration(
    launcher_directory: Path,
    environment: Mapping[str, str] | None = None,
) -> tuple[str, Path, Path] | None:
    """Validate direct-download layout and return model, checkpoint, and root."""

    values = os.environ if environment is None else environment
    present = [name in values for name in DIRECT_ENVIRONMENT]
    if not any(present):
        return None
    if not all(present):
        raise SystemExit("incomplete UCI Grabber direct-download environment")

    model = values["UCI_GRABBER_MODEL"]
    if model not in MODELS:
        raise SystemExit("UCI_GRABBER_MODEL must be maia3-5m, maia3-23m, or maia3-79m")
    root_value = values["UCI_GRABBER_INSTALL_ROOT"]
    root_path = Path(root_value)
    if not root_value or not root_path.is_absolute():
        raise SystemExit("UCI_GRABBER_INSTALL_ROOT must be an absolute directory")
    root = _resolved_directory(root_path, "UCI Grabber install root")
    launcher = _resolved_directory(launcher_directory, "Maia3 launcher directory")
    if launcher.name != "launcher" or launcher.parent != root:
        raise SystemExit("Maia3 launcher must be in the install root's launcher directory")

    packages = _resolved_directory(root / "packages", "Python package directory")
    package_roots: list[Path] = []
    try:
        package_members = sorted(packages.iterdir(), key=lambda path: path.name)
    except OSError as error:
        raise SystemExit(f"could not inspect Python package directory: {error}") from error
    for member in package_members:
        if member.is_symlink() or not member.is_dir():
            raise SystemExit(f"unexpected Python package path: {member}")
        resolved = _resolved_directory(member, "Python package directory")
        if resolved.parent != packages:
            raise SystemExit(f"Python package directory escapes its parent: {member}")
        package_roots.append(resolved)
    if not package_roots:
        raise SystemExit("Python package directory contains no package roots")

    maia_source_parent = _resolved_directory(root / "maia-source", "Maia3 source parent")
    maia_source = _resolved_directory(
        maia_source_parent / f"maia3-{MAIA3_COMMIT}",
        "reviewed Maia3 source directory",
    )
    if maia_source.parent != maia_source_parent:
        raise SystemExit("reviewed Maia3 source directory escapes its parent")
    chess_source_parent = _resolved_directory(root / "chess-source", "chess source parent")
    chess_source = _resolved_directory(
        chess_source_parent / "chess-1.11.2",
        "reviewed chess source directory",
    )
    if chess_source.parent != chess_source_parent:
        raise SystemExit("reviewed chess source directory escapes its parent")
    import_roots = [maia_source, chess_source, *package_roots]
    sys.path[:0] = [str(path) for path in import_roots if str(path) not in sys.path]

    checkpoint = launcher / "models" / f"{model}.pt"
    if checkpoint.is_symlink() or not checkpoint.is_file():
        raise SystemExit(f"Maia3 checkpoint is missing or is not a regular file: {checkpoint}")
    try:
        checkpoint = checkpoint.resolve(strict=True)
        checkpoint.relative_to(launcher)
    except (OSError, ValueError) as error:
        raise SystemExit("Maia3 checkpoint is outside the launcher directory") from error
    return model, checkpoint, root


def managed_main(arguments: list[str] | None = None, executable: Path | None = None) -> None:
    args = list(sys.argv[1:] if arguments is None else arguments)
    if args:
        raise SystemExit("packaged Maia3 launchers do not accept command-line arguments")
    direct = direct_configuration(Path(__file__).resolve().parent)
    if direct is None:
        executable = Path(sys.executable if executable is None else executable).resolve()
        model = executable.stem.lower()
        if model not in MODELS:
            raise SystemExit("Maia3 runtime must be launched as maia3-5m, maia3-23m, or maia3-79m")
        checkpoint = model_path(executable, model)
    else:
        model, checkpoint, _install_root = direct
    expected_bytes, expected_digest = MODELS[model]
    if checkpoint.is_symlink() or not checkpoint.is_file():
        raise SystemExit(f"Maia3 checkpoint is missing: {checkpoint}")
    actual_bytes = checkpoint.stat().st_size
    if actual_bytes != expected_bytes:
        raise SystemExit(
            f"Maia3 checkpoint size mismatch: expected {expected_bytes}, found {actual_bytes}"
        )
    actual_digest = sha256(checkpoint)
    if actual_digest != expected_digest:
        raise SystemExit("Maia3 checkpoint SHA-256 does not match the reviewed recipe")

    os.environ.update(
        {
            "HF_HUB_OFFLINE": "1",
            "HF_HUB_DISABLE_TELEMETRY": "1",
            "TRANSFORMERS_OFFLINE": "1",
            "CUDA_VISIBLE_DEVICES": "",
        }
    )
    from maia3.uci import main as maia_main

    original_stdout = sys.stdout
    sys.stdout = VariantNameOutput(original_stdout, model)
    try:
        maia_main(
            [
                "--model",
                model,
                "--checkpoint-path",
                str(checkpoint),
                "--device",
                "cpu",
                "--no-use-amp",
                "--local-files-only",
            ]
        )
    finally:
        sys.stdout = original_stdout


if __name__ == "__main__":
    managed_main()
