#!/usr/bin/env python3
"""Probe fs-verity on a newly created disposable test file only.

Linux x86_64, 4 KiB pages, and the exact installed kernel UAPI are required.
No existing input file is opened for writing and no filesystem feature or host
configuration is changed. The unique temporary directory is removed on exit.
"""
from __future__ import annotations

import argparse
import errno
import fcntl
import json
import mmap
import os
from pathlib import Path
import platform
import signal
import struct
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
# Linux generic _IOW('f', 133, struct fsverity_enable_arg), x86_64 only.
FS_IOC_ENABLE_VERITY = 0x40806685
ARG_FORMAT = "=IIIIQIIQ11Q"
UNSUPPORTED = {errno.ENOTTY, errno.EOPNOTSUPP, errno.ENOSYS}


def enable(fd: int) -> int:
    """Return zero or errno from FS_IOC_ENABLE_VERITY, never suppress errors."""
    arg = bytearray(struct.pack(ARG_FORMAT, 1, 1, 4096, 0, 0, 0, 0, 0, *([0] * 11)))
    if len(arg) != 128:
        raise RuntimeError("invalid kernel UAPI struct size")
    try:
        fcntl.ioctl(fd, FS_IOC_ENABLE_VERITY, arg, True)
        return 0
    except OSError as error:
        if error.errno is None:
            raise
        return error.errno


def write_attempt(path: Path) -> int:
    try:
        fd = os.open(path, os.O_WRONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as error:
        if error.errno is None:
            raise
        return error.errno
    try:
        try:
            os.pwrite(fd, b"B", 0)
            return 0
        except OSError as error:
            if error.errno is None:
                raise
            return error.errno
    finally:
        os.close(fd)


def probe(temp_parent: Path) -> dict:
    if sys.platform != "linux" or platform.machine() != "x86_64" or os.sysconf("SC_PAGE_SIZE") != 4096:
        raise ValueError("probe supports only Linux x86_64 with 4096-byte pages")
    parent = temp_parent.resolve(strict=True)
    if not parent.is_dir():
        raise ValueError("temporary parent is not a directory")
    read_pipe, write_pipe = os.pipe()
    try:
        unsupported_pipe = enable(read_pipe)
    finally:
        os.close(read_pipe)
        os.close(write_pipe)
    if unsupported_pipe != errno.ENOTTY:
        raise RuntimeError(f"unexpected pipe ioctl result: {unsupported_pipe}")
    with tempfile.TemporaryDirectory(prefix=".melodee-verity-probe-", dir=parent) as directory:
        root = Path(directory)
        path = root / "owned-stage-file"
        writer = os.open(path, os.O_CREAT | os.O_EXCL | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        readonly = None
        try:
            if os.write(writer, b"A" * 4096) != 4096:
                raise RuntimeError("short fixture write")
            os.fsync(writer)
            readonly = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            writer_result = enable(readonly)
            if writer_result in UNSUPPORTED:
                return {"status": "unsupported", "pipe_errno": unsupported_pipe,
                        "enable_with_writable_fd_errno": writer_result,
                        "limitations": ["no positive write-exclusion guarantee established"]}
            if writer_result != errno.ETXTBSY:
                raise RuntimeError(f"unexpected open-writer errno: {writer_result}")
            mapping = mmap.mmap(writer, 4096, access=mmap.ACCESS_WRITE)
            os.close(writer)
            writer = None
            try:
                mmap_result = enable(readonly)
            finally:
                mapping.close()
            if mmap_result != errno.ETXTBSY:
                raise RuntimeError(f"unexpected writable-mapping errno: {mmap_result}")
            seal_result = enable(readonly)
            if seal_result != 0:
                raise RuntimeError(f"enable failed after closing writers: {seal_result}")
            write_result = write_attempt(path)
            if write_result not in (errno.EPERM, errno.EACCES):
                raise RuntimeError(f"write was not rejected as expected: {write_result}")
            if os.pread(readonly, 4096, 0) != b"A" * 4096:
                raise RuntimeError("test bytes changed")
            return {"status": "verified", "pipe_errno": unsupported_pipe,
                    "enable_with_writable_fd_errno": writer_result,
                    "enable_with_writable_mapping_errno": mmap_result,
                    "enable_after_closing_writers_errno": seal_result,
                    "write_after_enable_errno": write_result,
                    "fixture_bytes": 4096, "filesystem": "temporary parent filesystem",
                    "limitations": ["one temporary file on one filesystem; not production custody",
                        "directory namespace, source mutation, crash recovery and cross-platform behavior not tested"]}
        finally:
            if readonly is not None:
                os.close(readonly)
            if writer is not None:
                os.close(writer)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--temp-parent", type=Path, default=ROOT,
                        help="existing parent for a unique disposable test directory")
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        print(json.dumps(probe(args.temp_parent), sort_keys=True))
    except (OSError, ValueError, RuntimeError, InterruptedError) as error:
        print(f"fs-verity probe failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
