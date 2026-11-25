#!/usr/bin/env python3
"""Find possible matches for listener data in an independently rebuilt library.

Run without --review to preview. Review saves approved public-ID pairs in a
private local file for melodee-cli users import-state. All database access is
read-only. Run again after ingesting more releases.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import unicodedata
from collections import Counter, defaultdict
from contextlib import contextmanager
from pathlib import Path
from urllib.parse import parse_qsl, quote, unquote, urlencode, urlsplit
from uuid import UUID

from export_legacy_user_state import ExportError, psql_binary


# One database snapshot supplies identity and live catalog metadata. The query
# never opens a media asset or reads native account secrets.
TARGET_SQL = """
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT json_build_object('installation',installation_id)
FROM server_installation_identity WHERE singleton;
SELECT json_build_object('kind','user','publicId',public_api_id,
                         'username',username,'email',email,'normalizedName',normalized_username,
                         'normalizedEmail',normalized_email) FROM native_users;
SELECT json_build_object('kind','artist','publicId',public_api_id,
                         'artist',canonical_name,'detail',disambiguation)
FROM artists WHERE status IN ('active','verified');
SELECT json_build_object('kind','release','publicId',public_api_id,
                         'artist',display_artist,'release',title,'releaseDate',release_date)
FROM releases WHERE status='active';
SELECT json_build_object('kind','track','publicId',t.public_api_id,'artist',r.display_artist,
                         'release',r.title,'releaseDate',r.release_date,'title',t.title,
                         'trackNumber',t.track_number,'disc',t.disc_number,'durationMs',t.duration_ms)
FROM tracks t JOIN releases r ON r.id=t.release_id WHERE r.status='active';
"""


def checked_rows(directory, manifest, name):
    digest = hashlib.sha256()
    checked = []
    with (directory / name).open("rb") as stream:
        for raw in stream:
            digest.update(raw)
            if raw.strip():
                checked.append(json.loads(raw))
    expected_checksum = manifest["checksums"].get(name)
    expected_count = manifest["counts"].get(name)
    if digest.hexdigest() != expected_checksum or len(checked) != expected_count:
        raise ValueError(f"{name} does not match the export manifest")
    return checked


def sources_from_export(directory):
    manifest = json.loads((directory / "manifest.json").read_bytes())
    if manifest.get("schemaVersion") != 1:
        raise ValueError("unsupported export version")
    # Resolve account conflicts first. Password hashes and profile data have no role in review.
    source = [
        {k: user[k] for k in ("publicId", "username", "email")} | {"kind": "user"}
        for user in checked_rows(directory, manifest, "users.ndjson")
    ]
    evidence = "catalog-evidence.ndjson"
    if evidence in manifest["checksums"] or evidence in manifest["counts"]:
        source.extend(checked_rows(directory, manifest, evidence))
    else:
        print(
            "Reviewing accounts only. For music suggestions, use an export containing catalog-evidence.ndjson.",
            file=sys.stderr,
        )
    return source


def normalized(value):
    return " ".join(unicodedata.normalize("NFKC", value or "").casefold().split())


def match_key(row):
    fields = {
        "user": ("username", "email"),
        "artist": ("artist",),
        "release": ("artist", "release"),
        "track": ("artist", "release", "title"),
    }[row["kind"]]
    values = tuple(normalized(row.get(field)) for field in fields)
    return (row["kind"], *values) if all(values) else None


def same_position_and_duration(source, target):
    if source["kind"] != "track":
        return True
    return (
        source.get("trackNumber") is not None
        and source["trackNumber"] == target.get("trackNumber")
        and (source.get("durationMs") or 0) > 0
        and (target.get("durationMs") or 0) > 0
        and abs(source["durationMs"] - target["durationMs"]) <= 2000
    )


def connection(url):
    parts = urlsplit(url)
    if parts.scheme not in ("postgres", "postgresql"):
        raise ValueError("MELODEE__DATABASE__URL must be a PostgreSQL URL")
    env = {**os.environ, "PGCONNECT_TIMEOUT": "10"}
    netloc = parts.netloc
    if parts.password is not None:
        env["PGPASSWORD"] = unquote(parts.password)
        netloc = netloc.partition(":")[0] + "@" + netloc.rpartition("@")[2]
    query = []
    for key, value in parse_qsl(parts.query, keep_blank_values=True):
        if key == "password":
            env["PGPASSWORD"] = value
        else:
            query.append((key, value))
    return parts._replace(
        netloc=netloc, query=urlencode(query, quote_via=quote)
    ).geturl(), env


def snapshot_query(sources):
    # Apply exactly the same database normalization and uniqueness rules as the importer.
    # Hex encodes JSON as data in a constant, outside psql's command-line arguments.
    evidence = [
        {key: row.get(key) for key in ("publicId", "artist", "release", "title")}
        for row in sources
        if row["kind"] == "track"
    ]
    if not evidence:
        return TARGET_SQL + "COMMIT;"
    path = (
        Path(__file__).resolve().parents[1]
        / "apps/melodee-cli/src/user_state_import/track_metadata.sql"
    )
    payload = (
        json.dumps(evidence, ensure_ascii=False, separators=(",", ":")).encode().hex()
    )
    sql = path.read_text().replace(
        "$1::jsonb", f"convert_from(decode('{payload}', 'hex'), 'UTF8')::jsonb"
    )
    return (
        TARGET_SQL
        + f"""
