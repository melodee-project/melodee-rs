#!/usr/bin/env python3
"""Build the debug ingestion libtest and attest its exact source and binary."""
from __future__ import annotations
import argparse
import json
import os
import selectors
import time
from pathlib import Path
import subprocess
import sys

from build_perf_server import command_output, file_digest, source_fingerprints
from perf_media_io_trace import child_exited, require_owned_session_support, terminate_owned

ROOT = Path(__file__).resolve().parents[1]


def capture_cargo(command: list[str], repo: Path, env: dict[str, str],
                  limit: int = 32 * 1024 * 1024, timeout: int = 3600) -> str:
    """Capture at most limit bytes while retaining the unreaped session leader."""
    require_owned_session_support()
    if limit <= 0 or timeout <= 0:
        raise ValueError("invalid build capture limits")
    process = subprocess.Popen(command, cwd=repo, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, start_new_session=True, bufsize=0)
    try:
        leader = os.pidfd_open(process.pid)
    except BaseException:
        terminate_owned(process)
        raise
    chunks = bytearray()
    started = time.monotonic()
    try:
        assert process.stdout is not None
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() - started >= timeout:
                    raise TimeoutError("owned build timed out")
                for key, _ in selector.select(timeout=0.1):
                    chunk = os.read(key.fd, min(65536, limit - len(chunks) + 1))
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        if len(chunks) + len(chunk) > limit:
                            raise RuntimeError("build output exceeded limit")
                        chunks.extend(chunk)
            while not child_exited(leader):
                if time.monotonic() - started >= timeout:
                    raise TimeoutError("owned build timed out")
                time.sleep(0.05)
    finally:
        try:
            terminate_owned(process)
        finally:
            os.close(leader)
            if process.stdout is not None:
                process.stdout.close()
    output = chunks.decode("utf-8", errors="replace")
    if process.returncode:
        raise subprocess.CalledProcessError(process.returncode, command, output=output)
    return output


def emit_compiler_diagnostics(output: str | bytes | None) -> None:
    """Expose only bounded rendered Cargo compiler diagnostics on failure."""
    if isinstance(output, bytes):
        output = output.decode("utf-8", errors="replace")
    remaining = 16_384
    for line in (output or "").splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if not isinstance(item, dict) or item.get("reason") != "compiler-message":
            continue
        message = item.get("message")
        rendered = message.get("rendered") if isinstance(message, dict) else None
        if not isinstance(rendered, str) or not rendered:
            continue
        chunk = rendered[:remaining]
        print(chunk, file=sys.stderr, end="" if chunk.endswith("\n") else "\n")
        remaining -= len(chunk)
        if remaining <= 0:
            break


def build(repo: Path, output: Path) -> None:
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    repo = repo.resolve(strict=True)
    head = command_output(["git", "rev-parse", "HEAD"], repo)
    before = source_fingerprints(repo)
    helpers = {name: file_digest(repo / name) for name in (
        "scripts/build_stage_verity_test.py", "scripts/build_perf_server.py",
        "scripts/perf_stage_verity_namespace.py", "scripts/perf_media_io_trace.py")}
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "2"
    env["RUST_TEST_THREADS"] = "1"
    try:
        output_text = capture_cargo(["cargo", "test", "--locked", "-p", "melodee-ingestion", "--features", "test-support",
                                     "--lib", "--no-run", "--message-format=json-render-diagnostics", "--target-dir", str(repo / "target")],
                                    repo, env)
    except subprocess.CalledProcessError as error:
        emit_compiler_diagnostics(error.stdout)
        raise
    binaries = set()
    for line in output_text.splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if (item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "melodee_ingestion"
                and item.get("profile", {}).get("test") and item.get("executable")):
            binaries.add(Path(item["executable"]))
    if len(binaries) != 1:
        raise RuntimeError("expected one ingestion libtest executable")
    binary = binaries.pop()
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("invalid ingestion test executable")
    if (source_fingerprints(repo) != before or command_output(["git", "rev-parse", "HEAD"], repo) != head
            or helpers != {name: file_digest(repo / name) for name in helpers}):
        raise RuntimeError("source or helper changed during build")
    record = {"target": "sealed-tree-libtest", "profile": "debug", "features": ["test-support"],
              "commit": head, "source_sha256": before, "script_sha256": helpers,
              "binary_name": binary.name, "binary_sha256": file_digest(binary),
              "limitations": ["matching hashes cannot exclude transient edits",
                  "dependency and toolchain binaries are not independently attested"]}
    with output.open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2, sort_keys=True)
        destination.write("\n")
    print(json.dumps({"binary_name": binary.name, "sha256": record["binary_sha256"]}))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        build(args.repo, args.output)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"Sealed-tree test build failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
