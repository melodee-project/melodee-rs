#!/usr/bin/env python3
"""Validate the committed current-client acceptance artifact.

The matrix is intentionally valid while physical-client work is pending, but
an accepted row must carry enough provenance to reproduce and review the run.
This validator is a repository gate; it does not turn historical evidence into
current acceptance and it never accepts a client row on its own.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MATRIX = ROOT / "compatibility" / "clients" / "r4" / "acceptance-matrix.json"

EXPECTED_ROWS = {
    ("android-phone", "native-v1"),
    ("android-auto-or-dhu", "native-v1"),
    ("meloamp", "native-v1"),
    ("representative-opensubsonic", "opensubsonic"),
    ("representative-jellyfin", "jellyfin"),
}
REQUIRED_WORKFLOWS = {
    ("android-phone", "native-v1"): (
        "login",
        "refresh",
        "browse-search",
        "direct-play",
        "configured-transcode",
        "range-seek",
        "next-previous",
        "background-playback",
        "reconnect",
        "playlists",
        "scrobble",
        "expected-errors",
    ),
    ("android-auto-or-dhu", "native-v1"): (
        "root-browse",
        "search-voice-playback",
        "next-previous",
        "seek",
        "ended-event-advance",
        "reconnect-auth-restore",
    ),
    ("meloamp", "native-v1"): (
        "login-restore",
        "browse-search",
        "direct-play-seek",
        "playlists",
        "reconnect",
        "error-paths",
    ),
    ("representative-opensubsonic", "opensubsonic"): (
        "connect-auth",
        "browse-search",
        "direct-play-range",
        "playlist",
        "favorites",
        "reconnect",
        "expected-errors",
    ),
    ("representative-jellyfin", "jellyfin"): (
        "connect-auth",
        "browse-search",
        "direct-play-transcode-range",
        "favorites-playlists",
        "reconnect",
        "expected-errors",
    ),
}
ALLOWED_STATUSES = {"pending-external", "pass", "fail"}
ALLOWED_WORKFLOW_STATUSES = {"pass", "fail"}
HEX_COMMIT_RE = re.compile(r"^[0-9a-fA-F]{7,64}$")
FINGERPRINT_RE = re.compile(r"^[0-9a-fA-F]{12,128}$")
SECRET_VALUE_RE = re.compile(
    r"(?i)(?:\bbearer\s+[A-Za-z0-9._~-]{12,}\b|"
    r"(?:password|passwd|secret|token|cookie|authorization)\s*[=:]\s*)[^\s,;]+"
    r"|postgres(?:ql)?://[^\s/]+:[^\s@]+@|-----BEGIN [^-]*PRIVATE KEY-----"
)


class ValidationError(ValueError):
    """A user-actionable matrix validation failure."""


def _require_string(value: Any, field: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value.strip()):
        raise ValidationError(f"{field} must be a non-empty string")
    return value


def _validate_relative_evidence_path(value: Any, field: str) -> None:
    path = Path(_require_string(value, field))
    if path.is_absolute() or ".." in path.parts:
        raise ValidationError(f"{field} must be a repository-relative path")
    resolved = (ROOT / path).resolve()
    try:
        resolved.relative_to(ROOT.resolve())
    except ValueError as error:
        raise ValidationError(f"{field} must remain inside the repository") from error
    if not resolved.is_file():
        raise ValidationError(f"{field} must reference an existing repository file")


def _validate_result_row(row: dict[str, Any], index: int) -> None:
    prefix = f"rows[{index}]"
    commit = _require_string(row.get("serverCommit"), f"{prefix}.serverCommit")
    if not HEX_COMMIT_RE.fullmatch(commit):
        raise ValidationError(f"{prefix}.serverCommit must be a hexadecimal commit")
    _require_string(row.get("migratedRunId"), f"{prefix}.migratedRunId")
    _require_string(row.get("clientVersion"), f"{prefix}.clientVersion")
    _require_string(row.get("platformVersion"), f"{prefix}.platformVersion")
    fingerprint = _require_string(
        row.get("configurationFingerprint"), f"{prefix}.configurationFingerprint"
    )
    if not FINGERPRINT_RE.fullmatch(fingerprint):
        raise ValidationError(
            f"{prefix}.configurationFingerprint must be a hexadecimal fingerprint"
        )
    _require_string(row.get("command"), f"{prefix}.command")
    result = _require_string(row.get("result"), f"{prefix}.result")
    expected_result = "pass" if row["status"] == "pass" else "fail"
    if result != expected_result:
        raise ValidationError(
            f"{prefix}.result must be {expected_result!r} for status {row['status']!r}"
        )
    evidence = row.get("evidence")
    if not isinstance(evidence, list) or not evidence:
        raise ValidationError(f"{prefix}.evidence must contain at least one path")
    for evidence_index, path in enumerate(evidence):
        _validate_relative_evidence_path(path, f"{prefix}.evidence[{evidence_index}]")


def _validate_workflow_results(
    row: dict[str, Any], index: int, identity: tuple[str, str]
) -> None:
    prefix = f"rows[{index}]"
    workflow_results = row.get("workflowResults")
    if not isinstance(workflow_results, dict):
        raise ValidationError(f"{prefix}.workflowResults must be an object")
    required = REQUIRED_WORKFLOWS[identity]
    if set(workflow_results) != set(required):
        raise ValidationError(
            f"{prefix}.workflowResults must contain exactly the required workflow IDs"
        )
    statuses: dict[str, str] = {}
    for workflow_id in required:
        result = workflow_results[workflow_id]
        workflow_prefix = f"{prefix}.workflowResults[{workflow_id!r}]"
        if not isinstance(result, dict):
            raise ValidationError(f"{workflow_prefix} must be an object")
        workflow_status = _require_string(result.get("status"), f"{workflow_prefix}.status")
        if workflow_status not in ALLOWED_WORKFLOW_STATUSES:
            raise ValidationError(
                f"{workflow_prefix}.status must be one of {sorted(ALLOWED_WORKFLOW_STATUSES)}"
            )
        if workflow_status == "fail":
            _require_string(result.get("notes"), f"{workflow_prefix}.notes")
        statuses[workflow_id] = workflow_status
    if row["status"] == "pass" and any(status != "pass" for status in statuses.values()):
        raise ValidationError(f"{prefix}.workflowResults must all pass for a passing row")
    if row["status"] == "fail" and all(status == "pass" for status in statuses.values()):
        raise ValidationError(f"{prefix}.workflowResults must record a failed workflow")


def validate_matrix(matrix: Any) -> dict[str, int]:
    if not isinstance(matrix, dict):
        raise ValidationError("matrix root must be an object")
    if matrix.get("artifact") != "melodee.r4.current-client-acceptance":
        raise ValidationError("matrix artifact name is invalid")
    if matrix.get("schemaVersion") != 3:
        raise ValidationError("matrix schemaVersion must be 3")
    workflow_schema = matrix.get("workflowSchema")
    expected_workflow_schema = {
        "statusValues": sorted(ALLOWED_WORKFLOW_STATUSES),
        "requiredByRow": {
            f"{client}/{surface}": list(workflows)
            for (client, surface), workflows in sorted(REQUIRED_WORKFLOWS.items())
        },
    }
    if workflow_schema != expected_workflow_schema:
        raise ValidationError("workflowSchema does not match the required client workflow matrix")
    status = _require_string(matrix.get("status"), "status")
    if status not in ALLOWED_STATUSES:
        raise ValidationError(f"status must be one of {sorted(ALLOWED_STATUSES)}")
    rows = matrix.get("rows")
    if not isinstance(rows, list) or len(rows) != len(EXPECTED_ROWS):
        raise ValidationError(f"rows must contain exactly {len(EXPECTED_ROWS)} entries")

    serialized = json.dumps(matrix, sort_keys=True, ensure_ascii=False)
    if SECRET_VALUE_RE.search(serialized):
        raise ValidationError("matrix contains an apparently unredacted secret value")

    seen: set[tuple[str, str]] = set()
    counts = {row_status: 0 for row_status in ALLOWED_STATUSES}
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            raise ValidationError(f"rows[{index}] must be an object")
        client = _require_string(row.get("client"), f"rows[{index}].client")
        surface = _require_string(row.get("surface"), f"rows[{index}].surface")
        identity = (client, surface)
        if identity not in EXPECTED_ROWS:
            raise ValidationError(f"rows[{index}] is not an expected client surface")
        if identity in seen:
            raise ValidationError(f"duplicate client surface: {client}/{surface}")
        seen.add(identity)
        row_status = _require_string(row.get("status"), f"rows[{index}].status")
        if row_status not in ALLOWED_STATUSES:
            raise ValidationError(f"rows[{index}].status is not supported")
        counts[row_status] += 1
        if row_status != "pending-external":
            _validate_result_row(row, index)
            _validate_workflow_results(row, index, identity)

    if seen != EXPECTED_ROWS:
        raise ValidationError(f"missing expected client surfaces: {sorted(EXPECTED_ROWS - seen)}")
    expected_status = "pass" if counts["pass"] == len(EXPECTED_ROWS) else "pending-external"
    if status != expected_status:
        raise ValidationError(
            f"matrix status must be {expected_status!r} while {counts['pass']} of "
            f"{len(EXPECTED_ROWS)} rows pass"
        )
    if counts["fail"] and status == "pass":
        raise ValidationError("matrix cannot be pass while a client row fails")
    return counts


def load_and_validate(path: Path) -> dict[str, int]:
    try:
        matrix = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValidationError(f"cannot read {path}: {error}") from error
    return validate_matrix(matrix)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--matrix", type=Path, default=DEFAULT_MATRIX)
    args = parser.parse_args()
    try:
        counts = load_and_validate(args.matrix)
    except ValidationError as error:
        print(f"Client acceptance validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    digest = hashlib.sha256(args.matrix.read_bytes()).hexdigest()[:16]
    print(
        "Client acceptance artifact passed: "
        f"{counts['pass']} pass, {counts['fail']} fail, "
        f"{counts['pending-external']} pending-external; sha256={digest}"
    )


if __name__ == "__main__":
    main()
