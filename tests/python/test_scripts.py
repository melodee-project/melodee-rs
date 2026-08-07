from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"unable to load {path}")
    module = importlib.util.module_from_spec(spec)
    # Register before execution so decorators like @dataclass can resolve the
    # module in sys.modules.
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class RequirementExtractionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("extract_requirements", ROOT / "scripts" / "extract_requirements.py")

    def test_prd_has_large_unique_requirement_inventory(self) -> None:
        requirements = self.module.extract()
        ids = [item["id"] for item in requirements]
        self.assertGreaterEqual(len(ids), 200)
        self.assertEqual(len(ids), len(set(ids)))
        self.assertNotIn("unmapped", {item["phase"] for item in requirements})


class CaptureSafetyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("capture_baseline", ROOT / "scripts" / "capture_baseline.py")

    def test_sensitive_query_values_are_redacted(self) -> None:
        source = "https://example.test/rest/ping.view?u=alice&p=secret&t=hash&s=salt&c=test"
        redacted = self.module.redact_url(source)
        self.assertIn("u=alice", redacted)
        self.assertNotIn("secret", redacted)
        self.assertNotIn("hash", redacted)
        self.assertNotIn("salt", redacted)

    def test_public_scenarios_are_valid_json_array(self) -> None:
        value = json.loads((ROOT / "compatibility" / "scenarios" / "public.json").read_text())
        self.assertIsInstance(value, list)
        self.assertGreater(len(value), 0)
        self.assertTrue(all("name" in item and "path" in item for item in value))


SAMPLE_CONTROLLER = """
using Microsoft.AspNetCore.Mvc;

namespace Melodee.Blazor.Controllers.OpenSubsonic;

[ApiController]
[Authorize(AuthenticationSchemes = JwtBearerDefaults.AuthenticationScheme)]
[ApiVersion(1)]
[Route("api/v{version:apiVersion}/widgets")]
public sealed class WidgetsController : ControllerBase
{
    /// <summary>Get a widget.</summary>
    [HttpGet]
    [Route("{id:guid}")]
    public async Task<IActionResult> GetWidget(Guid id)
    {
        return Ok();
    }

    [HttpGet]
    [HttpPost]
    [Route("/rest/getWidgets.view")]
    [Route("/rest/getWidgets")]
    public Task<IActionResult> GetWidgets()
    {
        return MakeResult();
    }

    [HttpGet]
    [Route("/rest/getChatMessages")]
    public IActionResult DeprecatedWontImplement()
    {
        HttpContext.Response.Headers.Append("Cache-Control", "no-cache");
        return StatusCode((int)HttpStatusCode.Gone);
    }

    [Authorize(Roles = "Admin")]
    [HttpDelete("{id:guid}")]
    public IActionResult Delete(Guid id)
    {
        return NoContent();
    }
}
"""


class RouteManifestTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("generate_route_manifest", ROOT / "scripts" / "generate_route_manifest.py")

    def parse_sample(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "WidgetsController.cs"
            path.write_text(SAMPLE_CONTROLLER, encoding="utf-8")
            return self.module.parse_controller(path)

    def test_version_and_prefix_are_substituted(self) -> None:
        paths = {route["path"] for route in self.parse_sample()}
        self.assertIn("/api/v1/widgets/{id:guid}", paths)
        self.assertNotIn("{version:apiVersion}", " ".join(paths))

    def test_absolute_routes_and_multiple_verbs(self) -> None:
        routes = {route["path"]: route for route in self.parse_sample()}
        self.assertEqual(routes["/rest/getWidgets.view"]["methods"], ["GET", "POST"])

    def test_terminal_gone_action_is_classified(self) -> None:
        routes = {route["path"]: route for route in self.parse_sample()}
        gone = routes["/rest/getChatMessages"]
        self.assertEqual(gone["implementation"], "gone")
        self.assertEqual(gone["intentionalStatuses"], [410])

    def test_roles_are_recorded(self) -> None:
        routes = {route["path"]: route for route in self.parse_sample()}
        delete = next(route for route in routes.values() if route["action"] == "Delete")
        self.assertEqual(delete["auth"].get("roles"), ["Admin"])

    def test_primary_constructor_is_not_an_action(self) -> None:
        actions = {route["action"] for route in self.parse_sample()}
        self.assertNotIn("WidgetsController", actions)

    def test_committed_manifests_are_well_formed(self) -> None:
        for surface in ("melodee-api-v1", "opensubsonic", "jellyfin"):
            manifest = json.loads(
                (ROOT / "compatibility" / surface / "contracts" / "route-manifest.json").read_text()
            )
            self.assertEqual(manifest["surface"], surface)
            self.assertRegex(manifest["legacyBaselineCommit"], r"^[0-9a-f]{40}$")
            self.assertEqual(manifest["routeCount"], len(manifest["routes"]))
            self.assertGreater(manifest["routeCount"], 0)
            for route in manifest["routes"]:
                self.assertNotIn("[controller]", route["path"])
                self.assertNotIn("{version:apiVersion}", route["path"])
                self.assertGreater(len(route["methods"]), 0)

    def test_openapi_contract_is_sanitized(self) -> None:
        text = (ROOT / "compatibility" / "melodee-api-v1" / "contracts" / "openapi-v1.json").read_text()
        document = json.loads(text)
        self.assertGreater(len(document["paths"]), 100)
        self.assertNotIn("127.0.0.1", text)
        self.assertEqual(document["servers"], [{"url": "https://{melodee-host}/"}])


class CompareCapturesTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("compare_captures", ROOT / "scripts" / "compare_captures.py")

    def write_capture(self, directory: Path, body: dict, *, status: int = 200, normalize: dict | None = None) -> None:
        directory.mkdir(parents=True, exist_ok=True)
        payload = (json.dumps(body, indent=2) + "\n").encode()
        (directory / "scenario.json").write_bytes(payload)
        metadata = {
            "name": "scenario",
            "request": {"method": "GET", "url": "http://example.test/x", "auth": "none"},
            "response": {
                "status": status,
                "headers": {"content-type": "application/json"},
                "bodyFile": "scenario.json",
                "sha256": "unused",
                "bytes": len(payload),
            },
            "normalize": normalize or {},
            "error": None,
            "allowFailure": False,
        }
        (directory / "scenario.metadata.json").write_text(json.dumps(metadata))

    def compare_dirs(self, left_body: dict, right_body: dict, normalize: dict | None = None):
        with tempfile.TemporaryDirectory() as tmp:
            left = Path(tmp) / "left"
            right = Path(tmp) / "right"
            self.write_capture(left, left_body, normalize=normalize)
            self.write_capture(right, right_body, normalize=normalize)
            return self.module.compare(left, right)

    def test_parse_json_path(self) -> None:
        self.assertEqual(self.module.parse_json_path("$.a.b"), ["a", "b"])
        self.assertEqual(self.module.parse_json_path("$.a[*].id"), ["a", "*", "id"])
        self.assertEqual(self.module.parse_json_path("$.a[0].b"), ["a", 0, "b"])
        with self.assertRaises(ValueError):
            self.module.parse_json_path("a.b")

    def test_mask_nested_and_wildcard_paths(self) -> None:
        document = {"token": "abc", "items": [{"id": 1}, {"id": 2}], "keep": 5}
        masked = self.module.normalize_json(document, ["$.token", "$.items[*].id"])
        self.assertEqual(masked["token"], "<volatile>")
        self.assertEqual([item["id"] for item in masked["items"]], ["<volatile>", "<volatile>"])
        self.assertEqual(masked["keep"], 5)

    def test_missing_mask_path_is_tolerated(self) -> None:
        masked = self.module.normalize_json({"a": 1}, ["$.nonexistent.deep"])
        self.assertEqual(masked, {"a": 1})

    def test_identical_captures_match(self) -> None:
        body = {"a": 1, "b": [1, 2], "c": None}
        self.assertEqual(self.compare_dirs(body, body), [])

    def test_volatile_fields_are_masked(self) -> None:
        left = {"token": "aaa", "expiresAt": "t1", "stable": True}
        right = {"token": "bbb", "expiresAt": "t2", "stable": True}
        normalize = {"maskJsonPaths": ["$.token", "$.expiresAt"]}
        self.assertEqual(self.compare_dirs(left, right, normalize), [])

    def test_deliberate_contract_difference_fails_usefully(self) -> None:
        left = {"status": "ok", "count": 3}
        right = {"status": "ok", "count": 4}
        differences = self.compare_dirs(left, right)
        self.assertEqual(len(differences), 1)
        self.assertIn("$.count", differences[0].field)

    def test_null_vs_omitted_is_significant(self) -> None:
        differences = self.compare_dirs({"value": None}, {})
        self.assertEqual(len(differences), 1)
        self.assertEqual(differences[0].expected, "null")
        self.assertEqual(differences[0].actual, '"<omitted>"')

    def test_array_order_is_significant(self) -> None:
        differences = self.compare_dirs({"items": [1, 2]}, {"items": [2, 1]})
        self.assertTrue(any("$" in item.field for item in differences))

    def test_type_change_is_significant(self) -> None:
        differences = self.compare_dirs({"count": "3"}, {"count": 3})
        self.assertEqual(len(differences), 1)


class NewAdrTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("new_adr", ROOT / "scripts" / "new_adr.py")

    def test_slugify(self) -> None:
        self.assertEqual(self.module.slugify("Use PostgreSQL-backed leases"), "use-postgresql-backed-leases")


if __name__ == "__main__":
    unittest.main()
