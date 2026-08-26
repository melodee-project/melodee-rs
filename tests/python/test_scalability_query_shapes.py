"""Architecture guards for catalog-scale request query shapes."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]


class ScalabilityQueryShapeTests(unittest.TestCase):
    def test_request_paths_never_sort_an_entire_relation_with_random(self) -> None:
        offenders: list[str] = []
        for source_root in (ROOT / "apps", ROOT / "crates"):
            for path in source_root.rglob("*.rs"):
                if "ORDER BY random()" in path.read_text(encoding="utf-8"):
                    offenders.append(str(path.relative_to(ROOT)))
        self.assertEqual(
            offenders,
            [],
            "request SQL must sample an indexed, bounded candidate window before random ordering",
        )

    def test_hot_genre_paths_use_the_serving_projection(self) -> None:
        paths = [
            ROOT / "apps/melodee-server/src/native/catalog.rs",
            ROOT / "apps/melodee-server/src/native/discovery.rs",
            ROOT / "apps/melodee-server/src/jellyfin/browse.rs",
            ROOT / "apps/melodee-server/src/opensubsonic/browse.rs",
            ROOT / "apps/melodee-server/src/opensubsonic/lists.rs",
            ROOT / "crates/melodee-persistence/src/search.rs",
        ]
        for path in paths:
            source = path.read_text(encoding="utf-8")
            self.assertNotIn(
                "file.value->'metadata'->>'genre'",
                source,
                f"{path.relative_to(ROOT)} expands manifest genre JSON on a hot path",
            )
            self.assertIn(
                "catalog_genre_terms",
                source,
                f"{path.relative_to(ROOT)} must consume the indexed genre projection",
            )

        opensubsonic = (paths[3]).read_text(encoding="utf-8")
        native = (paths[0]).read_text(encoding="utf-8")
        self.assertIn("catalog_genre_aggregates", opensubsonic)
        self.assertIn("catalog_genre_aggregates", native)


if __name__ == "__main__":
    unittest.main()
