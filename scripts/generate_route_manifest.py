#!/usr/bin/env python3
"""Generate route manifests for the legacy C# server's public API surfaces.

Statically parses ASP.NET Core attribute-routed controllers under
``src/Melodee.Blazor/Controllers`` and emits one stable JSON manifest per
compatibility surface (melodee-api-v1, opensubsonic, jellyfin). Manifests are
sorted and contain no timestamps so they are reviewable, diffable contract
artifacts tied to a recorded legacy baseline commit.

This is Phase 0 capture tooling (FIRST-10-ISSUES.md Issue 3). It never sends
traffic and never writes outside the given output directory.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTROLLERS_REL = Path("src/Melodee.Blazor/Controllers")

SCHEMA_VERSION = 1

SURFACES = {
    "Melodee": "melodee-api-v1",
    "OpenSubsonic": "opensubsonic",
    "Jellyfin": "jellyfin",
}
# Unversioned controllers living directly under Controllers/ belong to the
# native surface.
ROOT_FILES = {"ThemesController.cs": "melodee-api-v1"}

HTTP_VERBS = {"Get": "GET", "Post": "POST", "Put": "PUT", "Delete": "DELETE", "Patch": "PATCH"}

# Matches controller classes including outliers that lack the Controller
# suffix (OpenSubsonic MediaLibraryScanning — Issue 6 discovery). Base-class
# files produce no routes and are naturally excluded.
CLASS_RE = re.compile(r"\bclass\s+(?P<name>\w+)\b")
# Excludes type declarations such as `public sealed class Foo(` — the tempered
# lookahead rejects any token sequence ending in a type keyword.
METHOD_RE = re.compile(
    r"public\s+(?:async\s+)?(?!(?:\w+\s+)*?(?:class|record|interface|struct)\b)"
    r"[\w<>\[\],.? ]+?\s+(?P<name>\w+)\s*\("
)
# Attribute bodies may contain one level of nested brackets, e.g.
# [Route("api/jf/[controller]")].
ATTR_RE = re.compile(r"\[(?P<body>(?:[^\[\]]|\[[^\]]*\])+)\]")
HTTP_ATTR_RE = re.compile(r"Http(?P<verb>Get|Post|Put|Delete|Patch)(?:\(\"(?P<template>[^\"]*)\"\))?")
ROUTE_ATTR_RE = re.compile(r"Route\(\"(?P<template>[^\"]*)\"\)")
AUTHORIZE_ATTR_RE = re.compile(r"Authorize(?:\((?P<args>[^)]*)\))?")
API_VERSION_ATTR_RE = re.compile(r"ApiVersion\((?P<version>\d+)\)")
ROLES_RE = re.compile(r"Roles\s*=\s*\"(?P<roles>[^\"]+)\"")
SCHEMES_RE = re.compile(r"AuthenticationSchemes\s*=\s*(?P<schemes>[^,)]+)")


def parse_attributes(block: str) -> dict[str, object]:
    """Parse the attribute lines immediately preceding a class or method."""
    verbs: list[dict[str, str | None]] = []
    routes: list[str] = []
    authorize: list[str] = []
    allow_anonymous = False
    api_version: str | None = None
    for match in ATTR_RE.finditer(block):
        body = match.group("body")
        http = HTTP_ATTR_RE.fullmatch(body)
        if http:
            verbs.append({"verb": HTTP_VERBS[http.group("verb")], "template": http.group("template")})
            continue
        route = ROUTE_ATTR_RE.fullmatch(body)
        if route:
            routes.append(route.group("template"))
            continue
        auth = AUTHORIZE_ATTR_RE.match(body)
        if auth:
            authorize.append(auth.group("args") or "")
            continue
        if body.startswith("AllowAnonymous"):
            allow_anonymous = True
            continue
        version = API_VERSION_ATTR_RE.fullmatch(body)
        if version:
            api_version = version.group("version")
    return {
        "verbs": verbs,
        "routes": routes,
        "authorize": authorize,
        "allowAnonymous": allow_anonymous,
        "apiVersion": api_version,
    }


def split_member_bodies(source: str, method_spans: list[tuple[int, str]]) -> dict[str, str]:
    """Approximate method bodies: from each method declaration to the next."""
    bodies: dict[str, str] = {}
    for index, (start, name) in enumerate(method_spans):
        end = method_spans[index + 1][0] if index + 1 < len(method_spans) else len(source)
        bodies[name] = source[start:end]
    return bodies


def controller_token(controller: str) -> str:
    """ASP.NET's [controller] token: class name minus the Controller suffix."""
    return controller[: -len("Controller")] if controller.endswith("Controller") else controller


