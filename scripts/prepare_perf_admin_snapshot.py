#!/usr/bin/env python3
"""Prepare or restore a disposable admin-stats A/B source worktree.

The new private directory is never automatically deleted. Preparation copies the
current candidate source, then replaces only admin_stats.rs with the same HEAD's
version for a baseline build. Restore verifies the full baseline fingerprints
before putting back the exact saved candidate file. No production data is used.
"""
from __future__ import annotations
import argparse
import json
import hashlib
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import uuid

from build_perf_server import command_output, file_digest, source_fingerprints

ROOT = Path(__file__).resolve().parents[1]
TARGET = "apps/melodee-server/src/admin_stats.rs"


def reject_symlink_components(root: Path, relative: str) -> None:
    current = root
    for part in Path(relative).parts:
        current = current / part
        if current.is_symlink():
            raise ValueError("snapshot path contains a symlink")


def open_target(tree: Path) -> tuple[list[int], int]:
    """Pin each owned directory with no-follow opens and then its file."""
    descriptors = []
    try:
        descriptor = os.open(tree, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        descriptors.append(descriptor)
        for name in ("apps", "melodee-server", "src"):
            descriptor = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC,
                                 dir_fd=descriptor)
            descriptors.append(descriptor)
        for fd in descriptors:
            if os.fstat(fd).st_uid != os.geteuid():
                raise ValueError("snapshot directory owner changed")
        target = os.open("admin_stats.rs", os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=descriptor)
        info = os.fstat(target)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_nlink != 1:
            os.close(target)
            raise ValueError("snapshot target is not an exclusive owned regular file")
        return descriptors, target
    except BaseException:
        for fd in reversed(descriptors):
            os.close(fd)
        raise


def replace_owned_target(tree: Path, expected_hash: str, replacement: bytes) -> None:
    directories, target = open_target(tree)
    temporary_name = f".admin-stats-{uuid.uuid4().hex}.tmp"
    temporary_fd = None
    temporary_exists = False
    try:
        digest = hashlib.sha256()
        offset = 0
        while chunk := os.pread(target, 1024 * 1024, offset):
            digest.update(chunk)
            offset += len(chunk)
        if digest.hexdigest() != expected_hash:
            raise ValueError("snapshot target bytes changed")
        temporary_fd = os.open(temporary_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                               stat.S_IMODE(os.fstat(target).st_mode), dir_fd=directories[-1])
        temporary_exists = True
        view = memoryview(replacement)
        while view:
            written = os.write(temporary_fd, view)
            if written <= 0:
                raise OSError("short snapshot write")
            view = view[written:]
        os.fsync(temporary_fd)
        os.close(temporary_fd)
        temporary_fd = None
        # Re-open every path component before touching bytes. The held file
        # descriptor, not a later pathname lookup, receives the write.
        again, current = open_target(tree)
        try:
            identity = lambda fd: (os.fstat(fd).st_dev, os.fstat(fd).st_ino)
            if ([identity(fd) for fd in again] != [identity(fd) for fd in directories]
                    or identity(current) != identity(target)):
                raise ValueError("snapshot target ancestry changed")
        finally:
            os.close(current)
            for fd in reversed(again):
                os.close(fd)
        # Replace only the private directory entry. A hardlink created after
        # validation continues to refer to the untouched original inode.
        os.replace(temporary_name, "admin_stats.rs", src_dir_fd=directories[-1], dst_dir_fd=directories[-1])
        temporary_exists = False
        os.fsync(directories[-1])
    finally:
        try:
            if temporary_fd is not None:
                os.close(temporary_fd)
            if temporary_exists:
                os.unlink(temporary_name, dir_fd=directories[-1])
        finally:
            os.close(target)
            for fd in reversed(directories):
                os.close(fd)


