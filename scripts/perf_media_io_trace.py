#!/usr/bin/env python3
"""Profile an owned single-track inspection/normalization using Linux strace.

This stage-only profiler runs an already-built release perf_normalization_matrix
test binary on generated audio under a private temporary root. Raw syscall
traces and media are deleted on exit; retained evidence contains only aggregate
counts and sizes. It requires Linux strace and the normal media tools. This is
not a measurement of physical disk I/O or the full ingestion pipeline.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from build_perf_server import file_digest

ROOT = Path(__file__).resolve().parents[1]
TRACE_LIMIT = 64 * 1024 * 1024
FORMATS = {"mp3": ["-c:a", "libmp3lame", "-q:a", "4"],
           "flac": ["-c:a", "flac"]}
CALL = re.compile(r"^(read|pread64|readv|preadv|write|pwrite64|writev|pwritev)\(\d+<([^>]+)>,.*\)\s+=\s+(-?\d+)(?:\s|$)")
KERNEL_COPY = re.compile(r"^(copy_file_range|sendfile|splice)\((.*)\)\s+=\s+(-?\d+)(?:\s|$)")
DESCRIPTOR = re.compile(r"\d+<([^>]+)>")
EXEC = re.compile(r'^execve\("([^"]+)",.*\)\s+=\s+0(?:\s|$)')


def clean_environment(root: Path) -> dict[str, str]:
    allowed = ("PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "SYSTEMROOT", "WINDIR")
    env = {key: os.environ[key] for key in allowed if key in os.environ}
    env.update({"TMPDIR": str(root), "TMP": str(root), "TEMP": str(root)})
    return env


def source_kind(path: str, source: Path, work: Path) -> str | None:
    if path == str(source) or path.startswith(str(source) + os.sep):
        return "source"
    if path == str(work) or path.startswith(str(work) + os.sep):
        return "work"
    return None


def parse_trace(paths: list[Path], source: Path, work: Path) -> dict:
    result: dict = {"file_calls": {}, "executions": {}, "attributed_mmaps": 0,
                    "unparsed_file_calls": 0, "trace_files": len(paths)}
    for path in paths:
        process = "benchmark_or_other"
        with path.open(encoding="utf-8", errors="replace") as trace:
            for line in trace:
                event = EXEC.match(line)
                if event:
                    basename = Path(event.group(1)).name
                    process = basename if basename in {"ffmpeg", "ffprobe", "fpcalc"} else "benchmark_or_other"
                    if process != "benchmark_or_other":
                        kind = "version" if '"-version"' in line else "other"
                        key = f"{process}:{kind}"
                        result["executions"][key] = result["executions"].get(key, 0) + 1
                    continue
                match = CALL.match(line)
                if match:
                    syscall, target, amount = match.groups()
                    bucket = source_kind(target, source, work)
                    if bucket is None:
                        continue
                    key = f"{process}:{bucket}:{'read' if syscall in {'read', 'pread64', 'readv', 'preadv'} else 'write'}"
                    counter = result["file_calls"].setdefault(key, {"calls": 0, "bytes": 0, "errors": 0})
                    counter["calls"] += 1
                    if int(amount) < 0:
                        counter["errors"] += 1
                    else:
                        counter["bytes"] += int(amount)
                    continue
                copy = KERNEL_COPY.match(line)
                if copy:
                    syscall, args, amount = copy.groups()
                    descriptors = DESCRIPTOR.findall(args)
                    if len(descriptors) < 2:
                        result["unparsed_file_calls"] += 1
                        continue
                    inputs, outputs = ((descriptors[1], descriptors[0]) if syscall == "sendfile"
                                       else (descriptors[0], descriptors[1]))
                    for direction, target in (("kernel_read", inputs), ("kernel_write", outputs)):
                        bucket = source_kind(target, source, work)
                        if bucket is None:
                            continue
                        key = f"{process}:{bucket}:{direction}"
                        counter = result["file_calls"].setdefault(key, {"calls": 0, "bytes": 0, "errors": 0})
                        counter["calls"] += 1
                        if int(amount) < 0:
                            counter["errors"] += 1
                        else:
                            counter["bytes"] += int(amount)
                    continue
                if line.startswith(("read(", "pread64(", "readv(", "preadv(",
                                    "write(", "pwrite64(", "writev(", "pwritev(",
                                    "copy_file_range(", "sendfile(", "splice(")):
                    result["unparsed_file_calls"] += 1
                if line.startswith("mmap(") and (str(source) in line or str(work) in line):
                    result["attributed_mmaps"] += 1
    return result


def session_members(session_id: int) -> list[int]:
    """The media supervisor creates child process groups inside our session."""
    result = []
    for path in Path("/proc").iterdir():
        if not path.name.isdecimal():
            continue
        pid = int(path.name)
        try:
            same_session = os.getsid(pid) == session_id
        except (ProcessLookupError, PermissionError):
            continue
        if same_session:
            try:
                status = (path / "status").read_text(encoding="utf-8")
                if not any(line.startswith("State:") and "Z (zombie)" in line for line in status.splitlines()):
                    result.append(pid)
            except (FileNotFoundError, ProcessLookupError):
                continue
    return result


def signal_owned_session(session_id: int, sig: int) -> None:
    # pidfds prevent a recycled PID from targeting a different process. Check
    # session membership after opening each handle and signal the leader last.
    members = session_members(session_id)
    for pid in sorted(members, key=lambda value: value == session_id):
        try:
            fd = os.pidfd_open(pid)
        except ProcessLookupError:
            continue
        try:
            if os.getsid(pid) == session_id:
                signal.pidfd_send_signal(fd, sig)
        except ProcessLookupError:
            pass
        finally:
            os.close(fd)


def terminate_owned(process: subprocess.Popen) -> None:
    if process.poll() is None and os.getsid(process.pid) != process.pid:
        raise RuntimeError("owned profiler session topology changed")
    if session_members(process.pid):
        signal_owned_session(process.pid, signal.SIGTERM)
        deadline = time.monotonic() + 5
        while session_members(process.pid) and time.monotonic() < deadline:
            time.sleep(0.05)
        if session_members(process.pid):
            signal_owned_session(process.pid, signal.SIGKILL)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("owned profiler failed to terminate") from error
    if session_members(process.pid):
        raise RuntimeError("owned profiler descendants remain")


def run_child(args: list[str], cwd: Path, env: dict[str, str], timeout: int,
              trace_prefix: Path | None = None) -> tuple[float, str]:
    started = time.monotonic()
    # The workload has a small controlled stdout; stderr is not retained.
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stdout:
        process = subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=stdout, stderr=subprocess.DEVNULL, start_new_session=True)
        # Keep the session leader's PID identity pinned throughout cleanup so
        # its numeric session ID cannot be recycled into an unrelated session.
        try:
            leader_fd = os.pidfd_open(process.pid)
        except BaseException:
            if process.poll() is None:
                terminate_owned(process)
            raise
        try:
            while process.poll() is None:
                if time.monotonic() - started > timeout:
                    raise TimeoutError("owned profiler timed out")
                if trace_prefix is not None:
                    total = sum(path.stat().st_size for path in trace_prefix.parent.glob(trace_prefix.name + ".*"))
                    if total > TRACE_LIMIT:
                        raise RuntimeError("trace size limit exceeded")
                time.sleep(0.1)
            if process.returncode:
                raise RuntimeError("owned child failed")
            if stdout.tell() > 1024 * 1024:
                raise RuntimeError("owned child output exceeded limit")
            stdout.seek(0)
            return time.monotonic() - started, stdout.read()
        finally:
            try:
                terminate_owned(process)
            finally:
                os.close(leader_fd)


def validate_binary(binary: Path) -> Path:
    resolved = binary.resolve(strict=True)
    release_deps = (ROOT / "target" / "release" / "deps").resolve(strict=True)
    if (resolved.parent != release_deps or not resolved.name.startswith("perf_normalization_matrix-")
            or not resolved.is_file() or not os.access(resolved, os.X_OK)):
        raise ValueError("binary must be a built release perf_normalization_matrix target")
    return resolved


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="release perf_normalization_matrix test executable")
    parser.add_argument("--codec", choices=sorted(FORMATS), default="mp3")
    parser.add_argument("--seconds", type=int, default=5)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("--output", type=Path, required=True, help="new sanitized JSON evidence file")
    args = parser.parse_args()
    if not 1 <= args.seconds <= 60 or not 10 <= args.timeout <= 600:
        parser.error("seconds must be 1..60 and timeout 10..600")
    if args.output.exists() or args.output.is_symlink():
        parser.error("output already exists")
    if sys.platform != "linux":
        parser.error("this optional profiler requires Linux strace")
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        parser.error("this profiler requires Linux pidfd support for owned-child cleanup")
    tools = {name: shutil.which(name) for name in ("strace", "ffmpeg", "ffprobe", "fpcalc")}
    if any(path is None for path in tools.values()):
        parser.error("strace, ffmpeg, ffprobe and fpcalc are required")
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        binary = validate_binary(args.binary)
        binary_hash = file_digest(binary)
        source_paths = [ROOT / "Cargo.lock", ROOT / "crates/melodee-media/src/lib.rs",
                        ROOT / "crates/melodee-media/src/normalize.rs",
                        ROOT / "crates/melodee-media/src/secure_fs.rs",
                        ROOT / "crates/melodee-media/tests/perf_normalization_matrix.rs"]
        source_hashes = {str(path.relative_to(ROOT)): file_digest(path) for path in source_paths}
        repo_env = clean_environment(ROOT)
        revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, env=repo_env,
                                  check=True, text=True, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, timeout=15).stdout.strip()
        dirty = bool(subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, env=repo_env,
                                    check=True, text=True, stdout=subprocess.PIPE,
                                    stderr=subprocess.DEVNULL, timeout=15).stdout.strip())
        tool_versions = {}
        for name, path in tools.items():
            flag = "-V" if name == "strace" else "-version"
            result = subprocess.run([path, flag], cwd=ROOT, env=repo_env, check=True,
                                    text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                    timeout=15)
            tool_versions[name] = result.stdout.splitlines()[0][:512] if result.stdout else "unknown"
        with tempfile.TemporaryDirectory(prefix="melodee-media-trace-") as directory:
            root = Path(directory).resolve()
            (root / ".env").touch(exist_ok=False)
            source, work = root / "source", root / "work"
            source.mkdir()
            work.mkdir()
            audio = source / f"track.{args.codec}"
            env = clean_environment(root)
            run_child([tools["ffmpeg"], "-v", "error", "-f", "lavfi", "-i",
                       f"sine=frequency=440:sample_rate=48000:duration={args.seconds}",
                       *FORMATS[args.codec], str(audio)], root, env, args.timeout)
            env.update(MELODEE_PERF_SOURCE=str(source), MELODEE_PERF_WORK=str(work),
                       MELODEE_PERF_WORKERS="1", MELODEE_PERF_RELEASES="1")
            trace = root / "syscalls"
            elapsed, stdout = run_child([tools["strace"], "-ff", "-qq", "-yy", "-s", "128",
                                         "-e", "trace=read,pread64,readv,preadv,write,pwrite64,writev,pwritev,mmap,execve,copy_file_range,sendfile,splice",
                                         "-o", str(trace), str(binary), "--ignored", "--nocapture",
                                         "codec_worker_matrix"], root, env, args.timeout, trace)
            marker = "MELODEE_CODEC_MATRIX_RESULT="
            line = next((line for line in stdout.splitlines() if marker in line), None)
            if line is None:
                raise RuntimeError("production-path benchmark result missing")
            sample = json.loads(line.split(marker, 1)[1])
            if sample.get("releases") not in (None, 1) or sample.get("workers") != 1:
                raise RuntimeError("unexpected workload shape")
            traces = sorted(root.glob("syscalls.*"))
            if not traces:
                raise RuntimeError("profiler emitted no traces")
            if file_digest(binary) != binary_hash or any(file_digest(path) != source_hashes[str(path.relative_to(ROOT))] for path in source_paths):
                raise RuntimeError("binary or sampled source files changed during measurement")
            report = {"build_profile": "release expected; binary profile not independently verified",
                      "timestamp_utc": datetime.now(timezone.utc).isoformat(),
                      "repository_head": revision, "working_tree_dirty": dirty,
                      "source_sha256": source_hashes, "binary_sha256": binary_hash,
                      "script_sha256": file_digest(Path(__file__)),
                      "platform": platform.platform(), "tool_versions": tool_versions, "codec": args.codec,
                      "audio_seconds": args.seconds, "input_bytes": audio.stat().st_size,
                      "elapsed_seconds": round(elapsed, 6), "workload": sample,
                      "syscalls": parse_trace(traces, source, work),
                      "limitations": ["inspection and normalization combined, not full ingestion",
                                      "strace changes timing; one generated track and uncontrolled cache",
                                      "counts logical syscall return bytes, not physical I/O or hash counts; kernel copies are separate from read/write",
                                      "mmap page faults, untraced syscall variants and unparsed calls are not byte-attributed",
                                      "process attribution starts at observed execve; compare unparsed calls before use",
                                      "sampled source hashes do not prove the binary was built from that source"]}
            with args.output.open("x", encoding="utf-8") as output:
                json.dump(report, output, indent=2, sort_keys=True)
                output.write("\n")
    except (OSError, ValueError, RuntimeError, InterruptedError, TimeoutError,
            subprocess.SubprocessError) as error:
        print(f"media profiler failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
