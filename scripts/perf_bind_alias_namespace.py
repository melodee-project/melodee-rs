#!/usr/bin/env python3
"""Run one owned bind-alias test in an independently checked private namespace.

The launcher mounts only inside a newly checked private user/mount/PID namespace
and an invocation-owned temporary directory before entering a child-only chroot.
Only sanitized status is written. The owned fixture directory is retained for
operator inspection, avoiding cleanup through a replaceable path; private mounts
die with the last namespace process.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time

from build_perf_server import file_digest
from perf_media_io_trace import child_exited, require_owned_session_support, terminate_owned
from perf_stage_verity_namespace import validate_binary, validate_build_proof

ROOT = Path(__file__).resolve().parents[1]
SELECTOR = "custody::tests::private_namespace_bind_aliases_are_rejected"
MAX_LOG = 1024 * 1024
MAX_MOUNTINFO = 8 * 1024 * 1024
MAX_MOUNT_ERROR = 4096
TRACE_SYSCALLS = ("mount", "fsopen", "fsconfig", "fsmount", "move_mount", "open_tree", "mount_setattr")
TRACE_ERRNOS = {"EINVAL", "EPERM", "EACCES", "ENOENT", "ENOTDIR", "ELOOP", "ENOSYS", "EOPNOTSUPP", "EXDEV", "EBUSY"}
MOUNT_REASONS = {"permission denied": "permission_denied", "operation not permitted": "operation_not_permitted",
                 "wrong fs type": "wrong_fs_type", "invalid argument": "invalid_argument",
                 "no such file or directory": "not_found", "not a directory": "not_directory"}
STAGES = ("namespace", "binary", "fixture", "tree", "proc_mount", "proc_check",
          "library_bind", "library_remount", "binary_bind", "binary_remount",
          "mount_bind", "mount_remount", "umount_bind", "umount_remount", "chroot", "exec")
FAILURES = {"namespace must map an unprivileged caller to root": "caller_mapping",
            "private user, mount and PID namespaces required": "namespace_identity",
            "unexpected UID mapping": "uid_mapping", "mount propagation is not wholly private": "mount_propagation",
            "test binary identity changed": "binary_identity", "test binary bytes changed": "binary_digest",
            "fixture directory identity changed": "fixture_identity", "runtime library directory is not protected": "library_identity",
            "proc mount identity changed": "proc_mount_identity", "bound mount identity changed": "bind_identity",
            "mount topology changed": "mount_topology", "mount is not read-only": "mount_not_readonly",
            "chroot root identity changed": "chroot_identity", "unexpected inheritable descriptor": "inherited_descriptor"}


def mark_stage(stage: str) -> None:
    if stage not in STAGES:
        raise ValueError("invalid setup stage")
    print("bind_alias_stage=" + stage, flush=True)


def sanitized_setup(data: str) -> tuple[str | None, str | None, str | None, list[str]]:
    stage = reason = mount_reason = None
    calls = []
    for line in data.splitlines():
        if line.startswith("bind_alias_stage=") and line.removeprefix("bind_alias_stage=") in STAGES:
            stage = line.removeprefix("bind_alias_stage=")
        elif line.startswith("bind_alias_reason=") and line.removeprefix("bind_alias_reason=") in set(FAILURES.values()) | {"mount_command_failed", "other"}:
            reason = line.removeprefix("bind_alias_reason=")
        elif line.startswith("bind_alias_mount_reason=") and line.removeprefix("bind_alias_mount_reason=") in set(MOUNT_REASONS.values()) | {"other", "censored"}:
            mount_reason = line.removeprefix("bind_alias_mount_reason=")
        elif line.startswith("bind_alias_syscall="):
            value = line.removeprefix("bind_alias_syscall=")
            if value in {f"{call}:{error}" for call in TRACE_SYSCALLS for error in TRACE_ERRNOS | {"ok"}} and value not in calls:
                calls.append(value)
    return stage, reason, mount_reason, calls[:8]


def private_mountinfo(data: bytes) -> bool:
    """Reject malformed or propagated mounts, including unknown optional fields."""
    if not data or len(data) > MAX_MOUNTINFO:
        return False
    for line in data.splitlines():
        head, separator, tail = line.partition(b" - ")
        fields = head.split()
        if not separator or len(fields) != 6 or len(tail.split()) < 3:
            return False
    return True


def trusted_tool_paths(root: Path = Path("/usr/bin")) -> Path:
    """Reject caller-writable or redirected mount/namespace executables."""
    directory = root.lstat()
    if (not stat.S_ISDIR(directory.st_mode) or directory.st_uid != 0
            or directory.st_mode & 0o022):
        raise ValueError("trusted tool directory is not protected")
    for name in ("unshare", "mount", "umount"):
        info = (root / name).lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or info.st_mode & 0o022 or not info.st_mode & 0o111):
            raise ValueError("trusted tool is not protected")
    libraries = Path("/usr/lib64").lstat()
    if not stat.S_ISDIR(libraries.st_mode) or libraries.st_uid != 0 or libraries.st_mode & 0o022:
        raise ValueError("runtime library directory is not protected")
    return root / "unshare"


def trusted_trace_path() -> Path:
    path = Path("/usr/bin/strace")
    info = path.lstat()
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o6022
            or not info.st_mode & 0o111):
        raise ValueError("trace tool is not protected")
    return path


def syscall_labels(stderr: str) -> list[str]:
    """Return only fixed syscall/errno labels, never syscall arguments."""
    pattern = r"\b(" + "|".join(TRACE_SYSCALLS) + r")\([^\n]*\)\s+=\s+(-1\s+(E[A-Z0-9]+)|0\b)"
    labels = []
    for call, result, error in re.findall(pattern, stderr):
        value = "ok" if result == "0" else error
        if (value == "ok" or value in TRACE_ERRNOS) and f"{call}:{value}" not in labels:
            labels.append(f"{call}:{value}")
        if len(labels) == 8:
            break
    return labels


def test_command(executable: str) -> list[str]:
    if executable != "/test-binary":
        raise ValueError("invalid pinned test executable")
    return [executable, SELECTOR, "--exact", "--ignored", "--nocapture", "--test-threads=1"]


def validate_child_namespace(original_mount: str, original_user: str, original_pid: str, parent_uid: int) -> None:
    if parent_uid <= 0 or os.geteuid() != 0:
        raise ValueError("namespace must map an unprivileged caller to root")
    if (os.readlink("/proc/self/ns/mnt") == original_mount
            or os.readlink("/proc/self/ns/user") == original_user
            or os.readlink("/proc/self/ns/pid") == original_pid or os.getpid() != 1):
        raise ValueError("private user, mount and PID namespaces required")
    if Path("/proc/self/uid_map").read_text().split() != ["0", str(parent_uid), "1"]:
        raise ValueError("unexpected UID mapping")
    with open("/proc/self/mountinfo", "rb") as stream:
        if not private_mountinfo(stream.read(MAX_MOUNTINFO + 1)):
            raise ValueError("mount propagation is not wholly private")


def fixture_reference(fd: int) -> Path:
    if fd < 3:
        raise ValueError("invalid fixture descriptor")
    return Path(f"/proc/self/fd/{fd}")


def fixture_entry_is_original(path: Path, fd: int) -> bool:
    """Report identity only; never remove a possibly replaced pathname."""
    try:
        current, held = path.stat(follow_symlinks=False), os.fstat(fd)
        return (stat.S_ISDIR(current.st_mode)
                and (current.st_dev, current.st_ino) == (held.st_dev, held.st_ino))
    except OSError:
        return False


@contextlib.contextmanager
def retained_fixture():
    # Do not recursively remove a pathname that could have been replaced
    # while the test ran. An operator may inspect and clean it separately.
    yield tempfile.mkdtemp(prefix=".melodee-bind-alias-", dir=ROOT)


def child_environment(original_mount: str) -> dict[str, str]:
    # The pinned fixture becomes the child-only root; no host FDs survive exec.
    return {"PATH": "/usr/bin", "HOME": "/", "TMPDIR": "/",
            "RUST_TEST_THREADS": "1", "RUST_BACKTRACE": "0",
            "MELODEE_TEST_ORIGINAL_MOUNT_NS": original_mount}


def mount_id(fd: int) -> int:
    with open(f"/proc/self/fdinfo/{fd}", encoding="ascii") as stream:
        ids = [line.split()[1] for line in stream if line.startswith("mnt_id:")]
    if len(ids) != 1 or not ids[0].isdigit():
        raise ValueError("mount identity unavailable")
    return int(ids[0])


def require_readonly_mount(fd: int) -> None:
    mount = str(mount_id(fd)).encode()
    with open("/proc/self/mountinfo", "rb") as stream:
        data = stream.read(MAX_MOUNTINFO + 1)
    if not private_mountinfo(data):
        raise ValueError("mount topology changed")
    matches = [line.split(b" - ", 1)[0].split() for line in data.splitlines()
               if line.split(None, 1)[0] == mount]
    if len(matches) != 1 or b"ro" not in matches[0][5].split(b","):
        raise ValueError("mount is not read-only")


def capture_mount(command: list[str], fds: tuple[int, ...], timeout: int = 15,
                  limit: int = MAX_MOUNT_ERROR) -> tuple[int, str, bool]:
    """Bound one trusted mount child's stderr; the outer session owns descendants."""
    if timeout < 1 or limit < 1:
        raise ValueError("invalid mount capture limits")
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE, pass_fds=fds, bufsize=0,
                               env={"PATH": "/usr/bin", "LC_ALL": "C"})
    try:
        leader = os.pidfd_open(process.pid)
    except BaseException:
        try:
            os.kill(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        if process.stderr is not None:
            process.stderr.close()
        raise
    output = bytearray()
    started, censored = time.monotonic(), False
    try:
        if process.stderr is None:
            raise RuntimeError("mount child output unavailable")
        with selectors.DefaultSelector() as selector:
            selector.register(process.stderr, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() - started >= timeout:
                    censored = True
                    break
                for key, _ in selector.select(timeout=0.05):
                    chunk = os.read(key.fd, min(4096, limit - len(output) + 1))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    elif len(output) + len(chunk) > limit:
                        censored = True
                        break
                    else:
                        output.extend(chunk)
                if censored:
                    break
            while not censored and not child_exited(leader):
                if time.monotonic() - started >= timeout:
                    censored = True
                    break
                time.sleep(0.05)
    finally:
        try:
            if not child_exited(leader):
                try:
                    signal.pidfd_send_signal(leader, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=5)
        finally:
            os.close(leader)
            if process.stderr is not None:
                process.stderr.close()
    return process.returncode, output.decode("utf-8", errors="replace"), censored


def mount_private(args: argparse.Namespace, command: list[str], fds: list[int]) -> None:
    # unshare already made propagation private; recheck before each mount.
    validate_child_namespace(args.original_mount, args.original_user, args.original_pid, args.parent_uid)
    mount_argv = ["/usr/bin/mount", "--no-canonicalize", *command]
    argv = (["/usr/bin/strace", "-f", "-qq", "-e", "trace=" + ",".join(TRACE_SYSCALLS), *mount_argv]
            if args.trace_mount else mount_argv)
    returncode, stderr, censored = capture_mount(argv, tuple(fds))
    if args.trace_mount:
        for label in syscall_labels(stderr):
            print("bind_alias_syscall=" + label, flush=True)
    if returncode or censored:
        reason = "censored" if censored else next((label for text, label in MOUNT_REASONS.items()
                                                   if text in stderr.lower()), "other")
        print("bind_alias_mount_reason=" + reason, flush=True)
        raise subprocess.CalledProcessError(returncode or 1, argv)


def child(args: argparse.Namespace) -> None:
    mark_stage("namespace")
    validate_child_namespace(args.original_mount, args.original_user, args.original_pid, args.parent_uid)
    if args.binary_fd < 3 or args.fixture_fd < 3 or args.binary_fd == args.fixture_fd:
        raise ValueError("invalid inherited descriptors")
    mark_stage("binary")
    info = os.fstat(args.binary_fd)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != 0:
        raise ValueError("test binary identity changed")
    with os.fdopen(os.dup(args.binary_fd), "rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != args.binary_sha256:
        raise ValueError("test binary bytes changed")
    mark_stage("fixture")
    held = os.fstat(args.fixture_fd)
    if (not stat.S_ISDIR(held.st_mode) or held.st_uid != 0
            or stat.S_IMODE(held.st_mode) != 0o700
            or (held.st_dev, held.st_ino) != (args.fixture_device, args.fixture_inode)):
        raise ValueError("fixture directory identity changed")
    fds = [args.binary_fd, args.fixture_fd]
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    def directory(parent: int, name: str) -> int:
        os.mkdir(name, mode=0o700, dir_fd=parent)
        fd = os.open(name, flags, dir_fd=parent)
        fds.append(fd)
        return fd
    def placeholder(parent: int, name: str) -> int:
        fd = os.open(name, os.O_RDONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                     0o700, dir_fd=parent)
        fds.append(fd)
        return fd
    def bind_readonly(source: int, parent: int, name: str, target: int, is_dir: bool, label: str) -> None:
        mark_stage(label + "_bind")
        source_info = os.fstat(source)
        source_mount = mount_id(source)
        mount_private(args, ["--bind", str(fixture_reference(source)), str(fixture_reference(target))], fds)
        opened = os.open(name, flags if is_dir else os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
        fds.append(opened)
        actual = os.fstat(opened)
        if ((actual.st_dev, actual.st_ino) != (source_info.st_dev, source_info.st_ino)
                or mount_id(opened) == source_mount):
            raise ValueError("bound mount identity changed")
        mark_stage(label + "_remount")
        mount_private(args, ["-o", "remount,bind,ro", str(fixture_reference(opened))], fds)
        require_readonly_mount(opened)

    try:
        mark_stage("tree")
        proc = directory(args.fixture_fd, "proc")
        usr = directory(args.fixture_fd, "usr")
        bindir = directory(usr, "bin")
        libdir = directory(usr, "lib64")
        os.symlink("usr/lib64", "lib64", dir_fd=args.fixture_fd)
        test_target = placeholder(args.fixture_fd, "test-binary")
        mount_target = placeholder(bindir, "mount")
        umount_target = placeholder(bindir, "umount")
        lib_source = os.open("/usr/lib64", flags)
        fds.append(lib_source)
        lib_info = os.fstat(lib_source)
        if (not stat.S_ISDIR(lib_info.st_mode) or lib_info.st_uid != os.stat("/usr/bin/mount").st_uid
                or lib_info.st_mode & 0o022):
            raise ValueError("runtime library directory is not protected")
        tool_sources = []
        for name in ("mount", "umount"):
            fd = os.open("/usr/bin/" + name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            fds.append(fd)
            tool_sources.append(fd)
        mark_stage("proc_mount")
        mount_private(args, ["-t", "proc", "-o", "ro,nosuid,nodev,noexec", "proc", str(fixture_reference(proc))], fds)
        mark_stage("proc_check")
        proc_after = os.open("proc", flags, dir_fd=args.fixture_fd)
        fds.append(proc_after)
        if mount_id(proc_after) == mount_id(args.fixture_fd):
            raise ValueError("proc mount identity changed")
        require_readonly_mount(proc_after)
        bind_readonly(lib_source, usr, "lib64", libdir, True, "library")
        bind_readonly(args.binary_fd, args.fixture_fd, "test-binary", test_target, False, "binary")
        bind_readonly(tool_sources[0], bindir, "mount", mount_target, False, "mount")
        bind_readonly(tool_sources[1], bindir, "umount", umount_target, False, "umount")
        mark_stage("chroot")
        validate_child_namespace(args.original_mount, args.original_user, args.original_pid, args.parent_uid)
        os.fchdir(args.fixture_fd)
        os.chroot(".")
        os.chdir("/")
        info = os.stat("/")
        if (info.st_dev, info.st_ino) != (held.st_dev, held.st_ino):
            raise ValueError("chroot root identity changed")
    finally:
        for fd in set(fds):
            os.close(fd)
    for entry in os.listdir("/proc/self/fd"):
        if not entry.isdecimal() or int(entry) < 3:
            continue
        try:
            if os.get_inheritable(int(entry)):
                raise ValueError("unexpected inheritable descriptor")
        except OSError:
            pass
    mark_stage("exec")
    os.execve("/test-binary", test_command("/test-binary"), child_environment(args.original_mount))
    raise RuntimeError("test exec failed")


def capture_owned(command: list[str], cwd: str, env: dict[str, str], pass_fds: tuple[int, ...],
                  timeout: int, limit: int = MAX_LOG) -> tuple[int, str, bool]:
    """Bound retained bytes and stop the pinned owned session on overflow."""
    if timeout < 1 or limit < 1:
        raise ValueError("invalid capture limits")
    require_owned_session_support()
    process = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
                               pass_fds=pass_fds, bufsize=0)
    try:
        leader = os.pidfd_open(process.pid)
    except BaseException:
        try:
            terminate_owned(process)
        finally:
            if process.stdout is not None:
                process.stdout.close()
        raise
    output = bytearray()
    started, censored = time.monotonic(), False
    try:
        if process.stdout is None:
            raise RuntimeError("owned child output unavailable")
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() - started >= timeout:
                    censored = True
                    break
                for key, _ in selector.select(timeout=0.05):
                    chunk = os.read(key.fd, min(65536, limit - len(output) + 1))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    elif len(output) + len(chunk) > limit:
                        censored = True
                        break
                    else:
                        output.extend(chunk)
                if censored:
                    break
            while not censored and not child_exited(leader):
                if time.monotonic() - started >= timeout:
                    censored = True
                    break
                time.sleep(0.05)
    finally:
        try:
            terminate_owned(process)
        finally:
            os.close(leader)
            if process.stdout is not None:
                process.stdout.close()
    return process.returncode, output.decode("utf-8", errors="replace"), censored


def run(binary: Path, proof: Path, output: Path, timeout: int, trace_mount: bool = False) -> None:
    if not 1 <= timeout <= 300 or output.exists() or output.is_symlink():
        raise ValueError("invalid timeout or existing output")
    if os.geteuid() == 0:
        raise ValueError("unprivileged caller required")
    require_owned_session_support()
    binary = validate_binary(binary)
    proof_hash = validate_build_proof(binary, proof)
    binary_hash, launcher_hash = file_digest(binary), file_digest(Path(__file__))
    unshare = trusted_tool_paths()
    if trace_mount:
        trusted_trace_path()
    original = {name: os.readlink("/proc/self/ns/" + name) for name in ("mnt", "user", "pid")}
    with retained_fixture() as name:
        fixture = Path(name)
        fixture_fd = os.open(fixture, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        binary_fd = os.open(binary, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
        try:
            fi = os.fstat(fixture_fd)
            if fi.st_uid != os.geteuid() or stat.S_IMODE(fi.st_mode) != 0o700:
                raise ValueError("fixture is not privately owned")
            args = [str(unshare), "--user", "--map-root-user", "--mount", "--pid", "--fork", "--kill-child=KILL",
                    "--propagation", "private", "--", sys.executable, str(Path(__file__).resolve()),
                    "--internal-child", "--original-mount", original["mnt"], "--original-user", original["user"],
                    "--original-pid", original["pid"],
                    "--parent-uid", str(os.geteuid()), "--binary-fd", str(binary_fd),
                    "--fixture-fd", str(fixture_fd), "--fixture-device", str(fi.st_dev),
                    "--fixture-inode", str(fi.st_ino), "--binary-sha256", binary_hash,
                    *(["--trace-mount"] if trace_mount else [])]
            fixture_path = str(fixture_reference(fixture_fd))
            child_env = {"PATH": "/usr/bin", "HOME": fixture_path, "TMPDIR": fixture_path}
            current = fixture.stat(follow_symlinks=False)
            if (current.st_dev, current.st_ino) != (fi.st_dev, fi.st_ino):
                raise ValueError("fixture directory identity changed")
            returncode, text, censored = capture_owned(args, fixture_path, child_env,
                                                        (binary_fd, fixture_fd), timeout)
            stage, reason, mount_reason, syscall_trace = sanitized_setup(text)
            passed = bool(re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", text))
            status = "censored" if censored else ("verified" if returncode == 0 and passed else "failed")
            result = {"status": status, "exit_code": returncode,
                      "last_setup_stage": stage, "setup_failure_reason": reason,
                      "mount_failure_detail": mount_reason, "trace_mount": trace_mount,
                      "mount_syscall_labels": syscall_trace,
                      "exact_test_passed": passed, "test_binary_sha256": binary_hash,
                      "build_provenance_sha256": proof_hash, "launcher_sha256": launcher_hash,
                      "parent_mount_namespace_unchanged": os.readlink("/proc/self/ns/mnt") == original["mnt"],
                      "parent_user_namespace_unchanged": os.readlink("/proc/self/ns/user") == original["user"],
                      "parent_pid_namespace_unchanged": os.readlink("/proc/self/ns/pid") == original["pid"],
                      "fixture_entry_unchanged": fixture_entry_is_original(fixture, fixture_fd),
                      "fixture_cleanup": "retained_for_operator_inspection",
                      "fixture_basename": fixture.name,
                      "limitations": ["test-only namespace topology check, not production custody",
                                      "fixture is retained to avoid pathname replacement cleanup races"]}
        finally:
            os.close(binary_fd)
            os.close(fixture_fd)
    if (not result["fixture_entry_unchanged"] or not result["parent_mount_namespace_unchanged"]
            or not result["parent_user_namespace_unchanged"] or not result["parent_pid_namespace_unchanged"]
            or file_digest(binary) != binary_hash or file_digest(Path(__file__)) != launcher_hash
            or validate_build_proof(binary, proof) != proof_hash):
        result["status"] = "invalidated"
    with output.open("x", encoding="utf-8") as destination:
        json.dump(result, destination, indent=2, sort_keys=True)
        destination.write("\n")
    if result["status"] != "verified":
        raise RuntimeError("bind-alias test did not complete")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", type=Path)
    parser.add_argument("--build-provenance", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout-seconds", type=int, default=120)
    parser.add_argument("--trace-mount", action="store_true", help="diagnose setup mount syscall/errno only")
    parser.add_argument("--internal-child", action="store_true", help=argparse.SUPPRESS)
    for flag in ("original-mount", "original-user", "original-pid", "binary-sha256"):
        parser.add_argument("--" + flag, help=argparse.SUPPRESS)
    for flag in ("parent-uid", "binary-fd", "fixture-fd", "fixture-device", "fixture-inode"):
        parser.add_argument("--" + flag, type=int, help=argparse.SUPPRESS)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        if args.internal_child:
            child(args)
        elif args.test_binary is None or args.build_provenance is None or args.output is None:
            parser.error("--test-binary, --build-provenance and --output are required")
        else:
            run(args.test_binary, args.build_provenance, args.output, args.timeout_seconds, args.trace_mount)
    except (OSError, ValueError, RuntimeError, InterruptedError, subprocess.SubprocessError) as error:
        if args.internal_child:
            reason = ("mount_command_failed" if isinstance(error, subprocess.CalledProcessError)
                      else FAILURES.get(str(error), "other"))
            print("bind_alias_reason=" + reason, flush=True)
        print(f"Bind-alias namespace test failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
