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


class R4DualRunScenarioTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("run_r4_dual_run", ROOT / "scripts" / "run_r4_dual_run.py")

    def test_replays_the_separate_disabled_podcast_phase(self) -> None:
        self.assertIn("opensubsonic-conditional-off.json", self.module.SCENARIO_FILES)


class R4AdrDecisionPackageTests(unittest.TestCase):
    def test_adr_0150_inventory_matches_committed_surface_reports(self) -> None:
        expected = {"native": 181, "opensubsonic": 330, "jellyfin": 17}
        for surface, count in expected.items():
            with self.subTest(surface=surface):
                report = json.loads(
                    (
                        ROOT
                        / "design"
                        / "evidence"
                        / "r4"
                        / f"dual-run-report-{surface}.json"
                    ).read_text()
                )
                self.assertEqual(len(report["unexpected"]), count)

        package = (
            ROOT / "design" / "evidence" / "r4" / "adr-0150-decision-package.md"
        ).read_text()
        for required_text in (
            "181, 330, and 17",
            "Reference-only metadata",
            "Accepted schema-backed projection",
            "Bounded adapter reconstruction",
            "no comparator mask is added here",
        ):
            with self.subTest(required_text=required_text):
                self.assertIn(required_text, package)


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

    def write_capture(
        self,
        directory: Path,
        body: dict | bytes,
        *,
        status: int = 200,
        normalize: dict | None = None,
        request: dict | None = None,
        structure_checks: dict | None = None,
    ) -> None:
        directory.mkdir(parents=True, exist_ok=True)
        payload = body if isinstance(body, bytes) else (json.dumps(body, indent=2) + "\n").encode()
        (directory / "scenario.json").write_bytes(payload)
        metadata = {
            "name": "scenario",
            "request": request
            or {
                "method": "GET",
                "url": "http://example.test/x",
                "auth": "none",
                "headers": {},
                "jsonTemplate": None,
                "bodyTemplate": None,
            },
            "response": {
                "status": status,
                "headers": {"content-type": "application/json"},
                "bodyFile": "scenario.json",
                "sha256": "unused",
                "bytes": len(payload),
            },
            "normalize": normalize or {},
            "structureChecks": structure_checks,
            "error": None,
            "allowFailure": False,
        }
        (directory / "scenario.metadata.json").write_text(json.dumps(metadata))

    def compare_dirs(
        self,
        left_body: dict | bytes,
        right_body: dict | bytes,
        normalize: dict | None = None,
        *,
        left_request: dict | None = None,
        right_request: dict | None = None,
        left_checks: dict | None = None,
        right_checks: dict | None = None,
        right_normalize: dict | None = None,
    ):
        with tempfile.TemporaryDirectory() as tmp:
            left = Path(tmp) / "left"
            right = Path(tmp) / "right"
            self.write_capture(
                left,
                left_body,
                normalize=normalize,
                request=left_request,
                structure_checks=left_checks,
            )
            self.write_capture(
                right,
                right_body,
                normalize=right_normalize if right_normalize is not None else normalize,
                request=right_request,
                structure_checks=right_checks,
            )
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

    def test_json_integral_and_fractional_numbers_are_wire_equivalent(self) -> None:
        self.assertEqual(self.compare_dirs({"count": 0}, {"count": 0.0}), [])

    def test_request_method_and_url_are_contract(self) -> None:
        expected = {
            "method": "GET",
            "url": "{melodee-origin}/api/v1/artists?page=1",
            "auth": "bearer-var:accessToken",
            "headers": {},
            "jsonTemplate": None,
            "bodyTemplate": None,
        }
        actual = {**expected, "method": "POST", "url": "{melodee-origin}/api/v1/artists"}
        differences = self.compare_dirs(
            {"data": []},
            {"data": []},
            left_request=expected,
            right_request=actual,
        )
        self.assertEqual(
            {difference.field for difference in differences},
            {"request.method", "request.url"},
        )

    def test_declared_request_variables_are_normalized_but_route_text_is_not(self) -> None:
        template = "/api/v1/requests/{{requestId}}/seen"
        self.assertEqual(
            self.module.normalize_request_url(
                "{melodee-origin}/api/v1/requests/old-id/seen", template
            ),
            "{melodee-origin}/api/v1/requests/<scenario-variable>/seen",
        )
        self.assertEqual(
            self.module.normalize_request_url(
                "{melodee-origin}/api/v1/requests/old-id/complete", template
            ),
            "{melodee-origin}/api/v1/requests/old-id/complete",
        )

    def test_actual_capture_cannot_widen_baseline_normalization(self) -> None:
        differences = self.compare_dirs(
            {"stable": 1},
            {"stable": 2},
            normalize={},
            right_normalize={"maskJsonPaths": ["$.stable"]},
        )
        self.assertEqual(
            {difference.field for difference in differences},
            {"normalize", "response.body$.stable"},
        )

    def test_token_structure_checks_are_contract(self) -> None:
        differences = self.compare_dirs(
            {"token": "<volatile>"},
            {"token": "<volatile>"},
            left_checks={"$.token": {"valid": True, "headerAlg": "HS256"}},
            right_checks={"$.token": {"valid": False}},
        )
        self.assertEqual([difference.field for difference in differences], ["structureChecks"])

    def test_stream_structure_expiry_instant_is_replay_volatile(self) -> None:
        checks = {
            "$.streamUrl": {
                "type": "streamAuthToken",
                "valid": True,
                "expiresAt": "2026-08-16T19:54:25Z",
                "expiresInFuture": True,
                "expiresInDays": 7,
                "hmacShape": "base64-sha256",
            }
        }
        replayed = {**checks, "$.streamUrl": {**checks["$.streamUrl"], "expiresAt": "2026-08-25T19:33:13Z"}}
        self.assertEqual(self.compare_dirs({"stream": "a"}, {"stream": "a"}, left_checks=checks, right_checks=replayed), [])

    def test_binary_difference_reports_hashes_even_when_length_matches(self) -> None:
        differences = self.compare_dirs(b"abc", b"abd")
        self.assertEqual(len(differences), 1)
        self.assertIn("sha256=", differences[0].expected)
        self.assertIn("sha256=", differences[0].actual)

    def test_incremental_scenario_selection_ignores_unselected_capture_rows(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            expected = Path(tmp) / "expected"
            actual = Path(tmp) / "actual"
            self.write_capture(expected, {"value": 1})
            self.write_capture(actual, {"value": 1})

            extra_body = (json.dumps({"value": 2}) + "\n").encode()
            (expected / "extra.json").write_bytes(extra_body)
            extra_metadata = json.loads((expected / "scenario.metadata.json").read_text())
            extra_metadata["name"] = "extra"
            extra_metadata["response"]["bodyFile"] = "extra.json"
            (expected / "extra.metadata.json").write_text(json.dumps(extra_metadata))

            self.assertEqual(self.module.compare(expected, actual, {"scenario"}), [])
            differences = self.module.compare(expected, actual)
            self.assertEqual([difference.scenario for difference in differences], ["extra"])

    def test_unknown_incremental_scenario_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            expected = Path(tmp) / "expected"
            actual = Path(tmp) / "actual"
            self.write_capture(expected, {"value": 1})
            self.write_capture(actual, {"value": 1})
            differences = self.module.compare(expected, actual, {"missing"})
            self.assertEqual(len(differences), 1)
            self.assertEqual(differences[0].actual, "unknown")


class R4DifferenceClassificationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module(
            "run_phase8_differential", ROOT / "scripts" / "run_phase8_differential.py"
        )

    def test_replay_account_timestamps_use_only_narrow_volatile_rules(self) -> None:
        self.assertEqual(
            self.module.is_known_difference(
                "catalog/user-playlists-limit-param",
                "response.body$.data[0].owner.createdAt",
                '"old"',
                '"new"',
            ),
            "replay-created account timestamp is volatile",
        )
        self.assertEqual(
            self.module.is_known_difference(
                "native-user-extra/admin-users",
                "response.body$[0].lastLoginAt",
                '"old"',
                '"new"',
            ),
            "replay login timestamp is volatile",
        )

    def test_volatile_rules_do_not_widen_to_other_user_or_playlist_fields(self) -> None:
        self.assertIsNone(
            self.module.is_known_difference(
                "catalog/user-playlists-limit-param",
                "response.body$.data[0].owner.username",
                '"old"',
                '"new"',
            )
        )
        self.assertIsNone(
            self.module.is_known_difference(
                "native-user-extra/admin-users",
                "response.body$[0].email",
                '"old"',
                '"new"',
            )
        )

    def test_image_and_release_date_rules_are_exact_scenario_field_pairs(self) -> None:
        image_entries = [
            entry
            for entry in self.module.KNOWN_DIFFERENCES
            if "exact captured field is volatile" in entry[2]
        ]
        release_date_entries = [
            entry
            for entry in self.module.KNOWN_DIFFERENCES
            if "exact captured field" in entry[2]
            and "OriginalReleaseDate" in entry[2]
        ]
        self.assertEqual(len(image_entries), 32)
        self.assertEqual(len(release_date_entries), 6)
        for scenario, field, reason in image_entries + release_date_entries:
            self.assertEqual(
                self.module.is_known_difference(scenario, field, '"old"', '"new"'),
                reason,
            )

        self.assertIsNone(
            self.module.is_known_difference(
                "os-getalbum-compilation",
                "response.body$.subsonic-response.album.titleImageUrl",
                '"old"',
                '"new"',
            )
        )
        self.assertIsNone(
            self.module.is_known_difference(
                "os-getalbum-minimal",
                "response.body$.subsonic-response.album.artists[0].artistImageUrl",
                '"old"',
                '"new"',
            )
        )
        self.assertIsNone(
            self.module.is_known_difference(
                "os-getalbuminfo2-extra",
                "response.body$.subsonic-response.albumInfo.largeImageUrl",
                '"old"',
                '"new"',
            )
        )
        self.assertIsNone(
            self.module.is_known_difference(
                "os-getalbum-multidisc",
                "response.body$.subsonic-response.album.releaseDate.year",
                '"old"',
                '"new"',
            )
        )
        self.assertIn(
            "null-SortName",
            self.module.is_known_difference(
                "os-getalbum-multidisc",
                "response.body$.subsonic-response.album.sortName",
                '"old"',
                '"new"',
            ),
        )

class NewAdrTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.module = load_module("new_adr", ROOT / "scripts" / "new_adr.py")

    def test_slugify(self) -> None:
        self.assertEqual(self.module.slugify("Use PostgreSQL-backed leases"), "use-postgresql-backed-leases")


if __name__ == "__main__":
    unittest.main()
