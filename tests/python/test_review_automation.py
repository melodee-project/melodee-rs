from __future__ import annotations
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module

release = load('release_pipeline')
databases = load('test_database')
node = load('node_toolchain')
ci = load('ci_tasks')


class ReleaseSafetyTests(unittest.TestCase):
    def test_conflicting_immutable_version_never_updates_any_tag(self):
        with mock.patch.dict(os.environ, {'RELEASE_VERSION': '1.2.3'}):
            command = mock.Mock()
            inspect = mock.Mock(side_effect=['sha256:' + 'a' * 64, 'sha256:' + 'b' * 64])
            with self.assertRaisesRegex(ValueError, 'immutable'):
                release.publish_tags([release.IMAGE + ':1.2.3', release.IMAGE + ':latest'], 'candidate', inspect=inspect, command=command)
            command.assert_not_called()

    def test_matching_retry_does_not_roll_floating_tags_back(self):
        digest = 'sha256:' + 'a' * 64
        with mock.patch.dict(os.environ, {'RELEASE_VERSION': '1.2.3'}):
            command = mock.Mock()
            self.assertEqual(release.publish_tags([release.IMAGE + ':1.2.3', release.IMAGE + ':latest'], 'candidate', inspect=lambda _: digest, command=command), digest)
            command.assert_not_called()

    def test_new_release_publishes_verified_candidate_by_digest(self):
        digest = 'sha256:' + 'a' * 64
        inspect = mock.Mock(side_effect=[digest, None, digest, digest])
        command = mock.Mock()
        with mock.patch.dict(os.environ, {'RELEASE_VERSION': '1.2.3'}):
            release.publish_tags([release.IMAGE + ':1.2.3', release.IMAGE + ':latest'], 'candidate', inspect=inspect, command=command)
        self.assertEqual(len(command.call_args_list), 2)
        self.assertEqual(command.call_args_list[-1].args[0][-2], release.IMAGE + ':1.2.3')
        self.assertTrue(all(c.args[0][-1] == release.IMAGE + '@' + digest for c in command.call_args_list))

    def test_registry_auth_and_network_failures_do_not_mean_absence(self):
        for error in ('unauthorized', 'connection refused', '404 not found'):
            result = subprocess.CompletedProcess([], 1, '', error)
            with mock.patch.object(release.subprocess, 'run', return_value=result), self.assertRaises(RuntimeError):
                release.remote_digest('example')
        result = subprocess.CompletedProcess([], 1, '', 'manifest unknown')
        with mock.patch.object(release.subprocess, 'run', return_value=result):
            self.assertIsNone(release.remote_digest('example'))

    def test_version_validation_rejects_prerelease_mismatch_and_leading_zero(self):
        packages = [{'name': 'melodee-server', 'version': '1.2.3'}, {'name': 'melodee-cli', 'version': '1.2.3'}]
        self.assertEqual(release.version_metadata('v1.2.3', 'false', packages)['minor'], '1.2')
        for tag in ('v01.2.3', 'v1.2.3-rc.01', 'v1.2.3-rc..1', 'v1.2.3-rc.1'):
            with self.assertRaises(ValueError):
                release.version_metadata(tag, 'false', packages)

    def test_revision_mismatch_stops_before_build(self):
        with mock.patch.object(release, 'capture', return_value='a' * 40), self.assertRaises(ValueError):
            release.revision_matches('b' * 40)

    def test_release_workflow_pins_all_downstream_checkouts(self):
        text = (ROOT / '.github/workflows/release-container.yml').read_text()
        self.assertEqual(text.count('ref: ${{ github.event.release.tag_name }}'), 1)
        self.assertEqual(text.count('ref: ${{ needs.validate-release.outputs.revision }}'), 3)
        self.assertIn('group: melodee-release-publication', text)
        self.assertNotIn('run: |', text)