def prepare(destination: Path) -> None:
    if destination.exists() or destination.is_symlink():
        raise ValueError("snapshot directory must not exist")
    head = command_output(["git", "rev-parse", "HEAD"], ROOT)
    for name in ("apps", "crates", "scripts"):
        root = ROOT / name
        if root.is_symlink() or any(path.is_symlink() for path in root.rglob("*")):
            raise ValueError("source directory contains a symlink")
    before = source_fingerprints(ROOT)
    baseline = subprocess.run(["git", "show", f"{head}:{TARGET}"], cwd=ROOT,
                              stdout=subprocess.PIPE, check=True, timeout=15).stdout
    candidate = (ROOT / TARGET).read_bytes()
    if candidate == baseline:
        raise ValueError("candidate admin stats matches HEAD")
    destination.mkdir(mode=0o700)
    tree = destination / "tree"
    subprocess.run(["git", "worktree", "add", "--detach", str(tree), head], cwd=ROOT, check=True)
    for name in ("apps", "crates", "scripts"):
        root = tree / name
        if root.is_symlink() or any(path.is_symlink() for path in root.rglob("*")):
            raise ValueError("checkout directory contains a symlink")
        shutil.copytree(ROOT / name, tree / name, dirs_exist_ok=True, symlinks=True,
                        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"))
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
        reject_symlink_components(tree, name)
        shutil.copy2(ROOT / name, tree / name)
    for name in before:
        if name.startswith("compatibility/"):
            reject_symlink_components(tree, name)
            path = tree / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, path)
    if (source_fingerprints(ROOT) != before or source_fingerprints(tree) != before
            or command_output(["git", "rev-parse", "HEAD"], ROOT) != head):
        raise RuntimeError("source changed while snapshotting")
    saved = destination / "candidate-admin-stats.rs"
    with saved.open("xb") as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(candidate)
        output.flush()
        os.fsync(output.fileno())
    replace_owned_target(tree, before[TARGET], baseline)
    baseline_hashes = source_fingerprints(tree)
    if [name for name in sorted(before) if before[name] != baseline_hashes.get(name)] != [TARGET]:
        raise RuntimeError("baseline differs outside intended file")
    manifest = {"version": 1, "head": head, "candidate_sha256": before,
                "baseline_sha256": baseline_hashes, "saved_candidate_sha256": file_digest(saved)}
    with (destination / "manifest.json").open("x", encoding="utf-8") as output:
        os.fchmod(output.fileno(), 0o600)
        json.dump(manifest, output, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    print(json.dumps({"status": "baseline-ready", "head": head}))


def restore(destination: Path) -> None:
    info = destination.stat(follow_symlinks=False)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError("snapshot directory must be private and owned")
    tree = destination / "tree"
    manifest = destination / "manifest.json"
    saved = destination / "candidate-admin-stats.rs"
    if (tree.is_symlink() or not tree.is_dir()
            or any(path.is_symlink() or not path.is_file() for path in (manifest, saved, tree / TARGET))):
        raise ValueError("invalid snapshot files")
    for path in (manifest, saved):
        metadata = path.stat(follow_symlinks=False)
        if metadata.st_uid != os.geteuid() or stat.S_IMODE(metadata.st_mode) != 0o600:
            raise ValueError("snapshot metadata must be private and owned")
    registered = command_output(["git", "worktree", "list", "--porcelain"], ROOT)
    if f"worktree {tree.resolve(strict=True)}" not in registered.splitlines():
        raise ValueError("snapshot is not a registered owned worktree")
    if manifest.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("snapshot manifest too large")
    data = json.loads(manifest.read_text(encoding="utf-8"))
    if (not isinstance(data, dict) or data.get("version") != 1
            or command_output(["git", "rev-parse", "HEAD"], tree) != data.get("head")
            or source_fingerprints(tree) != data.get("baseline_sha256")
            or file_digest(saved) != data.get("saved_candidate_sha256")):
        raise ValueError("snapshot identity or source changed")
    replace_owned_target(tree, data["baseline_sha256"][TARGET], saved.read_bytes())
    if source_fingerprints(tree) != data.get("candidate_sha256"):
        raise RuntimeError("restored candidate differs from snapshot")
    print(json.dumps({"status": "candidate-ready", "head": data["head"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "restore"))
    parser.add_argument("snapshot", type=Path, help="new private directory owned by this invocation")
    args = parser.parse_args()
    try:
        if args.action == "prepare":
            prepare(args.snapshot)
        else:
            restore(args.snapshot)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Admin snapshot failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
