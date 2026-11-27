#!/usr/bin/env python3
"""Test-only FFmpeg shared-decode feasibility; never modifies production code.

Generates disposable media, compares separate fpcalc/full-decode against an
optional FFmpeg Chromaprint muxer, and records fingerprint/duration equality.
Linux pidfds are required for bounded child-session cleanup. Optional strace
records logical source-read syscalls, not physical I/O. This is not an import.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation, ROUND_HALF_UP
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from build_perf_server import file_digest
from perf_media_io_trace import child_exited, clean_environment, parse_trace, require_owned_session_support, terminate_owned

ROOT = Path(__file__).resolve().parents[1]
MAX_CAPTURE = 2 * 1024 * 1024
TRACE_LIMIT = 64 * 1024 * 1024
FILTERS = {
    "trim-before-resample": "[0:a:0]asplit=2[check][fp];[fp]atrim=duration=120[cut]",
    "resample-then-samples": "[0:a:0]asplit=2[check][fp];[fp]aresample=11025:osf=s16:ochl=mono,atrim=end_sample=1323000[cut]",
    "fpcalc-compatible-resampler": "[0:a:0]asplit=2[check][fp];[fp]aresample=11025:osf=s16:ochl=mono:filter_size=16:phase_shift=8:linear_interp=1:cutoff=0.8,atrim=end_sample=1323000[cut]",
}


def milliseconds(value: object) -> int | None:
    try:
        number = Decimal(str(value))
        if not number.is_finite() or number < 0:
            return None
        return int((number * 1000).quantize(Decimal("1"), rounding=ROUND_HALF_UP))
    except (InvalidOperation, ValueError):
        return None


def candidate_command(source: Path, output: Path, muxer_available: bool, realtime: bool = False,
                      variant: str = "trim-before-resample",
                      sample_output: Path | None = None) -> list[str] | None:
    if not muxer_available:
        return None
    graph = FILTERS[variant]
    if sample_output is not None:
        graph += ";[cut]asplit=2[fingerprint][samplecheck]"
    command = ["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-n", *(["-re"] if realtime else []),
            "-i", str(source), "-filter_complex", graph, "-map", "[check]", "-f", "null", "-",
            "-map", "[fingerprint]" if sample_output is not None else "[cut]", "-c:a", "pcm_s16le", "-algorithm", "1", "-fp_format", "base64",
            "-f", "chromaprint", str(output)]
    if sample_output is not None:
        command += ["-map", "[samplecheck]", "-c:a", "pcm_s16le", "-f", "s16le", str(sample_output)]
    return command


def run_command(args: list[str], cwd: Path, env: dict[str, str], timeout: float,
                trace_prefix: Path | None = None) -> dict:
    require_owned_session_support()
    started = time.monotonic()
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stdout:
        process = subprocess.Popen(args, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                   stdout=stdout, stderr=subprocess.DEVNULL, start_new_session=True)
        try:
            leader_fd = os.pidfd_open(process.pid)
        except BaseException:
            terminate_owned(process)
            raise
        timed_out = False
        try:
            while not child_exited(leader_fd):
                if time.monotonic() - started >= timeout:
                    timed_out = True
                    break
                if trace_prefix is not None and sum(path.stat().st_size for path in
                        trace_prefix.parent.glob(trace_prefix.name + ".*")) > TRACE_LIMIT:
                    raise RuntimeError("trace size bound exceeded")
                time.sleep(0.05)
        finally:
            try:
                terminate_owned(process)
            finally:
                os.close(leader_fd)
        if stdout.tell() > MAX_CAPTURE:
            raise RuntimeError("child stdout bound exceeded")
        stdout.seek(0)
        return {"exit_code": process.returncode, "timed_out": timed_out,
                "elapsed_seconds": round(time.monotonic() - started, 6), "stdout": stdout.read()}


def with_trace(args: list[str], root: Path, env: dict[str, str], name: str,
               source: Path, work: Path) -> tuple[dict, dict]:
    prefix = root / name
    command = ["strace", "-ff", "-qq", "-yy", "-s", "128", "-e",
               "trace=read,pread64,readv,preadv,write,pwrite64,writev,pwritev,mmap,execve,copy_file_range,sendfile,splice",
               "-o", str(prefix), *args]
    result = run_command(command, root, env, 90, prefix)
    files = sorted(root.glob(name + ".*"))
    if not files:
        raise RuntimeError("strace emitted no files")
    summary = parse_trace(files, source, work)
    for path in files:
        path.unlink()
    return result, summary


def fingerprint(stdout: str) -> tuple[str | None, object]:
    try:
        parsed = json.loads(stdout)
        return parsed.get("fingerprint"), parsed.get("duration")
    except (ValueError, AttributeError):
        return None, None


def ffprobe_duration(stdout: str) -> object:
    try:
        parsed = json.loads(stdout)
        stream = next(item for item in parsed["streams"] if item.get("codec_type") == "audio")
        return stream.get("duration") or parsed.get("format", {}).get("duration")
    except (ValueError, KeyError, TypeError, StopIteration):
        return None


def fpcalc_like_duration(stdout: str) -> int | None:
    """Test-only approximation of fpcalc stream duration and JSON rounding."""
    try:
        parsed = json.loads(stdout)
        stream = next(item for item in parsed["streams"] if item.get("codec_type") == "audio")
        if "duration_ts" in stream:
            ticks = int(stream["duration_ts"])
            numerator, denominator = map(int, stream["time_base"].split("/"))
            if ticks < 0 or numerator <= 0 or denominator <= 0:
                return None
            duration_ms = ticks * numerator * 1000 // denominator
        else:
            duration = Decimal(str(parsed["format"]["duration"]))
            if not duration.is_finite() or duration < 0:
                return None
            duration_ms = int(duration * 1000)
        # fpcalc renders the integer-millisecond duration with printf("%.2f").
        return milliseconds(format(duration_ms / 1000.0, ".2f"))
    except (InvalidOperation, ValueError, KeyError, TypeError, StopIteration, ZeroDivisionError):
        return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new sanitized JSON file")
    parser.add_argument("--repeats", type=int, default=2, help="untraced wall-time repetitions, 1..3")
    parser.add_argument("--variant", choices=sorted(FILTERS), default="trim-before-resample")
    parser.add_argument("--verify-samples", action="store_true", help="write owned PCM branch to count samples (affects timing)")
    parser.add_argument("--trace-io", action="store_true", help="add strace logical source-read samples")
    parser.add_argument("--force-fallback", action="store_true", help="test unavailable-muxer fallback")
    args = parser.parse_args()
    if args.verify_samples and args.variant != "fpcalc-compatible-resampler":
        parser.error("sample verification requires fpcalc-compatible-resampler")
    if not 1 <= args.repeats <= 3:
        parser.error("repeats must be 1..3")
    if args.output.exists() or args.output.is_symlink():
        parser.error("output must not already exist")
    if sys.platform != "linux" or not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        parser.error("test-only prototype requires Linux pidfd cleanup")
    for tool in ("ffmpeg", "ffprobe", "fpcalc", *(["strace"] if args.trace_io else [])):
        if shutil.which(tool) is None:
            parser.error(f"required tool not found: {tool}")
    cases = [("mp3", "mp3", 44100, 1, 15, ["-c:a", "libmp3lame", "-q:a", "4"]),
             ("flac", "flac", 48000, 2, 15, ["-c:a", "flac"]),
             ("aac", "m4a", 48000, 1, 15, ["-c:a", "aac"]),
             ("opus", "ogg", 48000, 2, 15, ["-c:a", "libopus"]),
             ("mp3_long", "mp3", 44100, 2, 125, ["-c:a", "libmp3lame", "-q:a", "4"]),
             ("flac_before_cutoff", "flac", 32000, 1, 119.9, ["-c:a", "flac"]),
             ("flac_at_cutoff", "flac", 44100, 2, 120, ["-c:a", "flac"]),
             ("flac_after_cutoff", "flac", 48000, 1, 120.1, ["-c:a", "flac"]),
             ("aac_high_rate", "m4a", 96000, 2, 17.123, ["-c:a", "aac"]),
             ("opus_odd_duration", "ogg", 48000, 1, 13.047, ["-c:a", "libopus"])]
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        with tempfile.TemporaryDirectory(prefix="melodee-shared-decode-") as directory:
            root = Path(directory).resolve()
            (root / ".env").touch(exist_ok=False)
            source, work = root / "source", root / "work"
            source.mkdir(); work.mkdir()
            env = clean_environment(root)
            capability = run_command(["ffmpeg", "-hide_banner", "-h", "muxer=chromaprint"], root, env, 10)
            available = not args.force_fallback and capability["exit_code"] == 0 and "Muxer chromaprint" in capability["stdout"]
            versions = {}
            for tool in ("ffmpeg", "ffprobe", "fpcalc"):
                version = run_command([tool, "-version"], root, env, 10)
                versions[tool] = version["stdout"].splitlines()[0][:512] if version["exit_code"] == 0 and version["stdout"] else None
            report = {"description": "test-only generated media comparison, not production ingestion",
                      "timestamp_utc": datetime.now(timezone.utc).isoformat(),
                      "platform": platform.platform(), "logical_cpus": os.cpu_count(),
                      "script_sha256": file_digest(Path(__file__)),
                      "tool_versions": versions,
                      "repeats": args.repeats, "trace_io": args.trace_io, "verify_samples": args.verify_samples,
                      "muxer_available": available, "forced_fallback": args.force_fallback,
                      "variant": args.variant, "filter": FILTERS[args.variant], "candidate_algorithm": 1, "fpcalc_algorithm": "default",
                      "commands": {"decode": ["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-i", "<source>", "-map", "0:a:0", "-f", "null", "-"],
                          "fpcalc": ["fpcalc", "-json", "-length", "120", "--", "<source>"],
                          "probe": ["ffprobe", "-v", "error", "-print_format", "json", "-show_format", "-show_streams", "--", "<source>"],
                          "candidate": candidate_command(Path("<source>"), Path("<output>"), True, variant=args.variant, sample_output=Path("<samples>") if args.verify_samples else None)},
                      "cases": [], "limitations": ["generated signals, not real music",
                          "untraced wall time includes process startup and filesystem cache is uncontrolled",
                          "trace bytes are logical returned syscall counts, not physical disk I/O",
                          "strace affects timing; incomplete calls, mmap faults and native library reads may be unattributed",
                          "candidate does not independently produce fpcalc's duration value",
                          "fpcalc-like duration is an unvalidated approximation; format duration is rendered decimal",
                          "test supervisor is not the production subprocess supervisor",
                          "optional PCM counting changes the candidate graph and runtime"]}
            for name, extension, rate, channels, seconds, encoding in cases:
                path = source / f"{name}.{extension}"
                generated = run_command(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                    f"aevalsrc=0.2*sin(2*PI*(220+8*t)*t)+0.1*sin(2*PI*(380+3*t)*t):s={rate}:d={seconds}",
                    "-ac", str(channels), *encoding, str(path)], root, env, 90)
                if generated["exit_code"] != 0 or generated["timed_out"]:
                    raise RuntimeError("fixture generation failed")
                probe = run_command(["ffprobe", "-v", "error", "-print_format", "json", "-show_format",
                                     "-show_streams", "--", str(path)], root, env, 30)
                duration = ffprobe_duration(probe["stdout"])
                estimated_duration_ms = fpcalc_like_duration(probe["stdout"])
                rows = []
                for iteration in range(args.repeats):
                    decode = run_command(["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-i", str(path),
                                          "-map", "0:a:0", "-f", "null", "-"], root, env, 90)
                    baseline = run_command(["fpcalc", "-json", "-length", "120", "--", str(path)], root, env, 90)
                    original, fp_duration = fingerprint(baseline["stdout"])
                    output = work / f"{name}-{iteration}.fp"
                    sample_output = work / f"{name}-{iteration}.pcm" if args.verify_samples else None
                    command = candidate_command(path, output, available, variant=args.variant, sample_output=sample_output)
                    candidate = run_command(command, root, env, 90) if command else None
                    if output.exists() and output.stat().st_size > MAX_CAPTURE:
                        raise RuntimeError("fingerprint output bound exceeded")
                    observed = output.read_text().strip() if output.exists() else None
                    sample_bytes = sample_output.stat().st_size if sample_output and sample_output.exists() else None
                    if sample_bytes is not None and (sample_bytes > 2646000 or sample_bytes % 2):
                        raise RuntimeError("PCM sample count exceeded its bound")
                    rows.append({"fingerprint_input_samples": sample_bytes // 2 if sample_bytes is not None else None,
                                 "decode_exit": decode["exit_code"], "fpcalc_exit": baseline["exit_code"],
                                 "decode_seconds": decode["elapsed_seconds"], "fpcalc_seconds": baseline["elapsed_seconds"],
                                 "candidate_exit": candidate["exit_code"] if candidate else None,
                                 "candidate_seconds": candidate["elapsed_seconds"] if candidate else None,
                                 "baseline_fingerprint": original, "candidate_fingerprint": observed,
                                 "fingerprints_equal": bool(original and observed and original == observed),
                                 "fpcalc_duration": fp_duration,
                                 "ffprobe_duration": duration,
                                 "fpcalc_like_duration_ms": estimated_duration_ms,
                                 "fpcalc_like_duration_equal": estimated_duration_ms is not None and milliseconds(fp_duration) == estimated_duration_ms,
                                 "duration_ms_equal": milliseconds(fp_duration) is not None and milliseconds(fp_duration) == milliseconds(duration)})
                case = {"codec": name, "sample_rate_hz": rate, "channels": channels, "encoding_options": encoding,
                        "generated_seconds": seconds, "source_bytes": path.stat().st_size,
                        "fallback": not available, "samples": rows}
                if args.trace_io and available:
                    decoded, trace_decode = with_trace(["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-i", str(path), "-map", "0:a:0", "-f", "null", "-"], root, env, f"trace-{name}-decode", source, work)
                    calculated, trace_fpcalc = with_trace(["fpcalc", "-json", "-length", "120", "--", str(path)], root, env, f"trace-{name}-fpcalc", source, work)
                    traced, trace_shared = with_trace(candidate_command(path, work / f"{name}-traced.fp", True, variant=args.variant), root, env, f"trace-{name}-shared", source, work)
                    case["trace"] = {"decode_exit": decoded["exit_code"], "fpcalc_exit": calculated["exit_code"],
                                     "shared_exit": traced["exit_code"], "decode": trace_decode,
                                     "fpcalc": trace_fpcalc, "shared": trace_shared}
                report["cases"].append(case)
            # fpcalc selects FFmpeg's best stream; the proposed candidate maps the first.
            # Multi-stream input must therefore fail closed unless selection is proven equal.
            multi = source / "multi.mka"
            generated = run_command(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                "sine=frequency=311:sample_rate=44100:duration=15", "-f", "lavfi", "-i",
                "sine=frequency=997:sample_rate=44100:duration=15", "-map", "0:a:0", "-map", "1:a:0",
                "-disposition:a:0", "0", "-disposition:a:1", "default", "-c:a", "flac", str(multi)], root, env, 90)
            multi_probe = run_command(["ffprobe", "-v", "error", "-print_format", "json", "-show_streams", "--", str(multi)], root, env, 30)
            if generated["exit_code"] != 0 or multi_probe["exit_code"] != 0:
                raise RuntimeError("multistream generation or probing failed")
            audio_count = sum(stream.get("codec_type") == "audio" for stream in json.loads(multi_probe["stdout"])["streams"])
            multi_baseline = run_command(["fpcalc", "-json", "-length", "120", "--", str(multi)], root, env, 90)
            multi_original, _ = fingerprint(multi_baseline["stdout"])
            multi_output = work / "multi.fp"
            multi_command = candidate_command(multi, multi_output, available, variant=args.variant)
            multi_candidate = run_command(multi_command, root, env, 90) if multi_command else None
            multi_observed = multi_output.read_text().strip() if multi_output.exists() and multi_output.stat().st_size <= MAX_CAPTURE else None
            report["multi_stream"] = {"audio_streams": audio_count, "conservative_fallback": audio_count != 1,
                "fpcalc_exit": multi_baseline["exit_code"], "candidate_exit": multi_candidate["exit_code"] if multi_candidate else None,
                "fingerprints_equal": bool(multi_original and multi_observed and multi_original == multi_observed)}
            report["metadata_cases"] = []
            for label, muxer_options in (("matroska", []), ("matroska_live", ["-live", "1"])):
                metadata_path = source / f"{label}.mka"
                generated = run_command(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                    "sine=frequency=743:sample_rate=48000:duration=15.123", "-c:a", "flac",
                    *muxer_options, "-f", "matroska", str(metadata_path)], root, env, 90)
                probe = run_command(["ffprobe", "-v", "error", "-print_format", "json", "-show_format",
                                     "-show_streams", "--", str(metadata_path)], root, env, 30)
                baseline = run_command(["fpcalc", "-json", "-length", "120", "--", str(metadata_path)], root, env, 90)
                if generated["exit_code"] != 0 or probe["exit_code"] != 0:
                    raise RuntimeError("metadata fixture generation or probing failed")
                parsed_probe = json.loads(probe["stdout"])
                stream = next(item for item in parsed_probe["streams"] if item.get("codec_type") == "audio")
                _, measured_duration = fingerprint(baseline["stdout"])
                estimated = fpcalc_like_duration(probe["stdout"])
                report["metadata_cases"].append({"case": label, "stream_duration_ts": stream.get("duration_ts"),
                    "format_duration": parsed_probe.get("format", {}).get("duration"),
                    "fpcalc_exit": baseline["exit_code"], "fpcalc_duration": measured_duration,
                    "fpcalc_like_duration_ms": estimated,
                    "approximation_equal": estimated is not None and estimated == milliseconds(measured_duration)})
            # A truncated FLAC is tested against both full-decode commands.
            valid = source / "flac.flac"
            damaged = source / "truncated.flac"
            data = valid.read_bytes(); damaged.write_bytes(data[:len(data) * 2 // 3])
            baseline = run_command(["ffmpeg", "-nostdin", "-v", "error", "-xerror", "-i", str(damaged), "-map", "0:a:0", "-f", "null", "-"], root, env, 90)
            candidate_args = candidate_command(damaged, work / "truncated.fp", available, variant=args.variant)
            candidate = run_command(candidate_args, root, env, 90) if candidate_args else None
            report["truncated_flac"] = {"baseline_exit": baseline["exit_code"], "candidate_exit": candidate["exit_code"] if candidate else None,
                                        "both_rejected": baseline["exit_code"] != 0 and candidate is not None and candidate["exit_code"] != 0}
            if available:
                timed = run_command(candidate_command(source / "mp3_long.mp3", work / "timed.fp", True, True, variant=args.variant), root, env, 1)
                report["timeout_cleanup"] = {"timed_out": timed["timed_out"], "exit_code": timed["exit_code"],
                                             "supervisor": "owned session and pidfd cleanup; child groups verified empty"}
            report["all_fingerprints_equal"] = (all(sample["fingerprints_equal"] for case in report["cases"]
                                                        for sample in case["samples"]) if available else None)
            report["all_durations_equal"] = (all(sample["duration_ms_equal"] for case in report["cases"]
                                                     for sample in case["samples"]) if available else None)
            report["production_eligibility"] = "not established"
            with args.output.open("x", encoding="utf-8") as output:
                json.dump(report, output, indent=2, sort_keys=True); output.write("\n")
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"shared decode experiment failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
