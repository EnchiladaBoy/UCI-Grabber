#!/usr/bin/env python3
"""Exercise a personalized Maia3 launcher using a synthetic Python binary."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import os
import shutil
import stat
import struct
import subprocess
import sys
import time
from pathlib import Path


MARKER = b"\0UCI_GRABBER_PAYLOAD_REVIEW_V1\0"
PLACEHOLDER = MARKER + b"X" * 64 + b"X" * 16 + b"X" * 16


def package_snapshot(root: Path, excluded: Path) -> tuple[str, int, int]:
    files: list[tuple[str, Path, int]] = []
    total = 0
    for path in root.rglob("*"):
        if path.is_symlink():
            raise ValueError(f"synthetic package contains a link: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"synthetic package contains a special file: {path}")
        if path == excluded:
            continue
        relative = path.relative_to(root).as_posix()
        length = path.stat().st_size
        total += length
        files.append((relative, path, length))

    digest = hashlib.sha256()
    for relative, path, length in sorted(files):
        relative_bytes = relative.encode("utf-8")
        file_digest = hashlib.sha256(path.read_bytes()).hexdigest().encode("ascii")
        digest.update(struct.pack("<Q", len(relative_bytes)))
        digest.update(relative_bytes)
        digest.update(struct.pack("<Q", length))
        digest.update(file_digest)
    return digest.hexdigest(), len(files), total


def personalize(package: Path, launcher: Path) -> None:
    digest, file_count, byte_count = package_snapshot(package, launcher)
    review = (
        MARKER
        + digest.encode("ascii")
        + f"{file_count:016d}".encode("ascii")
        + f"{byte_count:016d}".encode("ascii")
    )
    contents = launcher.read_bytes()
    if contents.count(PLACEHOLDER) != 1:
        raise ValueError("launcher does not contain exactly one payload placeholder")
    launcher.write_bytes(contents.replace(PLACEHOLDER, review))


def launcher_is_running(pid: int) -> bool:
    if sys.platform != "win32":
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return False
        except PermissionError:
            return True
        return True

    synchronize = 0x00100000
    wait_timeout = 0x00000102
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    handle = kernel32.OpenProcess(synchronize, False, pid)
    if not handle:
        return False
    try:
        return kernel32.WaitForSingleObject(handle, 0) == wait_timeout
    finally:
        kernel32.CloseHandle(handle)


def require_child_termination(launcher: Path, package: Path) -> None:
    environment = os.environ.copy()
    environment["UCI_GRABBER_SMOKE_WAIT"] = "1"
    process = subprocess.Popen(
        [launcher],
        cwd=package,
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    assert process.stdout is not None
    line = process.stdout.readline().strip()
    if not line.startswith("SMOKE_PID="):
        _, stderr = process.communicate(timeout=10)
        raise RuntimeError(f"synthetic child did not report its PID: {line!r}; {stderr}")
    child_pid = int(line.removeprefix("SMOKE_PID="))
    process.terminate()
    process.wait(timeout=10)
    deadline = time.monotonic() + 10
    while launcher_is_running(child_pid) and time.monotonic() < deadline:
        time.sleep(0.05)
    if launcher_is_running(child_pid):
        raise RuntimeError(f"synthetic child {child_pid} survived launcher termination")


def make_read_only(package: Path, launcher: Path, fake_python: Path) -> None:
    if os.name == "nt":
        return
    for path in package.rglob("*"):
        if path.is_file():
            mode = 0o555 if path in {launcher, fake_python} else 0o444
            path.chmod(mode)
    for path in sorted(
        (item for item in package.rglob("*") if item.is_dir()),
        key=lambda item: len(item.parts),
        reverse=True,
    ):
        path.chmod(0o555)
    package.chmod(0o555)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", required=True)
    parser.add_argument("--launcher", type=Path, required=True)
    parser.add_argument("--fake-python", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()

    if args.work.exists():
        raise SystemExit(f"smoke directory already exists: {args.work}")
    package = args.work / "package"
    launcher_directory = package / "launcher"
    launcher_directory.mkdir(parents=True)
    windows = args.platform == "windows-x86_64"
    launcher = launcher_directory / ("maia3-launcher.exe" if windows else "maia3-launcher")
    fake_python = (
        package / "python-runtime" / "python" / "python.exe"
        if windows
        else package / "python-runtime" / "python" / "bin" / "python3.12"
    )
    fake_python.parent.mkdir(parents=True)
    (launcher_directory / "models").mkdir()
    shutil.copyfile(args.launcher, launcher)
    shutil.copyfile(args.fake_python, fake_python)
    launcher.chmod(launcher.stat().st_mode | stat.S_IXUSR)
    fake_python.chmod(fake_python.stat().st_mode | stat.S_IXUSR)
    (launcher_directory / "maia3_entry.py").write_text(
        "# synthetic launcher entry point\n", encoding="utf-8", newline="\n"
    )
    (launcher_directory / "models" / "maia3-5m.pt").write_bytes(b"synthetic checkpoint")
    personalize(package, launcher)

    if sys.platform == "darwin":
        subprocess.run(
            ["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", launcher],
            check=True,
        )
        subprocess.run(["/usr/bin/codesign", "--verify", "--strict", launcher], check=True)

    make_read_only(package, launcher, fake_python)
    completed = subprocess.run(
        [launcher],
        cwd=package,
        stdin=subprocess.DEVNULL,
        text=True,
        capture_output=True,
        timeout=20,
        check=True,
    )
    if "synthetic Maia3 child reached" not in completed.stdout:
        raise RuntimeError(f"launcher did not reach its synthetic child: {completed.stdout!r}")
    require_child_termination(launcher, package)

    entry_point = launcher_directory / "maia3_entry.py"
    entry_point.chmod(0o644)
    with entry_point.open("ab") as handle:
        handle.write(b"# changed\n")
    changed = subprocess.run(
        [launcher],
        cwd=package,
        stdin=subprocess.DEVNULL,
        text=True,
        capture_output=True,
        timeout=20,
    )
    if changed.returncode == 0 or "package has changed" not in changed.stderr:
        raise RuntimeError(
            "personalized launcher did not reject a changed payload: "
            f"status={changed.returncode}, stderr={changed.stderr!r}"
        )
    print(f"personalized launcher smoke passed for {args.platform}")


if __name__ == "__main__":
    main()