SELECT json_build_object('exactSource', source, 'exactTarget', target)
FROM ({sql}) matches WHERE source_count=1 AND target_count=1;
COMMIT;
"""
    )


def target_snapshot(sources, url):
    keys = {match_key(row) for row in sources if row["kind"] != "user"} - {None}
    account_emails = {
        row["email"].strip().upper() for row in sources if row["kind"] == "user"
    }
    account_names = {
        row["username"].strip().upper() for row in sources if row["kind"] == "user"
    }
    original = {(row["kind"], row["publicId"]) for row in sources}
    candidates = defaultdict(list)
    direct = set()
    installation = None
    database, env = connection(url)
    exact_tracks = {}
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as queries:
        queries.write(snapshot_query(sources))
        queries.seek(0)
        with subprocess.Popen(
            [
                psql_binary(),
                "-XqAt",
                "-w",
                "-d",
                database,
                "-v",
                "ON_ERROR_STOP=1",
                "-f",
                "-",
            ],
            env=env,
            stdin=queries,
            stdout=subprocess.PIPE,
            text=True,
        ) as process:
            for line in process.stdout:
                row = json.loads(line)
                if "installation" in row:
                    installation = str(UUID(row["installation"]))
                    continue
                if "exactSource" in row:
                    exact_tracks[row["exactSource"]] = row["exactTarget"]
                    continue
                identity = row["kind"], row["publicId"]
                if identity in original:
                    direct.add(identity)
                if row["kind"] == "user":
                    if row["normalizedEmail"] in account_emails:
                        candidates[("email", row["normalizedEmail"])].append(row)
                    if row["normalizedName"] in account_names:
                        candidates[("username", row["normalizedName"])].append(row)
                    continue
                key = match_key(row)
                if key in keys:
                    candidates[key].append(row)
            if process.wait() or installation is None:
                raise ValueError("could not read the target Melodee catalog")
    return installation, direct, candidates, exact_tracks


def read_map(path, installation):
    if not path.exists():
        return {
            "schemaVersion": 1,
            "targetInstallationId": installation,
            "approved": [],
        }
    data = json.loads(path.read_bytes())
    if (
        data.get("schemaVersion") != 1
        or data.get("targetInstallationId") != installation
    ):
        raise ValueError("identity map version or target installation does not match")
    sources, targets = set(), set()
    for link in data["approved"]:
        kind = link["kind"]
        source, target = str(UUID(link["legacyId"])), str(UUID(link["targetId"]))
        if (
            kind not in ("user", "artist", "release", "track")
            or source == target
            or UUID(source).int == 0
            or UUID(target).int == 0
            or (kind, source) in sources
            or (kind, target) in targets
        ):
            raise ValueError("invalid or conflicting links in the identity map")
        link.update(legacyId=source, targetId=target)
        sources.add((kind, source))
        targets.add((kind, target))
    return data


@contextmanager
def exclusive_review(path):
    lock = path.with_name(path.name + ".lock")
    try:
        handle = lock.open("x")
    except FileExistsError as error:
        raise ValueError(
            f"another review holds {lock}; if it was interrupted, close it before removing the lock"
        ) from error
    try:
        with handle:
            yield
    finally:
        lock.unlink()


def save_map(path, data):
    out = tempfile.NamedTemporaryFile(
        dir=path.parent, delete=False, mode="w", encoding="utf-8"
    )
    temp = Path(out.name)
    try:
        with out:
            json.dump(data, out, indent=2)
            out.write("\n")
            out.flush()
            os.fsync(out.fileno())
        os.replace(temp, path)
    finally:
        temp.unlink(missing_ok=True)


def display(row):
    fields = (
        "username",
        "email",
        "artist",
        "release",
        "releaseDate",
        "title",
        "disc",
        "trackNumber",
        "durationMs",
        "detail",
    )
    return " ".join(
        f"{k}={json.dumps(row[k], ensure_ascii=False)}" for k in fields if row.get(k)
    )


def usable_account_email(email):
    local, separator, domain = email.partition("@")
    prefix = "MIGRATED-USER-"
    placeholder = (
        domain == "IMPORT.LOCAL"
        and local.startswith(prefix)
        and local[len(prefix) :].isascii()
        and local[len(prefix) :].isdigit()
    )
    return bool(
        local
        and separator
        and domain
        and "@" not in domain
        and not placeholder
        and not any(c.isspace() for c in email)
    )


def reconcile(
    sources,
    direct,
    candidates,
    data,
    path,
    review=False,
    show_pending=False,
    saved_links=(),
    exact_tracks=None,
):
    linked = {(r["kind"], r["legacyId"]) for r in data["approved"]}
    assigned = {(r["kind"], r["targetId"]) for r in data["approved"]}
    for link in saved_links:
        linked.add((link["kind"], link["legacyId"]))
        assigned.add((link["kind"], link["targetId"]))
    exact_tracks = exact_tracks or {}
    originals = {(r["kind"], r["publicId"]) for r in sources}
    counts = Counter()
    emails = Counter(
        row["email"].strip().upper() for row in sources if row["kind"] == "user"
    )
    for row in sources:
        kind, source = row["kind"], row["publicId"]
        if (kind, source) in direct:
            counts["same public ID"] += 1
            continue
        if (kind, source) in linked:
            counts["previously linked"] += 1
            continue
        if kind == "track" and source in exact_tracks:
            target = exact_tracks[source]
            if (kind, target) not in originals and (kind, target) not in assigned:
                counts[
                    "tracks automatically matched by artist, release, and title"
                ] += 1
                continue
        if kind == "user":
            email = row["email"].strip().upper()
            same_email = candidates.get(("email", email), ())
            same_name = candidates.get(
                ("username", row["username"].strip().upper()), ()
            )
            if (
                len(same_email) == 1
                and emails[email] == 1
                and usable_account_email(email)
            ):
                target = same_email[0]["publicId"]
                if (
                    all(c["publicId"] == target for c in same_name)
                    and (kind, target) not in originals
                    and (kind, target) not in assigned
                ):
                    counts["accounts automatically matched by email"] += 1
                    continue
            matches = sorted(
                {c["publicId"]: c for c in (*same_email, *same_name)}.values(),
                key=lambda c: c["publicId"],
            )
        else:
            matches = sorted(
                candidates.get(match_key(row), ()),
                key=lambda candidate: (
                    not same_position_and_duration(row, candidate),
                    candidate["publicId"],
                ),
            )
        if not matches:
            counts["without a suggestion"] += 1
            if show_pending:
                print(f"Pending {kind} {source} {display(row)}")
            continue
        print(f"Legacy {kind} {source} {display(row)}")
        for number, candidate in enumerate(matches, 1):
            print(f"  {number}. {candidate['publicId']} {display(candidate)}")
        if review:
            choice = input(
                "Approve which number? Enter skips, q saves progress and exits: "
            ).strip()
            if choice.lower() == "q":
                break
            if choice:
                if not choice.isdecimal() or not 1 <= int(choice) <= len(matches):
                    raise ValueError("invalid choice; previous approvals are saved")
                target = matches[int(choice) - 1]["publicId"]
                if (kind, target) in originals or (kind, target) in assigned:
                    raise ValueError(
                        "this target belongs to another source identity; previous approvals are saved"
                    )
                data["approved"].append(
                    {"kind": kind, "legacyId": source, "targetId": target}
                )
                save_map(path, data)
                assigned.add((kind, target))
                linked.add((kind, source))
                counts["newly approved"] += 1
                continue
        counts["awaiting review" if len(matches) == 1 else "multiple candidates"] += 1
    print("Summary (users and distinct music items)")
    for label, count in counts.items():
        print(f"  {label}: {count}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input", required=True, type=Path, help="portable export directory"
    )
    parser.add_argument(
        "--identity-map", required=True, type=Path, help="reusable local approval file"
    )
    parser.add_argument(
        "--review",
        action="store_true",
        help="review candidates and save each approval locally",
    )
    parser.add_argument(
        "--show-pending",
        action="store_true",
        help="also list items without candidate metadata matches",
    )
    args = parser.parse_args()
    try:
        url = os.environ.get("MELODEE__DATABASE__URL")
        if not url:
            raise ValueError("set MELODEE__DATABASE__URL to the Rust database")
        sources = sources_from_export(args.input)
        installation, direct, candidates, exact_tracks = target_snapshot(sources, url)
        path = args.identity_map
        receipts = [
            args.input / f"{prefix}-links-{installation}.json"
            for prefix in ("account", "track")
        ]
        saved = [
            link
            for receipt in receipts
            for link in read_map(receipt, installation)["approved"]
        ]
        if path.resolve() in {receipt.resolve() for receipt in receipts}:
            raise ValueError(
                "choose a separate file for reviewed music and account links"
            )
        if args.review:
            with exclusive_review(path):
                data = read_map(path, installation)
                if not path.exists():
                    save_map(path, data)
                reconcile(
                    sources,
                    direct,
                    candidates,
                    data,
                    path,
                    True,
                    args.show_pending,
                    saved,
                    exact_tracks,
                )
        else:
            reconcile(
                sources,
                direct,
                candidates,
                read_map(path, installation),
                path,
                show_pending=args.show_pending,
                saved_links=saved,
                exact_tracks=exact_tracks,
            )
    except (OSError, ValueError, KeyError, ExportError, EOFError) as error:
        parser.exit(1, f"Reconciliation failed: {error}\n")


if __name__ == "__main__":
    main()
