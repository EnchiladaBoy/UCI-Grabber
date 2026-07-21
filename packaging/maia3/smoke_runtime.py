#!/usr/bin/env python3
"""Exercise a zero-argument Maia3 launcher with a verified local checkpoint."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime-root", type=Path, required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--model", choices=("maia3-5m", "maia3-23m", "maia3-79m"), required=True)
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    metadata = json.loads((ROOT / "component-metadata.json").read_text(encoding="utf-8"))
    if args.platform not in metadata["runtimes"]:
        parser.error("unknown platform")
    if not args.runtime_root.is_dir() or not args.checkpoint.is_file():
        parser.error("runtime root and checkpoint must exist")
    runtime = metadata["runtimes"][args.platform]
    executable_relative = runtime["executable_template"].format(model=args.model)
    model_relative = runtime["model_destination_template"].format(model=args.model)
    prefix = "maia3-runtime/"
    if not executable_relative.startswith(prefix) or not model_relative.startswith(prefix):
        parser.error("reviewed paths do not begin with maia3-runtime/")

    with tempfile.TemporaryDirectory(prefix="uci-grabber-maia3-smoke-") as temporary:
        staged = Path(temporary) / "maia3-runtime"
        shutil.copytree(args.runtime_root, staged, symlinks=True)
        checkpoint = staged / model_relative.removeprefix(prefix)
        checkpoint.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(args.checkpoint, checkpoint)
        executable = staged / executable_relative.removeprefix(prefix)
        if not executable.is_file():
            parser.error(f"runtime is missing launcher: {executable}")
        environment = os.environ.copy()
        environment.update(
            {
                "HF_HUB_OFFLINE": "1",
                "HF_HUB_DISABLE_TELEMETRY": "1",
                "TRANSFORMERS_OFFLINE": "1",
                "CUDA_VISIBLE_DEVICES": "",
            }
        )
        process = subprocess.run(
            [str(executable)],
            cwd=executable.parent,
            input="uci\nisready\nposition startpos\ngo depth 1\nquit\n",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            env=environment,
            timeout=args.timeout,
            check=False,
        )
    required = ("id name Maia3", "uciok", "readyok", "bestmove ")
    missing = [token for token in required if token not in process.stdout]
    if process.returncode != 0 or missing:
        raise SystemExit(
            f"Maia3 smoke failed (exit={process.returncode}, missing={missing})\n"
            f"stdout:\n{process.stdout[-4000:]}\nstderr:\n{process.stderr[-4000:]}"
        )
    print("Maia3 completed zero-argument offline UCI readiness and depth-one search.")


if __name__ == "__main__":
    main()
