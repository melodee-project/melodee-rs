#!/usr/bin/env python3
"""Build and run the pinned Android Auto instrumentation probe."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXPECTED_COMMIT = "7008eed187f42940b6614fe3438cf02fa5a07d94"
PROBE_RELATIVE_PATH = Path(
    "app/src/androidTest/java/com/melodee/autoplayer/Phase7RealServerAndroidAutoTest.kt"
)
PROBE_CLASS = "com.melodee.autoplayer.Phase7RealServerAndroidAutoTest"


class ProbeError(RuntimeError):
    """The external Android client checkout is not safe to modify temporarily."""


def default_client_root() -> Path:
    return ROOT.parent / "melodee-player"


def gradle_wrapper(client_source: Path, platform: str = os.name) -> Path:
    return client_source / ("gradlew.bat" if platform == "nt" else "gradlew")


def captured(command: list[str], *, cwd: Path) -> str:
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True)
    return result.stdout.strip()


def run_probe() -> None:
    client_root = Path(
        os.environ.get("MELODEE_ANDROID_CLIENT_ROOT", str(default_client_root()))
    ).expanduser()
    client_source = client_root / "src"
    base_url = os.environ.get("MELODEE_ANDROID_CLIENT_BASE_URL", "")
    if not base_url:
        raise ProbeError("set MELODEE_ANDROID_CLIENT_BASE_URL to the disposable Rust fixture URL")
    email = os.environ.get("MELODEE_ANDROID_CLIENT_EMAIL", "android-client@example.test")
    password = os.environ.get("MELODEE_ANDROID_CLIENT_PASSWORD", "android-client-password")

    actual_commit = captured(["git", "rev-parse", "HEAD"], cwd=client_root)
    if actual_commit != EXPECTED_COMMIT:
        raise ProbeError(
            f"Android client commit mismatch: expected {EXPECTED_COMMIT}, found {actual_commit}"
        )
    if captured(["git", "status", "--short"], cwd=client_root):
        raise ProbeError("Android client checkout must be clean")

    probe_source = ROOT / "compatibility/clients/android" / PROBE_RELATIVE_PATH.name
    probe_target = client_source / PROBE_RELATIVE_PATH
    if probe_target.exists():
        raise ProbeError(f"Probe target already exists: {probe_target}")
    if not probe_source.is_file():
        raise ProbeError(f"Probe source does not exist: {probe_source}")

    probe_target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(probe_source, probe_target)
    try:
        wrapper = gradle_wrapper(client_source)
        subprocess.run(
            [str(wrapper), "assembleDebug", "assembleDebugAndroidTest"],
            cwd=client_source,
            check=True,
        )
        subprocess.run(
            [
                "adb",
                "install",
                "-r",
                str(client_source / "app/build/outputs/apk/debug/app-debug.apk"),
            ],
            check=True,
        )
        subprocess.run(
            [
                "adb",
                "install",
                "-r",
                str(
                    client_source
                    / "app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
                ),
            ],
            check=True,
        )
        subprocess.run(
            [
                "adb",
                "shell",
                "am",
                "instrument",
                "-w",
                "-r",
                "-e",
                "class",
                PROBE_CLASS,
                "-e",
                "baseUrl",
                base_url,
                "-e",
                "email",
                email,
                "-e",
                "password",
                password,
                "com.melodee.autoplayer.test/androidx.test.runner.AndroidJUnitRunner",
            ],
            check=True,
        )
    finally:
        probe_target.unlink(missing_ok=True)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog=(
            "Set MELODEE_ANDROID_CLIENT_BASE_URL. Optional environment: "
            "MELODEE_ANDROID_CLIENT_ROOT, MELODEE_ANDROID_CLIENT_EMAIL, "
            "MELODEE_ANDROID_CLIENT_PASSWORD."
        ),
    )
    return parser.parse_args()


def main() -> int:
    parse_args()
    try:
        run_probe()
    except ProbeError as error:
        print(str(error), file=sys.stderr)
        return 1
    except FileNotFoundError as error:
        print(f"Required command or path not found: {error.filename}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
