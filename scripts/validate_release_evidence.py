#!/usr/bin/env python3
"""Create and validate the machine-readable per-architecture image evidence.

The release workflow deliberately keeps publication separate from evidence
capture.  This helper makes the captured bundle independently reviewable: the
image identity, release metadata, required reports, and checksum manifest
must all agree.  It never contacts a registry and it never changes an image.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import stat
import sys
from pathlib import Path
from typing import Any


MANIFEST_NAME = "release-evidence.json"
CHECKSUMS_NAME = "SHA256SUMS"
REQUIRED_FILES = (
    "image-metadata.json",
    "image-digest.txt",
    "image.tar.gz",
    "sbom.spdx.json",
    "trivy-critical.json",
    "trivy-high.json",
    "scan-status.txt",
)
EXPECTED_LABELS = {
    "org.opencontainers.image.title": "Melodee",
    "org.opencontainers.image.source": "https://github.com/melodee-project/melodee-rs",
    "org.opencontainers.image.licenses": "MIT",
}
EXPECTED_RUNTIME_USER = "melodee:melodee"


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{path.name} is not valid UTF-8 JSON") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path.name} must contain a JSON object")
    return value


def require_regular_file(path: Path) -> None:
    """Reject links and non-files before any evidence bytes are consumed."""
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        raise ValueError(f"evidence file is not inspectable: {path.name}") from error
    if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
        raise ValueError(f"evidence file must be a regular non-symlink file: {path.name}")


def contains_symlink_component(path: Path) -> bool:
    """Reject an evidence directory reached through a link at any component."""
    current = Path(path.anchor) if path.is_absolute() else Path(".")
    for component in path.parts:
        if component in {"", path.anchor}:
            continue
        current /= component
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            break
        except OSError:
            return True
        if stat.S_ISLNK(mode):
            return True
    return False


def require_files(directory: Path, names: tuple[str, ...]) -> None:
    missing: list[str] = []
    empty: list[str] = []
    for name in names:
        path = directory / name
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            missing.append(name)
            continue
        except OSError as error:
            raise ValueError(f"evidence file is not inspectable: {name}") from error
        if stat.S_ISLNK(mode) or not stat.S_ISREG(mode):
            raise ValueError(f"evidence file must be a regular non-symlink file: {name}")
        if path.stat().st_size == 0:
            empty.append(name)
    if missing:
        raise ValueError(f"missing evidence file(s): {', '.join(missing)}")
    if empty:
        raise ValueError(f"empty evidence file(s): {', '.join(empty)}")


def validate_inputs(
    directory: Path,
    architecture: str,
    version: str | None,
    revision: str | None,
) -> tuple[dict[str, Any], dict[str, Any]]:
    if architecture not in {"amd64", "arm64"}:
        raise ValueError("architecture must be amd64 or arm64")
    require_files(directory, REQUIRED_FILES)

    metadata = load_json(directory / "image-metadata.json")
    image_id = metadata.get("Id")
    if not isinstance(image_id, str) or not image_id.startswith("sha256:"):
        raise ValueError("image metadata has no sha256 image Id")
    if metadata.get("Architecture") != architecture or metadata.get("Os") != "linux":
        raise ValueError("image metadata architecture or OS does not match the evidence bundle")
    config = metadata.get("Config")
    if not isinstance(config, dict) or config.get("User") != EXPECTED_RUNTIME_USER:
        raise ValueError("image metadata does not prove the melodee:melodee runtime user")
    labels = config.get("Labels")
    if not isinstance(labels, dict):
        raise ValueError("image metadata has no OCI labels")
    for key, expected in EXPECTED_LABELS.items():
        if labels.get(key) != expected:
            raise ValueError(f"image metadata label {key} is incorrect")
    if version is not None and labels.get("org.opencontainers.image.version") != version:
        raise ValueError("image metadata version label does not match the release")
    if revision is not None and labels.get("org.opencontainers.image.revision") != revision:
        raise ValueError("image metadata revision label does not match the release")

    image_digest = (directory / "image-digest.txt").read_text(encoding="utf-8").strip()
    if image_digest != image_id:
        raise ValueError("image-digest.txt does not match image-metadata.json")

    sbom = load_json(directory / "sbom.spdx.json")
    if not isinstance(sbom.get("spdxVersion"), str) or not sbom["spdxVersion"].startswith("SPDX-"):
        raise ValueError("SBOM is not an SPDX document")
    if not isinstance(sbom.get("packages"), list):
        raise ValueError("SBOM has no packages array")
    load_json(directory / "trivy-critical.json")
    load_json(directory / "trivy-high.json")

    status_lines = {
        line.partition("=")[0]: line.partition("=")[2]
        for line in (directory / "scan-status.txt").read_text(encoding="utf-8").splitlines()
        if "=" in line
    }
    if status_lines.get("architecture") != architecture:
        raise ValueError("scan-status.txt architecture does not match the evidence bundle")
    if status_lines.get("critical_scan") != "present" or status_lines.get("high_scan") != "present":
        raise ValueError("scan-status.txt does not prove both vulnerability reports were captured")

    return metadata, labels


def file_records(directory: Path) -> dict[str, dict[str, int | str]]:
    records: dict[str, dict[str, int | str]] = {}
    for name in (*REQUIRED_FILES, MANIFEST_NAME):
        if name == MANIFEST_NAME:
            continue
        path = directory / name
        records[name] = {"bytes": path.stat().st_size, "sha256": digest(path)}
    return records


def write_manifest(
    directory: Path,
    architecture: str,
    version: str,
    revision: str,
    metadata: dict[str, Any],
    labels: dict[str, Any],
) -> None:
    manifest = {
        "schemaVersion": 1,
        "kind": "melodee-release-image-evidence",
        "architecture": architecture,
        "release": {"version": version, "revision": revision},
        "image": {
            "id": metadata["Id"],
            "architecture": metadata["Architecture"],
            "os": metadata["Os"],
            "runtimeUser": metadata["Config"]["User"],
            "labels": {
                key: labels[key]
                for key in (
                    "org.opencontainers.image.title",
                    "org.opencontainers.image.source",
                    "org.opencontainers.image.licenses",
                    "org.opencontainers.image.version",
                    "org.opencontainers.image.revision",
                )
            },
        },
        "files": file_records(directory),
        "checksumManifest": {
            "path": CHECKSUMS_NAME,
            "covers": [*REQUIRED_FILES, MANIFEST_NAME],
        },
    }
    (directory / MANIFEST_NAME).write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def validate_checksum_manifest(directory: Path) -> None:
    path = directory / CHECKSUMS_NAME
    try:
        require_regular_file(path)
    except ValueError as error:
        raise ValueError("missing or empty SHA256SUMS") from error
    if path.stat().st_size == 0:
        raise ValueError("missing or empty SHA256SUMS")
    expected_names = {*REQUIRED_FILES, MANIFEST_NAME}
    observed: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        checksum, separator, name = line.partition("  ")
        if not separator or len(checksum) != 64 or not name:
            raise ValueError("SHA256SUMS contains a malformed line")
        if name in observed:
            raise ValueError(f"SHA256SUMS contains duplicate entry: {name}")
        observed[name] = checksum
    if set(observed) != expected_names:
        raise ValueError("SHA256SUMS does not cover exactly the evidence files")
    for name, expected in observed.items():
        if digest(directory / name) != expected:
            raise ValueError(f"SHA256SUMS hash mismatch: {name}")


def validate_manifest(
    directory: Path,
    architecture: str,
    version: str | None,
    revision: str | None,
) -> None:
    metadata, labels = validate_inputs(directory, architecture, version, revision)
    manifest = load_json(directory / MANIFEST_NAME)
    if manifest.get("schemaVersion") != 1 or manifest.get("kind") != "melodee-release-image-evidence":
        raise ValueError("unsupported release evidence schema")
    if manifest.get("architecture") != architecture:
        raise ValueError("release evidence architecture does not match the bundle")
    release = manifest.get("release")
    if not isinstance(release, dict):
        raise ValueError("release evidence has no release metadata")
    if version is not None and release.get("version") != version:
        raise ValueError("release evidence version does not match the release")
    if revision is not None and release.get("revision") != revision:
        raise ValueError("release evidence revision does not match the release")
    image = manifest.get("image")
    if not isinstance(image, dict) or image.get("id") != metadata["Id"]:
        raise ValueError("release evidence image ID does not match image metadata")
    if image.get("labels") != {
        key: labels[key]
        for key in (
            "org.opencontainers.image.title",
            "org.opencontainers.image.source",
            "org.opencontainers.image.licenses",
            "org.opencontainers.image.version",
            "org.opencontainers.image.revision",
        )
    }:
        raise ValueError("release evidence OCI labels do not match image metadata")
    records = manifest.get("files")
    if records != file_records(directory):
        raise ValueError("release evidence file records do not match the bundle")
    checksum_manifest = manifest.get("checksumManifest")
    if not isinstance(checksum_manifest, dict) or checksum_manifest.get("path") != CHECKSUMS_NAME:
        raise ValueError("release evidence has no SHA256SUMS declaration")
    if checksum_manifest.get("covers") != [*REQUIRED_FILES, MANIFEST_NAME]:
        raise ValueError("release evidence checksum coverage is incomplete")
    validate_checksum_manifest(directory)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--architecture", required=True)
    parser.add_argument("--version")
    parser.add_argument("--revision")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="write release-evidence.json")
    mode.add_argument("--check", action="store_true", help="validate the complete bundle")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        directory = args.directory
        if contains_symlink_component(directory) or not directory.is_dir():
            raise ValueError("evidence directory does not exist")
        metadata, labels = validate_inputs(directory, args.architecture, args.version, args.revision)
        if args.write:
            if args.version is None or args.revision is None:
                raise ValueError("--write requires --version and --revision")
            write_manifest(
                directory,
                args.architecture,
                args.version,
                args.revision,
                metadata,
                labels,
            )
        else:
            validate_manifest(directory, args.architecture, args.version, args.revision)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"release evidence validation failed: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
