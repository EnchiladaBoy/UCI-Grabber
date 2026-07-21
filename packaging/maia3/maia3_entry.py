"""Zero-argument, fail-closed entry point for packaged Maia3 UCI engines."""

from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path


MODELS = {
    "maia3-5m": (20_968_049, "ba14208b2992d85502f5fb501934abf6aaaeb355e9f3fdf90e326911f562524f"),
    "maia3-23m": (91_799_307, "bce6cd1af5f0399ac7eed33fabb7a6a2ef6193662c2740f262bf93af7bfb3569"),
    "maia3-79m": (315_651_851, "3fc6181d5db789b45a15305732148757ae74efa3e0028e81ba335b462dac45c2"),
}


def sha256(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def model_path(executable: Path, model: str) -> Path:
    # macOS launchers live in Foo.app/Contents/MacOS; models deliberately live
    # outside the signed app bundle so installation cannot invalidate signing.
    if executable.parent.name == "MacOS" and executable.parent.parent.name == "Contents":
        runtime_root = executable.parent.parent.parent.parent
    else:
        runtime_root = executable.parent
    return runtime_root / "models" / f"{model}.pt"


def managed_main(arguments: list[str] | None = None, executable: Path | None = None) -> None:
    args = list(sys.argv[1:] if arguments is None else arguments)
    if args:
        raise SystemExit("packaged Maia3 launchers do not accept command-line arguments")
    executable = Path(sys.executable if executable is None else executable).resolve()
    model = executable.stem.lower()
    if model not in MODELS:
        raise SystemExit("Maia3 runtime must be launched as maia3-5m, maia3-23m, or maia3-79m")
    checkpoint = model_path(executable, model)
    expected_bytes, expected_digest = MODELS[model]
    if not checkpoint.is_file():
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


if __name__ == "__main__":
    managed_main()
