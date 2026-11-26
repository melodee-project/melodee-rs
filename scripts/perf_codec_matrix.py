#!/usr/bin/env python3
"""Measure real media normalization over a disposable generated codec matrix.

This is a stage benchmark, not end-to-end ingestion: it exercises the production
inspection and normalization functions, without PostgreSQL, HTTP, or the worker
scheduler. FFmpeg receives the application's unchanged default arguments.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time

try:
    import resource
except ImportError:
    resource = None

ROOT = Path(__file__).resolve().parents[1]
CODECS = {
    "mp3": ("mp3", ["-c:a", "libmp3lame", "-q:a", "4"]),
    "flac": ("flac", ["-c:a", "flac"]),
    "ogg": ("ogg", ["-c:a", "libvorbis", "-q:a", "4"]),
    "wav": ("wav", ["-c:a", "pcm_s16le"]),
}


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be positive")
    return parsed


def command_output(args: list[str]) -> str:
    return subprocess.run(args, cwd=ROOT, text=True, check=True, capture_output=True).stdout.strip()


def build() -> Path:
    args = ["cargo", "test", "-p", "melodee-media", "--release", "--test",
            "perf_normalization_matrix", "--no-run", "--message-format=json"]
    process = subprocess.run(args, cwd=ROOT, text=True, check=True, stdout=subprocess.PIPE)
    for line in process.stdout.splitlines():
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact"
                and message.get("target", {}).get("name") == "perf_normalization_matrix"
                and message.get("executable")):
            return Path(message["executable"])
    raise RuntimeError("Cargo did not report the benchmark executable")


def usage():
    return resource.getrusage(resource.RUSAGE_CHILDREN) if resource else None


def cpu_description() -> str:
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text(encoding="utf-8").splitlines():
            if line.startswith("model name") and ":" in line:
                return line.split(":", 1)[1].strip()
    return platform.processor() or "unknown"


def process_tree_rss_kib(root_pid: int) -> int | None:
    """Sample Linux process-tree RSS; unavailable on other platforms."""
    proc = Path("/proc")
    if not proc.is_dir():
        return None
    processes = {}
    for path in proc.iterdir():
        if not path.name.isdecimal():
            continue
        try:
            status = (path / "status").read_text(encoding="utf-8")
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
        try:
            processes[int(path.name)] = (int(fields["PPid"]), int(fields.get("VmRSS", "0").split()[0]))
        except (KeyError, ValueError, IndexError):
            continue
    children = {root_pid}
    while True:
        expanded = children | {pid for pid, (parent, _) in processes.items() if parent in children}
        if expanded == children:
            break
        children = expanded
    return sum(processes.get(pid, (0, 0))[1] for pid in children)


def run_benchmark(executable: Path, env: dict[str, str]) -> tuple[str, int | None]:
    process = subprocess.Popen([str(executable), "--ignored", "--nocapture", "codec_worker_matrix"],
                               cwd=ROOT, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    peak = None
    while True:
        sample = process_tree_rss_kib(process.pid)
        if sample is not None:
            peak = max(peak or 0, sample)
        try:
            stdout, stderr = process.communicate(timeout=0.05)
            break
        except subprocess.TimeoutExpired:
            continue
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, process.args, stdout, stderr)
    return stdout, peak


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workers", type=positive_int, nargs="+", default=[1, 2, 4])
    parser.add_argument("--releases", type=positive_int, default=8)
    parser.add_argument("--seconds", type=positive_int, default=60)
    parser.add_argument("--repeats", type=positive_int, default=2)
    parser.add_argument("--codecs", nargs="+", choices=sorted(CODECS), default=list(CODECS))
    parser.add_argument("--output", type=Path, required=True, help="new JSON evidence file; existing files are never overwritten")
    args = parser.parse_args()
    if max(args.workers) > args.releases:
        parser.error("workers must not exceed releases")
    if max(args.workers) > 8 or args.releases > 128 or args.seconds > 600 or args.repeats > 10:
        parser.error("limits: workers <= 8, releases <= 128, seconds <= 600, repeats <= 10")
    if args.output.exists():
        parser.error("output must not already exist")
    for tool in ("cargo", "ffmpeg", "ffprobe", "fpcalc", "git"):
        if shutil.which(tool) is None:
            parser.error(f"required tool not found: {tool}")
    executable = build()
    results = {
        "description": "production media normalization stage only; single-track synthetic releases",
        "commit": command_output(["git", "rev-parse", "HEAD"]),
        "working_tree_dirty": bool(command_output(["git", "status", "--porcelain"])),
        "build_profile": "release", "platform": platform.platform(),
        "cpu_count": os.cpu_count(), "processor": cpu_description(),
        "ffmpeg_version": command_output(["ffmpeg", "-version"]).splitlines()[0],
        "ffprobe_version": command_output(["ffprobe", "-version"]).splitlines()[0],
        "fpcalc_version": command_output(["fpcalc", "-version"]).splitlines()[0],
        "source_sha256": {
            str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in [ROOT / "Cargo.lock", ROOT / "crates/melodee-media/src/lib.rs",
                         ROOT / "crates/melodee-media/src/normalize.rs",
                         ROOT / "crates/melodee-media/src/secure_fs.rs",
                         ROOT / "crates/melodee-media/tests/perf_normalization_matrix.rs"]
        },
        "sample_audio_seconds": args.seconds, "repeats": args.repeats,
        "cache_conditions": "warm: same generated source reused; no cache eviction",
        "limitations": ["no PostgreSQL, full worker scheduler, API, search, or network traffic",
                        "FFmpeg defaults only; no FFmpeg thread-count matrix or interactive contention",
                        "Linux process-tree peak RSS is sampled every 50 ms and can miss brief peaks",
                        "RUSAGE_CHILDREN max RSS is cumulative high-water mark, not case-specific"],
        "cases": [],
    }
    with tempfile.TemporaryDirectory(prefix="melodee-codec-matrix-") as directory:
        root = Path(directory)
        results["temporary_filesystem_free_bytes"] = shutil.disk_usage(root).free
        for codec in args.codecs:
            extension, options = CODECS[codec]
            source = root / codec
            source.mkdir()
            subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                            f"sine=frequency=440:sample_rate=48000:duration={args.seconds}",
                            *options, str(source / f"track.{extension}")], check=True, cwd=ROOT)
            for workers in args.workers:
                for iteration in range(args.repeats):
                    work = root / f"work-{codec}-{workers}-{iteration}"
                    work.mkdir()
                    env = os.environ.copy()
                    env.update(MELODEE_PERF_SOURCE=str(source), MELODEE_PERF_WORK=str(work),
                               MELODEE_PERF_WORKERS=str(workers), MELODEE_PERF_RELEASES=str(args.releases))
                    before = usage()
                    started = time.monotonic()
                    stdout, peak_rss = run_benchmark(executable, env)
                    after = usage()
                    marker = "MELODEE_CODEC_MATRIX_RESULT="
                    line = next((line for line in stdout.splitlines() if marker in line), None)
                    if line is None:
                        raise RuntimeError("benchmark result missing from test output")
                    case = json.loads(line.split(marker, 1)[1])
                    case.update(codec=codec, iteration=iteration + 1,
                                source_file_bytes=(source / f"track.{extension}").stat().st_size,
                                process_wall_seconds=time.monotonic() - started,
                                sampled_process_tree_peak_rss_kib=peak_rss,
                                output_bytes=sum(p.stat().st_size for p in work.rglob("*") if p.is_file()))
                    if before is not None and after is not None:
                        case.update(child_user_seconds=after.ru_utime - before.ru_utime,
                                    child_system_seconds=after.ru_stime - before.ru_stime,
                                    cumulative_children_maxrss_platform_units=after.ru_maxrss,
                                    child_input_blocks=after.ru_inblock - before.ru_inblock,
                                    child_output_blocks=after.ru_oublock - before.ru_oublock)
                    results["cases"].append(case)
                    shutil.rmtree(work)
    with args.output.open("x", encoding="utf-8") as output:
        json.dump(results, output, indent=2)
        output.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
