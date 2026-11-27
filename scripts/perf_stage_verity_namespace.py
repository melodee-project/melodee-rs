#!/usr/bin/env python3
"""Run the existing stage-verity tests inside an owned private mount namespace.

The child bind-mounts a fresh workspace-owned Btrfs fixture, creates a new
PID namespace and a fresh read-only proc mount, and enters a child-only chroot; the parent never mounts or changes its security settings.
A successful test requires unprivileged user namespaces and mount operations.
Only sanitized test status is persisted. SIGKILL/host failure can leave owned
fixture directories for inspection, but private mounts die with the namespace.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

from build_perf_server import command_output, file_digest, source_fingerprints
from perf_media_io_trace import child_exited, require_owned_session_support, terminate_owned

ROOT = Path(__file__).resolve().parents[1]
TEST_DIR = ROOT / "target" / "debug" / "deps"
MAX_OUTPUT = 2 * 1024 * 1024
TEST_NAME = re.compile(r"melodee_ingestion-[0-9a-f]{16}\Z")
TEST_FILTERS = {
    "stage-verity": ("stage_verity::tests", False, 7),
    "sealed-tree": ("custody::tests::protected_service_owned_custody_namespace_is_accepted", True, 1),
}
SEALED_TREE_SUCCESS = "protected tree copy sealed all file contents"
SEALED_TREE_UNSUPPORTED = "protected tree copy retained source on unsupported fs-verity filesystem"
KNOWN_FAILURES = (
    "custody ancestry has an ACL", "custody ACL inspection failed",
    "custody root must be private, service-owned and on the source filesystem",
    "custody ancestry is not protected from the producer",
    "source and custody must use the same mount", "mount identity unavailable",
    "stage namespace has a submount", "private stage parent changed",
    "no writable ACL-free private stage parent available",
)


def sanitized_panics(output: str) -> list[dict[str, int | str]]:
    """Extract bounded locations only from known in-repository test modules."""
    pattern = r"panicked at (?:crates/melodee-ingestion/src/)?(custody|stage_verity)\.rs:(\d{1,6}):(\d{1,6})(?::|\s)"
    found = []
    for module, line, column in re.findall(pattern, output):
        location = {"file": module + ".rs", "line": int(line), "column": int(column)}
        if location not in found:
            found.append(location)
        if len(found) == 8:
            break
    return found


def sanitized_mount_failure(output: str) -> dict[str, str | int] | None:
    steps = {"fixture_bind", "proc_private_readonly", "libraries_bind", "libraries_readonly",
             "binary_bind", "binary_readonly"}
    reasons = {"permission denied", "operation not permitted", "wrong fs type", "invalid argument",
               "no such file or directory", "other mount error"}
    for line in output.splitlines():
        if not line.startswith('{"status": "mount_failed"') or len(line) > 512:
            continue
        try:
            value = json.loads(line)
        except (ValueError, TypeError):
            continue
        if (isinstance(value, dict) and set(value) == {"status", "step", "reason", "exit_code"}
                and value["status"] == "mount_failed" and isinstance(value["step"], str) and value["step"] in steps
                and isinstance(value["reason"], str) and value["reason"] in reasons and type(value["exit_code"]) is int
                and -255 <= value["exit_code"] <= 255 and value["exit_code"] != 0):
            return value
    return None


def test_command(selector: str) -> list[str]:
    if selector not in TEST_FILTERS:
        raise ValueError("test selector is not allowlisted")
    test_name, exact, _ = TEST_FILTERS[selector]
    return ["/test-binary", test_name, *(["--exact"] if exact else []), "--nocapture", "--test-threads=1"]


def classify_result(selector: str, exit_code: int, passed: int, unsupported: bool,
                    tree_success: bool, tree_unsupported: bool, censored: bool) -> str:
    test_command(selector)
    if censored:
        return "censored"
    if exit_code != 0 or passed != TEST_FILTERS[selector][2]:
        return "failed"
    if unsupported or tree_unsupported:
        return "unsupported"
    if selector == "sealed-tree" and not tree_success:
        return "failed"
    return "verified"


def validate_binary(binary: Path) -> Path:
    if binary.is_symlink() or not binary.is_file() or not TEST_NAME.fullmatch(binary.name):
        raise ValueError("invalid ingestion test executable")
    resolved = binary.resolve(strict=True)
    if resolved.parent != TEST_DIR.resolve(strict=True):
        raise ValueError("test executable is outside the workspace test directory")
    return resolved


def private_directory(path: Path) -> None:
    info = path.stat(follow_symlinks=False)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ValueError("fixture directory is not uniquely owned and private")


def write_private_log(path: Path, contents: bytes) -> None:
    """Create a private raw log without following directory or target symlinks."""
    parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        info = os.fstat(parent)
        if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise ValueError("raw log directory is not privately owned")
        fd = os.open(path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o600, dir_fd=parent)
        with os.fdopen(fd, "wb") as raw:
            raw.write(contents)
            raw.flush()
            os.fsync(raw.fileno())
    finally:
        os.close(parent)


def private_mountinfo(data: str) -> bool:
    for line in data.splitlines():
        head, separator, _ = line.partition(" - ")
        fields = head.split()
        if not separator or len(fields) < 6:
            return False
        if any(item.startswith(("shared:", "master:", "propagate_from:")) for item in fields[6:]):
            return False
    return bool(data.strip())


def validate_namespace(original_mount_ino: int, original_user_ino: int,
                       original_pid_ino: int, parent_uid: int) -> None:
    if os.geteuid() != 0 or parent_uid == 0:
        raise ValueError("namespace must map an unprivileged user to root")
    if (os.stat("/proc/self/ns/mnt").st_ino == original_mount_ino
            or os.stat("/proc/self/ns/user").st_ino == original_user_ino
            or os.stat("/proc/self/ns/pid").st_ino == original_pid_ino or os.getpid() != 1):
        raise ValueError("refusing operations without private user, mount and PID namespaces")
    mapping = Path("/proc/self/uid_map").read_text().split()
    if mapping != ["0", str(parent_uid), "1"]:
        raise ValueError("unexpected user namespace mapping")
    # The launcher makes propagation private. Verify before issuing any mount.
    if not private_mountinfo(Path("/proc/self/mountinfo").read_text()):
        raise ValueError("mount propagation is not wholly private")


def close_fixture_fds(source_fd: int, mount_fd: int) -> None:
    if source_fd < 3 or mount_fd < 3 or source_fd == mount_fd:
        raise ValueError("invalid fixture descriptors")
    os.close(source_fd)
    os.close(mount_fd)
    for entry in os.listdir("/proc/self/fd"):
        if not entry.isdecimal() or int(entry) < 3:
            continue
        fd = int(entry)
        try:
            inheritable = os.get_inheritable(fd)
        except OSError:
            # The directory enumeration FD may already have been closed.
            continue
        if inheritable:
            raise ValueError("unexpected inheritable descriptor")


def test_environment() -> dict[str, str]:
    return {"PATH": "/usr/bin:/bin", "XDG_RUNTIME_DIR": "/", "HOME": "/",
            "RUST_TEST_THREADS": "1", "RUST_BACKTRACE": "0",
            "MELODEE_TEST_EXECUTABLE": "/test-binary"}


def child(args: argparse.Namespace) -> int:
    validate_namespace(args.original_mount_ino, args.original_user_ino,
                       args.original_pid_ino, args.parent_uid)
    command = test_command(args.test_filter)
    source = Path(args.source).resolve(strict=True)
    target = Path(args.mountpoint).resolve(strict=True)
    binary = validate_binary(Path(args.test_binary))
    # uid maps to zero in this namespace. Both paths were freshly created by
    # the parent and passed with open directory FDs; reject substitution.
    for path, fd in ((source, args.source_fd), (target, args.mount_fd)):
        info, held = path.stat(follow_symlinks=False), os.fstat(fd)
        if (not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != 0o700
                or (info.st_dev, info.st_ino) != (held.st_dev, held.st_ino)):
            raise ValueError("fixture directory identity changed")
    if source == target or source in target.parents or target in source.parents:
        raise ValueError("fixture directories overlap")
    # Dedicated mount points are created only under our fresh source fixture.
    (source / "proc").mkdir(mode=0o700)
    (source / "usr").mkdir(mode=0o700)
    (source / "usr" / "lib64").mkdir(mode=0o700)
    (source / "lib64").symlink_to("usr/lib64")
    test_target = source / "test-binary"
    fd = os.open(test_target, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o700)
    os.close(fd)
    mount = shutil.which("mount", path="/usr/bin:/bin")
    if mount is None:
        raise ValueError("mount executable unavailable")
    def do_mount(step: str, arguments: list[str]) -> None:
        result = subprocess.run([mount, *arguments], capture_output=True, text=True, check=False, timeout=15)
        if result.returncode:
            stderr = result.stderr.lower()
            reason = next((reason for reason in ("permission denied", "operation not permitted", "wrong fs type", "invalid argument", "no such file or directory") if reason in stderr), "other mount error")
            print(json.dumps({"status": "mount_failed", "step": step, "reason": reason, "exit_code": result.returncode}), flush=True)
            raise RuntimeError("mount failed inside private namespace")
    do_mount("fixture_bind", ["--bind", str(source), str(target)])
    do_mount("proc_private_readonly", ["-t", "proc", "-o", "ro,nosuid,nodev,noexec", "proc", str(target / "proc")])
    do_mount("libraries_bind", ["--bind", "/usr/lib64", str(target / "usr" / "lib64")])
    do_mount("libraries_readonly", ["-o", "remount,bind,ro", str(target / "usr" / "lib64")])
    do_mount("binary_bind", ["--bind", str(binary), str(target / "test-binary")])
    do_mount("binary_readonly", ["-o", "remount,bind,ro", str(target / "test-binary")])
    os.chroot(target)
    os.chdir("/")
    # After chroot, the new root must be our original fixture inode.
    info, held = os.stat("/"), os.fstat(args.source_fd)
    if (info.st_dev, info.st_ino) != (held.st_dev, held.st_ino):
        raise ValueError("chroot root is not the owned fixture")
    close_fixture_fds(args.source_fd, args.mount_fd)
    os.execve("/test-binary", command, test_environment())
    raise RuntimeError("exec failed")


def validate_build_proof(binary: Path, proof: Path) -> str:
    if proof.is_symlink() or not proof.is_file() or proof.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid sealed-tree build proof")
    record = json.loads(proof.read_text(encoding="utf-8"))
    scripts = ("scripts/build_stage_verity_test.py", "scripts/build_perf_server.py",
               "scripts/perf_stage_verity_namespace.py", "scripts/perf_media_io_trace.py")
    if (not isinstance(record, dict) or record.get("target") != "sealed-tree-libtest"
            or record.get("profile") != "debug" or record.get("features") != ["test-support"]
            or record.get("binary_name") != binary.name or record.get("binary_sha256") != file_digest(binary)
            or record.get("commit") != command_output(["git", "rev-parse", "HEAD"], ROOT)
            or record.get("source_sha256") != source_fingerprints(ROOT)
            or record.get("script_sha256") != {name: file_digest(ROOT / name) for name in scripts}):
        raise ValueError("sealed-tree executable or source does not match proof")
    return file_digest(proof)


def run(binary: Path, output: Path, timeout: int, selector: str = "stage-verity",
        provenance: Path | None = None, private_log: Path | None = None) -> None:
    test_command(selector)
    if timeout < 1 or timeout > 300:
        raise ValueError("timeout out of bounds")
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if private_log is not None:
        private_directory(private_log.parent)
        if private_log.exists() or private_log.is_symlink():
            raise ValueError("private log already exists")
    binary = validate_binary(binary)
    if selector == "sealed-tree" and provenance is None:
        raise ValueError("sealed-tree requires a fingerprinted build proof")
    proof_hash = validate_build_proof(binary, provenance) if provenance is not None else None
    if os.geteuid() == 0:
        raise ValueError("unprivileged Linux pidfd environment required")
    require_owned_session_support()
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", ""))
    if not runtime.is_absolute():
        raise ValueError("private runtime parent unavailable")
    private_directory(runtime)
    unshare = shutil.which("unshare", path="/usr/bin:/bin")
    if unshare is None:
        raise ValueError("unshare executable unavailable")
    before = {name: os.stat("/proc/self/ns/" + name).st_ino for name in ("user", "mnt", "pid")}
    binary_hash = file_digest(binary)
    with tempfile.TemporaryDirectory(prefix=".melodee-verity-source-", dir=ROOT) as source_name, \
         tempfile.TemporaryDirectory(prefix=".melodee-verity-mount-", dir=runtime) as mount_name:
        source, target = Path(source_name), Path(mount_name)
        private_directory(source)
        private_directory(target)
        flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        source_fd, mount_fd = os.open(source, flags), os.open(target, flags)
        try:
            args = [unshare, "--user", "--map-root-user", "--mount", "--pid", "--fork", "--kill-child=KILL", "--propagation", "private", "--",
                sys.executable, str(Path(__file__).resolve()), "--internal-child", "--source", str(source),
                "--mountpoint", str(target), "--source-fd", str(source_fd), "--mount-fd", str(mount_fd),
                "--original-mount-ino", str(before["mnt"]), "--original-user-ino", str(before["user"]),
                "--original-pid-ino", str(before["pid"]),
                "--parent-uid", str(os.geteuid()), "--test-binary", str(binary), "--test-filter", selector]
            with tempfile.TemporaryFile(mode="w+b") as log:
                process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                           pass_fds=(source_fd, mount_fd), start_new_session=True)
                try:
                    leader = os.pidfd_open(process.pid)
                except BaseException:
                    terminate_owned(process)
                    raise
                started, censored = time.monotonic(), False
                try:
                    while not child_exited(leader):
                        if time.monotonic() - started >= timeout or os.fstat(log.fileno()).st_size > MAX_OUTPUT:
                            censored = True
                            break
                        time.sleep(0.05)
                finally:
                    try:
                        terminate_owned(process)
                    finally:
                        os.close(leader)
                if os.fstat(log.fileno()).st_size > MAX_OUTPUT:
                    censored = True
                log.seek(0)
                text = log.read(MAX_OUTPUT + 1).decode("utf-8", errors="replace")
                if private_log is not None:
                    write_private_log(private_log, text.encode("utf-8"))
                # Keep only fixed recognized diagnostics, never raw paths.
                unsupported = text.count("fs-verity unsupported on this test filesystem")
                passed = re.search(r"test result: ok\. (\d+) passed; 0 failed", text)
                mount_failure = sanitized_mount_failure(text)
                passed_count = int(passed.group(1)) if passed else 0
                tree_success = SEALED_TREE_SUCCESS in text
                tree_unsupported = SEALED_TREE_UNSUPPORTED in text
                status = classify_result(selector, process.returncode, passed_count, bool(unsupported),
                                         tree_success, tree_unsupported, censored)
                result = {"status": status,
                    "build_provenance_sha256": proof_hash,
                    "test_filter": selector, "sealed_tree_success_marker": tree_success,
                    "sealed_tree_unsupported_marker": tree_unsupported,
                    "exit_code": process.returncode, "test_binary_sha256": binary_hash,
                    "test_functions_passed": passed_count,
                    "unsupported_branch_messages": unsupported,
                    "mount_failure": mount_failure,
                    "known_failure_reasons": [reason for reason in KNOWN_FAILURES if reason in text],
                    "panic_locations": sanitized_panics(text),
                    "parent_mount_namespace_unchanged": os.stat("/proc/self/ns/mnt").st_ino == before["mnt"],
                    "parent_user_namespace_unchanged": os.stat("/proc/self/ns/user").st_ino == before["user"],
                    "parent_pid_namespace_unchanged": os.stat("/proc/self/ns/pid").st_ino == before["pid"],
                    "limitations": ["test-only adapter, not production custody", "not a performance benchmark",
                        "namespace exit removes private mounts; SIGKILL may leave owned directories"]}
        finally:
            os.close(source_fd)
            os.close(mount_fd)
    if provenance is not None:
        try:
            if validate_build_proof(binary, provenance) != proof_hash:
                result["status"] = "invalidated"
        except (OSError, ValueError, subprocess.SubprocessError):
            result["status"] = "invalidated"
    with output.open("x", encoding="utf-8") as destination:
        json.dump(result, destination, indent=2, sort_keys=True)
        destination.write("\n")
    if result["status"] != "verified":
        raise RuntimeError("namespace test did not complete")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--build-provenance", type=Path)
    parser.add_argument("--test-filter", choices=sorted(TEST_FILTERS), default="stage-verity")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--private-log", type=Path, help="retain bounded raw output in an owned 0700 directory")
    parser.add_argument("--timeout-seconds", type=int, default=120)
    parser.add_argument("--internal-child", action="store_true", help=argparse.SUPPRESS)
    for name in ("source", "mountpoint"):
        parser.add_argument("--" + name, help=argparse.SUPPRESS)
    for name in ("source-fd", "mount-fd", "original-mount-ino", "original-user-ino", "original-pid-ino", "parent-uid"):
        parser.add_argument("--" + name, type=int, help=argparse.SUPPRESS)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        if args.internal_child:
            return child(args)
        if args.output is None:
            parser.error("--output is required")
        run(args.test_binary, args.output, args.timeout_seconds, args.test_filter, args.build_provenance, args.private_log)
    except (OSError, ValueError, RuntimeError, InterruptedError, subprocess.SubprocessError) as error:
        print(f"Stage namespace test failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
