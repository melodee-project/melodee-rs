"""Safety and size checks for the disposable representative fixture."""
import importlib.util
import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("perf_representative", ROOT / "scripts" / "perf_representative.py")
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RepresentativeFixtureTests(unittest.TestCase):
    def test_acl_placeholder_option_controls_the_earliest_catalog_imports(self):
        # The policy dry-run loader measures the earliest catalog_imports rows.
        # The ACL placeholders occupy those rows, so the opt-out must actually
        # remove them or a bounded policy benchmark silently measures fixture
        # placeholders instead of the workload's own Releases.
        with_acl = MODULE.fixture_sql(2, 2, 1)
        without_acl = MODULE.fixture_sql(2, 2, 1, acl_placeholders=False)
        self.assertIn("bench_acl_releases", with_acl)
        self.assertIn("INSERT INTO catalog_imports", with_acl)
        self.assertNotIn("bench_acl_releases", without_acl)
        self.assertNotIn("INSERT INTO catalog_imports", without_acl)
        self.assertIn("INSERT INTO tracks", without_acl)

    def test_rejects_invalid_sizes(self):
        for sizes in [(0, 1, 1), (1, -1, 1), (1, 1, 0)]:
            with self.assertRaises(ValueError):
                MODULE.fixture_sql(*sizes)

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "O_NOFOLLOW")
                         and hasattr(os, "O_DIRECTORY"), "no-follow directory support required")
    def test_artwork_install_rejects_symlinked_shard_and_preserves_external_file(self):
        content = MODULE.fixture_artwork()
        digest = hashlib.sha256(content).hexdigest()
        key = f"{digest}-{len(content)}"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "owned"
            external = Path(directory) / "external"
            root.mkdir()
            external.mkdir()
            sentinel = external / key
            sentinel.write_bytes(b"external data")
            (root / digest[:2]).symlink_to(external, target_is_directory=True)
            with self.assertRaises(OSError):
                MODULE.install_fixture_artwork(root)
            self.assertEqual(sentinel.read_bytes(), b"external data")

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "O_NOFOLLOW")
                         and hasattr(os, "O_DIRECTORY"), "no-follow directory support required")
    def test_artwork_install_never_overwrites_an_existing_object(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            key, _ = MODULE.install_fixture_artwork(root)
            existing = root / key[:2] / key
            before = existing.read_bytes()
            with self.assertRaises(FileExistsError):
                MODULE.install_fixture_artwork(root)
            self.assertEqual(existing.read_bytes(), before)

    @unittest.skipUnless(os.name == "posix" and hasattr(os, "O_NOFOLLOW")
                         and hasattr(os, "O_DIRECTORY"), "no-follow directory support required")
    def test_artwork_install_rejects_symlinked_root_and_object(self):
        content = MODULE.fixture_artwork()
        digest = hashlib.sha256(content).hexdigest()
        key = f"{digest}-{len(content)}"
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "owned"
            root.mkdir()
            linked_root = base / "root-link"
            linked_root.symlink_to(root, target_is_directory=True)
            with self.assertRaises(OSError):
                MODULE.install_fixture_artwork(linked_root)
            shard = root / digest[:2]
            shard.mkdir()
            external = base / "external"
            external.write_bytes(b"external data")
            (shard / key).symlink_to(external)
            with self.assertRaises(FileExistsError):
                MODULE.install_fixture_artwork(root)
            self.assertEqual(external.read_bytes(), b"external data")

    @unittest.skipUnless(os.environ.get("MELODEE_PERF_OWNED_DATABASE"),
                         "requires an invocation-owned representative fixture")
    def test_seeded_ratings_cover_all_five_values(self):
        """Run inside the fixture workload with at least 100 releases and 10 tracks each."""
        name = os.environ["MELODEE_PERF_OWNED_DATABASE"]
        url = os.environ["MELODEE__DATABASE__URL"]
        self.assertRegex(name, r"^melodee_template_[0-9a-f]{32}$")
        self.assertEqual(unquote(urlsplit(url).path.lstrip("/")), name)
        for table in ("native_artist_user_states", "native_release_user_states",
                      "native_track_user_states"):
            with self.subTest(table=table):
                result = subprocess.run(
                    ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
                     "--no-align", "--dbname", url, "--command",
                     f"SELECT array_agg(DISTINCT rating ORDER BY rating) FROM {table} WHERE rating > 0"],
                    capture_output=True, text=True, check=True,
                )
                self.assertEqual(result.stdout.strip(), "{1,2,3,4,5}")

    @unittest.skipUnless(os.environ.get("MELODEE_PERF_OWNED_DATABASE"),
                         "requires an invocation-owned representative fixture")
    def test_seeded_playable_and_alternate_assets(self):
        """Run inside a fixture large enough to include alternate assets."""
        name = os.environ["MELODEE_PERF_OWNED_DATABASE"]
        url = os.environ["MELODEE__DATABASE__URL"]
        self.assertRegex(name, r"^melodee_template_[0-9a-f]{32}$")
        self.assertEqual(unquote(urlsplit(url).path.lstrip("/")), name)
        result = subprocess.run(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
             "--no-align", "--dbname", url, "--command",
             "SELECT purpose, count(*) FROM track_media_assets GROUP BY purpose ORDER BY purpose"],
            capture_output=True, text=True, check=True,
        )
        counts = dict(line.split("|") for line in result.stdout.splitlines())
        self.assertGreater(int(counts["playable"]), 0)
        self.assertGreater(int(counts["alternate"]), 0)
        cardinalities = subprocess.run(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
             "--no-align", "--dbname", url, "--command",
             "SELECT (SELECT count(*) FROM artists), (SELECT count(*) FROM releases), "
             "(SELECT count(*) FROM tracks)"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        artists, releases, tracks = (int(value) for value in cardinalities.split("|"))
        self.assertGreater(artists, 0)
        self.assertGreater(releases, 0)
        self.assertEqual(int(counts["playable"]), tracks)
        print(f"Observed catalog: {artists} artists, {releases} releases, {tracks} tracks; "
              f"{counts['alternate']} alternate assets", flush=True)

    @unittest.skipUnless(os.environ.get("MELODEE_PERF_OWNED_DATABASE"),
                         "requires an invocation-owned representative fixture")
    def test_seeded_artwork_references_resolve_to_content_addressed_file(self):
        name = os.environ["MELODEE_PERF_OWNED_DATABASE"]
        url = os.environ["MELODEE__DATABASE__URL"]
        self.assertRegex(name, r"^melodee_template_[0-9a-f]{32}$")
        self.assertEqual(unquote(urlsplit(url).path.lstrip("/")), name)
        result = subprocess.run(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
             "--no-align", "--dbname", url, "--command",
             "SELECT entity_type, asset_key, count(*) FROM catalog_entity_images "
             "GROUP BY entity_type, asset_key ORDER BY entity_type"],
            capture_output=True, text=True, check=True,
        )
        rows = [line.split("|") for line in result.stdout.splitlines()]
        self.assertEqual([row[0] for row in rows], ["artist", "release"])
        root = Path(os.environ["MELODEE__STORAGE__ASSET_ROOT"])
        for _, key, count in rows:
            self.assertGreater(int(count), 0)
            digest, size = key.split("-")
            content = (root / digest[:2] / key).read_bytes()
            self.assertTrue(content.startswith(b"\x89PNG\r\n\x1a\n"))
            self.assertEqual(len(content), int(size))
            self.assertEqual(hashlib.sha256(content).hexdigest(), digest)

    @unittest.skipUnless(os.environ.get("MELODEE_PERF_OWNED_DATABASE"),
                         "requires an invocation-owned representative fixture")
    def test_seeded_acl_exercises_allowed_and_denied_releases(self):
        name = os.environ["MELODEE_PERF_OWNED_DATABASE"]
        url = os.environ["MELODEE__DATABASE__URL"]
        self.assertRegex(name, r"^melodee_template_[0-9a-f]{32}$")
        self.assertEqual(unquote(urlsplit(url).path.lstrip("/")), name)
        result = subprocess.run(
            ["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
             "--no-align", "--dbname", url, "--command",
             "SELECT u.normalized_username, "
             "count(*) FILTER (WHERE native_user_can_access_release(u.id, r.id)), "
             "count(*) FILTER (WHERE NOT native_user_can_access_release(u.id, r.id)) "
             "FROM native_users u CROSS JOIN releases r "
             "WHERE u.normalized_username IN ('BENCHMARK-2', 'BENCHMARK-60') "
             "GROUP BY u.normalized_username ORDER BY u.normalized_username"],
            capture_output=True, text=True, check=True,
        )
        rows = [line.split("|") for line in result.stdout.splitlines()]
        self.assertEqual([row[0] for row in rows], ["BENCHMARK-2", "BENCHMARK-60"])
        for _, allowed, denied in rows:
            self.assertGreater(int(allowed), 0)
            self.assertGreater(int(denied), 0)

    def test_quiescence_refuses_unowned_or_retargeted_databases_before_psql(self):
        name = "melodee_template_" + "a" * 32
        admin = "postgres://user:secret@localhost:5433/postgres"
        url = MODULE.test_database.database_url(admin, name)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(MODULE.subprocess, "run") as run:
            for created, actual_url in ((set(), url), ({name}, MODULE.test_database.database_url(admin, "melodee_template_" + "b" * 32))):
                with self.assertRaises(ValueError):
                    MODULE.quiesce_owned_fixture(admin, name, actual_url, created, Path(directory), {})
            run.assert_not_called()

    def test_quiescence_vacuums_only_confirmed_fixture_and_waits_for_stability(self):
        name = "melodee_template_" + "a" * 32
        admin = "postgres://user:secret@localhost:5433/postgres"
        url = MODULE.test_database.database_url(admin, name)
        observations = iter([{"active": 1, "database_size": 10, "tables": []},
                             {"active": 0, "database_size": 10, "tables": []},
                             *[{"active": 0, "database_size": 11, "tables": []}] * 3])
        calls = []
        def run(args, **kwargs):
            calls.append((args, kwargs))
            statement = args[-1]
            result = name if statement == "SELECT current_database()" else ("VACUUM" if statement == "VACUUM (ANALYZE)" else json.dumps(next(observations)))
            return subprocess.CompletedProcess(args, 0, stdout=result)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE.time, "sleep"):
            result = MODULE.quiesce_owned_fixture(admin, name, url, {name}, Path(directory), {}, timeout=10)
        self.assertEqual(result["stable_observations"], 3)
        self.assertEqual(result["observations"], 5)
        self.assertEqual(calls[1][0][-1], "VACUUM (ANALYZE)")
        self.assertTrue(all(call[0][call[0].index("--dbname") + 1] == url for call in calls))
        self.assertTrue(all(call[1]["timeout"] <= 10 for call in calls))

    def test_quiescence_timeout_is_propagated_without_observing_fake_stability(self):
        name = "melodee_template_" + "d" * 32
        admin = "postgres://user:secret@localhost:5433/postgres"
        url = MODULE.test_database.database_url(admin, name)
        def run(args, **_kwargs):
            if args[-1] == "SELECT current_database()":
                return subprocess.CompletedProcess(args, 0, stdout=name)
            raise subprocess.TimeoutExpired(args, 3)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(MODULE.subprocess, "run", side_effect=run):
            with self.assertRaises(subprocess.TimeoutExpired):
                MODULE.quiesce_owned_fixture(admin, name, url, {name}, Path(directory), {}, timeout=3)

    def test_quiescence_rejects_wrong_connected_database_before_vacuum(self):
        name = "melodee_template_" + "c" * 32
        admin = "postgres://user:secret@localhost:5433/postgres"
        url = MODULE.test_database.database_url(admin, name)
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(MODULE.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, stdout="other")) as run:
            with self.assertRaises(ValueError):
                MODULE.quiesce_owned_fixture(admin, name, url, {name}, Path(directory), {})
            self.assertEqual(run.call_count, 1)

    def test_help_is_safe_without_database(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "perf_representative.py"), "--help"],
            env={"MELODEE_TEST_DATABASE_URL": "invalid"}, capture_output=True, text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0)
        self.assertIn("--tracks-per-release", result.stdout)

    def test_migration_proof_requires_exact_binary_source_and_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary, proof = root / "cli", root / "proof.json"
            binary.write_bytes(b"cli")
            record = {"target": "migration-cli", "build_profile": "release", "binary_sha256": MODULE.file_digest(binary),
                      "commit": "fixture", "source_sha256": {"source": "digest"},
                      "provenance_producer_sha256": MODULE.file_digest(ROOT / "scripts/build_perf_cli.py"),
                      "fingerprint_helper_sha256": MODULE.file_digest(ROOT / "scripts/build_perf_server.py")}
            with mock.patch.object(MODULE, "source_fingerprints", return_value={"source": "digest"}), \
                 mock.patch.object(MODULE, "command_output", return_value="fixture"):
                proof.write_text(json.dumps(record))
                self.assertEqual(MODULE.validate_migration_proof(binary, proof), MODULE.file_digest(proof))
                for key in ("binary_sha256", "source_sha256", "commit", "provenance_producer_sha256", "fingerprint_helper_sha256"):
                    proof.write_text(json.dumps({**record, key: "wrong"}))
                    with self.subTest(key=key), self.assertRaises(ValueError):
                        MODULE.validate_migration_proof(binary, proof)

    def test_invalid_migration_proof_refuses_before_database_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            proof = Path(directory) / "proof.json"
            proof.write_text('{}')
            args = ["perf_representative.py", "--migration-cli", sys.executable,
                    "--migration-cli-provenance", str(proof), "--", "workload"]
            with mock.patch.object(sys, "argv", args), mock.patch.object(MODULE.test_database, "create") as create, \
                 contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(MODULE.main(), 1)
                create.assert_not_called()

    def test_workload_receives_exact_created_database_and_cleanup_runs(self):
        name = "melodee_template_" + "a" * 32
        calls = []
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            if args[-1:] == ["migrate"]:
                self.assertEqual((kwargs["cwd"] / ".env").read_bytes(), b"")
                self.assertEqual(kwargs["env"]["MELODEE__STORAGE__ASSET_ROOT"], str(kwargs["cwd"]))
                self.assertNotIn("PRIVATE_TOKEN", kwargs["env"])
                self.assertNotIn("MELODEE__UNRELATED_SECRET", kwargs["env"])
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0)
        def owned(args, **kwargs):
            calls.append((args, kwargs))
            return 0
        def drop(_admin, target, created):
            self.assertEqual(target, name)
            created.remove(target)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--artists", "1", "--releases", "1", "--tracks-per-release", "1", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.dict(MODULE.os.environ, {"PRIVATE_TOKEN": "secret", "MELODEE__UNRELATED_SECRET": "secret"}), \
             mock.patch.object(MODULE.test_database, "drop_owned", side_effect=drop) as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", side_effect=owned), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 0)
        self.assertEqual(calls[-1][0], ["workload"])
        self.assertNotIn("PRIVATE_TOKEN", calls[-1][1]["env"])
        self.assertNotIn("MELODEE__UNRELATED_SECRET", calls[-1][1]["env"])
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_OWNED_DATABASE"], name)
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_FIXTURE_PID"], str(MODULE.os.getpid()))
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_FIXTURE_SHA256"], MODULE.file_digest(ROOT / "scripts/perf_representative.py"))
        self.assertEqual(calls[-1][1]["env"]["MELODEE_PERF_DATABASE_HELPER_SHA256"], MODULE.file_digest(ROOT / "scripts/test_database.py"))
        asset_root = Path(calls[-1][1]["env"]["MELODEE__STORAGE__ASSET_ROOT"])
        self.assertTrue(asset_root.is_absolute())
        self.assertFalse(asset_root.exists(), "invocation-owned assets must be cleaned")
        cleanup.assert_called_once()

    def test_workload_failure_still_cleans_up_owned_database(self):
        name = "melodee_template_" + "b" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **_kwargs):
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", return_value=1), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_interrupted_workload_still_cleans_up_owned_database(self):
        name = "melodee_template_" + "e" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **_kwargs):
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", side_effect=InterruptedError("test")), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(MODULE.main(), 1)
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_baseline_cli_targets_only_the_owned_database(self):
        name = "melodee_template_" + "c" * 32
        calls = []
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            calls.append((args, kwargs))
            return subprocess.CompletedProcess(args, 0)
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--migration-cli", sys.executable, "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE.test_database, "migrate") as default_migrate, \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             mock.patch.object(MODULE, "run_owned_child", return_value=0), \
             contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(MODULE.main(), 0)
        self.assertEqual(calls[0][0], [str(Path(sys.executable).resolve()), "migrate"])
        self.assertTrue(calls[0][1]["env"]["MELODEE__DATABASE__URL"].endswith("/" + name))
        default_migrate.assert_not_called()
        self.assertEqual(cleanup.call_args.args[1], name)

    def test_seed_timeout_is_censored_and_owned_database_is_cleaned(self):
        name = "melodee_template_" + "d" * 32
        def create(_admin, created):
            created.add(name)
            return name
        def run(args, **kwargs):
            if args[0] == "psql":
                raise subprocess.TimeoutExpired(args, kwargs["timeout"])
            return subprocess.CompletedProcess(args, 0)
        output = io.StringIO()
        with mock.patch.object(sys, "argv", ["perf_representative.py", "--seed-timeout", "1", "--", "workload"]), \
             mock.patch.object(MODULE.test_database, "create", side_effect=create), \
             mock.patch.object(MODULE, "default_migration_cli", return_value=Path(sys.executable)), \
             mock.patch.object(MODULE.test_database, "drop_owned") as cleanup, \
             mock.patch.object(MODULE.subprocess, "run", side_effect=run), \
             contextlib.redirect_stdout(output):
            self.assertEqual(MODULE.main(), 124)
        self.assertIn('"status": "seed_timeout_censored"', output.getvalue())
        self.assertEqual(cleanup.call_args.args[1], name)

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_interrupted_owned_child_is_signalled_and_reaped(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), 0]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child) as popen, \
             mock.patch.object(MODULE.os, "getpgid", return_value=4567), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                MODULE.run_owned_child(["workload"], ROOT, {})
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        kill.assert_called_once_with(4567, MODULE.signal.SIGTERM)
        self.assertEqual(child.wait.call_args.kwargs, {"timeout": 45})

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_changed_child_topology_is_never_signalled(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = KeyboardInterrupt()
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", return_value=99), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(RuntimeError):
                MODULE.run_owned_child(["workload"], ROOT, {})
        kill.assert_not_called()

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_unresponsive_owned_child_is_killed_only_after_topology_revalidation(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired("workload", 45), 0]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", return_value=4567) as group, \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                MODULE.run_owned_child(["workload"], ROOT, {})
        self.assertEqual(group.call_count, 2)
        self.assertEqual(kill.call_args_list, [mock.call(4567, MODULE.signal.SIGTERM),
                                               mock.call(4567, MODULE.signal.SIGKILL)])

    @unittest.skipUnless(MODULE.os.name == "posix", "POSIX process groups required")
    def test_topology_change_after_termination_refuses_group_kill(self):
        child = mock.Mock(pid=4567)
        child.wait.side_effect = [KeyboardInterrupt(), subprocess.TimeoutExpired("workload", 45)]
        child.poll.return_value = None
        with mock.patch.object(MODULE.subprocess, "Popen", return_value=child), \
             mock.patch.object(MODULE.os, "getpgid", side_effect=[4567, 99]), \
             mock.patch.object(MODULE.os, "getsid", return_value=4567), \
             mock.patch.object(MODULE.os, "killpg") as kill:
            with self.assertRaises(RuntimeError):
                MODULE.run_owned_child(["workload"], ROOT, {})
        kill.assert_called_once_with(4567, MODULE.signal.SIGTERM)


if __name__ == "__main__":
    unittest.main()