def resolve_routes(
    class_prefix: str | None,
    controller: str,
    api_version: str | None,
    verb_templates: list[str | None],
    route_templates: list[str],
) -> list[str]:
    prefix = class_prefix or ""
    prefix = prefix.replace("[controller]", controller_token(controller))
    version = api_version or "1"
    prefix = prefix.replace("{version:apiVersion}", version)

    templates: list[str | None] = route_templates or verb_templates or [None]
    resolved: list[str] = []
    for template in templates:
        if template is not None:
            template = template.replace("[controller]", controller_token(controller))
        if template is None:
            path = prefix
        elif template.startswith("/"):
            path = template
        elif prefix:
            path = f"{prefix}/{template}"
        else:
            path = template
        path = path.replace("{version:apiVersion}", version)
        path = re.sub(r"/+", "/", "/" + path.lstrip("/"))
        resolved.append(path)
    return sorted(set(resolved))


def auth_contract(class_attrs: dict[str, object], method_attrs: dict[str, object]) -> dict[str, object]:
    if method_attrs["allowAnonymous"]:
        return {"required": False}
    authorize = list(method_attrs["authorize"]) or list(class_attrs["authorize"])
    if not authorize:
        return {"required": False, "note": "no ASP.NET authorization attribute; handler may authenticate internally"}
    result: dict[str, object] = {"required": True}
    for args in authorize:
        roles = ROLES_RE.search(args)
        if roles:
            result["roles"] = sorted(roles.group("roles").split(","))
        schemes = SCHEMES_RE.search(args)
        if schemes:
            result["schemes"] = [schemes.group("schemes").strip()]
    return result


def intentional_statuses(body: str) -> list[int]:
    statuses: set[int] = set()
    if "HttpStatusCode.Gone" in body:
        statuses.add(410)
    if "HttpStatusCode.NotImplemented" in body:
        statuses.add(501)
    return sorted(statuses)


def implementation_status(body: str, statuses: list[int]) -> str:
    """Classify the action. A terminal Gone/NotImplemented action has no other
    return statements than the marker return."""
    if not statuses:
        return "implemented"
    returns = [line.strip() for line in body.splitlines() if line.strip().startswith("return ")]
    terminal_only = returns and all("StatusCode" in line for line in returns)
    if terminal_only and len(statuses) == 1:
        return {410: "gone", 501: "not-implemented"}[statuses[0]]
    return "implemented-conditional"


def parse_controller(path: Path) -> list[dict[str, object]]:
    source = path.read_text(encoding="utf-8")
    class_match = CLASS_RE.search(source)
    if not class_match:
        return []
    controller = class_match.group("name")
    class_block = source[max(0, class_match.start() - 2000) : class_match.start()]
    class_attrs = parse_attributes(class_block)
    class_prefix = class_attrs["routes"][-1] if class_attrs["routes"] else None
    api_version = str(class_attrs["apiVersion"]) if class_attrs["apiVersion"] else None

    method_spans = [(m.start(), m.group("name")) for m in METHOD_RE.finditer(source)]
    bodies = split_member_bodies(source, method_spans)

    actions: list[dict[str, object]] = []
    for start, name in method_spans:
        attr_block = source[max(0, start - 2000) : start]
        # Take the contiguous trailing region of attribute and doc-comment
        # lines. Scanning line-wise avoids mistaking `{id:guid}` route-template
        # braces for block boundaries.
        region: list[str] = []
        for line in reversed(attr_block.splitlines()):
            stripped = line.strip()
            if stripped.startswith("[") or stripped.startswith("///") or not stripped:
                region.append(line)
            else:
                break
        trailing = "\n".join(reversed(region))
        method_attrs = parse_attributes(trailing)
        if not method_attrs["verbs"] and not method_attrs["routes"]:
            continue
        verbs = sorted({v["verb"] for v in method_attrs["verbs"]})
        if not verbs:
            verbs = ["GET"]
        verb_templates = [v["template"] for v in method_attrs["verbs"]]
        routes = resolve_routes(
            class_prefix,
            controller,
            api_version,
            verb_templates,
            list(method_attrs["routes"]),
        )
        body = bodies.get(name, "")
        statuses = intentional_statuses(body)
        for route in routes:
            actions.append(
                {
                    "path": route,
                    "methods": verbs,
                    "controller": controller,
                    "action": name,
                    "auth": auth_contract(class_attrs, method_attrs),
                    "apiVersion": api_version,
                    "intentionalStatuses": statuses,
                    "implementation": implementation_status(body, statuses),
                    "source": path.name,
                }
            )
    return actions