class TestDatabaseSafetyTests(unittest.TestCase):
    def test_database_url_replaces_nondefault_database_and_override_query(self):
        name = 'melodee_template_' + 'a' * 32
        actual = databases.database_url('postgres://test:synthetic@localhost:5433/test_admin?sslmode=disable&dbname=other', name)
        self.assertIn('/' + name + '?sslmode=disable', actual)
        self.assertNotIn('test_admin', actual)
        self.assertNotIn('dbname=', actual)

    def test_cleanup_refuses_even_well_formed_name_not_created_this_run(self):
        with mock.patch.object(databases, 'sql') as sql, self.assertRaises(ValueError):
            databases.drop_owned('postgres://localhost/postgres', 'melodee_template_' + 'a' * 32, set())
        sql.assert_not_called()

    def test_create_refuses_unowned_template_before_mutating(self):
        with mock.patch.object(databases, 'sql') as sql, self.assertRaises(ValueError):
            databases.create('postgres://localhost/postgres', set(), 'production')
        sql.assert_not_called()


class PnpmTests(unittest.TestCase):
    def test_corepack_uses_committed_pin_despite_global_mismatch(self):
        def which(name, *, path): return name
        versions = [subprocess.CompletedProcess([], 1, '', 'wrong version'), subprocess.CompletedProcess([], 0, '12.10.1\n', '')]
        with mock.patch.object(node.shutil, 'which', side_effect=which), mock.patch.object(node.subprocess, 'run', side_effect=versions):
            self.assertEqual(node.command(ROOT / 'admin-ui'), ['corepack', 'pnpm'])

    def test_fallback_pins_npx_package_and_does_not_change_global_version(self):
        with mock.patch.object(node.shutil, 'which', side_effect=lambda name, **kwargs: 'npx' if name == 'npx' else None):
            self.assertEqual(node.command(ROOT / 'admin-ui'), ['npx', '--yes', '--package=pnpm@12.10.1', '--', 'pnpm'])

    def test_supplied_environment_controls_lookup_and_version_probes(self):
        environment = {'PATH': '/isolated/node/bin', 'COREPACK_HOME': '/isolated/cache'}
        versions = [subprocess.CompletedProcess([], 0, '12.18.0\n', ''), subprocess.CompletedProcess([], 0, '12.10.1\n', '')]
        with mock.patch.object(node.shutil, 'which', side_effect=lambda name, **kwargs: '/isolated/node/bin/' + name) as which, mock.patch.object(node.subprocess, 'run', side_effect=versions) as run:
            self.assertEqual(node.command(ROOT / 'admin-ui', environment=environment), ['/isolated/node/bin/corepack', 'pnpm'])
        self.assertTrue(all(call.kwargs['path'] == environment['PATH'] for call in which.call_args_list))
        for call in run.call_args_list:
            self.assertIs(call.kwargs['env'], environment)
            self.assertEqual(call.kwargs['cwd'], ROOT / 'admin-ui')

    def test_environment_without_path_does_not_inherit_parent_search_path(self):
        with mock.patch.dict(os.environ, {'PATH': '/unexpected/global/bin'}), mock.patch.object(node.shutil, 'which', return_value=None) as which, self.assertRaisesRegex(RuntimeError, 'pnpm@12.10.1'):
            node.command(ROOT / 'admin-ui', environment={})
        self.assertTrue(all(call.kwargs['path'] == os.defpath for call in which.call_args_list))


class CiSafetyTests(unittest.TestCase):
    def test_cleanup_refuses_non_hosted_machine(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(ci.shutil, 'rmtree') as remove, self.assertRaises(ValueError):
            ci.free_space()
        remove.assert_not_called()

    def test_cleanup_validates_all_targets_before_any_removal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            good = root / 'good'; good.mkdir()
            unsafe = root / 'unsafe'; unsafe.symlink_to(good)
            with mock.patch.dict(os.environ, {'GITHUB_ACTIONS': 'true', 'MELODEE_CI_RUNNER_ENVIRONMENT': 'github-hosted'}), mock.patch.object(ci, 'HOSTED_TOOL_DIRECTORIES', (good, unsafe)), mock.patch.object(ci.shutil, 'rmtree') as remove, self.assertRaises(ValueError):
                ci.free_space()
            remove.assert_not_called()

    def test_new_entrypoints_help_without_services_or_mutations(self):
        for script in ('ci_tasks', 'release_pipeline', 'test_database', 'node_toolchain', 'admin_preview'):
            result = subprocess.run([sys.executable, str(ROOT / 'scripts' / (script + '.py')), '--help'], cwd=ROOT, capture_output=True, text=True, check=False, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)

if __name__ == '__main__':
    unittest.main()