CSPROJ_REL = Path("src/Melodee.Blazor/Melodee.Blazor.csproj")
COMPILE_REMOVE_RE = re.compile(r'<Compile\s+Remove="(?P<path>[^"]+)"')


def build_excluded_sources(legacy_root: Path) -> set[str]:
    """Source files excluded from compilation by ``<Compile Remove>`` entries.

    Discovery (Issue 5): the frozen baseline excludes QueueController and all
    Party controllers this way — their routes exist in source but answer 404
    at runtime because the handlers are never compiled into the server."""
    csproj = legacy_root / CSPROJ_REL
    if not csproj.exists():
        return set()
    excluded: set[str] = set()
    for match in COMPILE_REMOVE_RE.finditer(csproj.read_text(encoding="utf-8")):
        normalized = match.group("path").replace("\\", "/")
        if normalized.endswith("**"):
            continue
        excluded.add(normalized.rsplit("/", 1)[-1])
    return excluded


def legacy_commit(legacy_root: Path) -> str:
    result = subprocess.run(
        ["git", "-C", str(legacy_root), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def build_manifests(legacy_root: Path) -> dict[str, dict[str, object]]:
    controllers = legacy_root / CONTROLLERS_REL
    excluded = build_excluded_sources(legacy_root)
    manifests: dict[str, list[dict[str, object]]] = {surface: [] for surface in SURFACES.values()}
    for directory, surface in SURFACES.items():
        for path in sorted((controllers / directory).glob("*.cs")):
            for route in parse_controller(path):
                if path.name in excluded:
                    route["implementation"] = "excluded-from-build"
                    route["buildExcluded"] = True
                manifests[surface].append(route)
    for filename, surface in ROOT_FILES.items():
        path = controllers / filename
        if path.exists():
            manifests[surface].extend(parse_controller(path))

    commit = legacy_commit(legacy_root)
    return {
        surface: {
            "schemaVersion": SCHEMA_VERSION,
            "surface": surface,
            "legacyBaselineCommit": commit,
            "legacyControllersRoot": str(CONTROLLERS_REL),
            "routeCount": len(routes),
            "routes": sorted(routes, key=lambda r: (r["path"], r["controller"], r["action"])),
        }
        for surface, routes in manifests.items()
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--legacy-root",
        type=Path,
        default=Path("../melodee").resolve(),
        help="Checkout of the legacy C# repository (read-only).",
    )
    parser.add_argument("--output-root", type=Path, default=ROOT / "compatibility")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Fail if committed manifests differ from regeneration.",
    )
    args = parser.parse_args()

    if not (args.legacy_root / CONTROLLERS_REL).is_dir():
        print(f"error: {args.legacy_root} does not contain {CONTROLLERS_REL}", file=sys.stderr)
        raise SystemExit(2)

    manifests = build_manifests(args.legacy_root)
    drift = False
    for surface, manifest in manifests.items():
        output = args.output_root / surface / "contracts" / "route-manifest.json"
        text = json.dumps(manifest, indent=2, ensure_ascii=False) + "\n"
        if args.check:
            if not output.exists() or output.read_text(encoding="utf-8") != text:
                print(f"DRIFT: {output.relative_to(ROOT)}", file=sys.stderr)
                drift = True
            continue
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(text, encoding="utf-8")
        print(f"{surface}: {manifest['routeCount']} routes -> {output.relative_to(ROOT)}")
    if drift:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
