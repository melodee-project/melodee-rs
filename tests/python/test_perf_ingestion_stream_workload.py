"""Safety and measurement checks for the owned mixed ingestion workload."""
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import perf_ingestion_stream_workload as perf


class MixedWorkloadTests(unittest.TestCase):
    def test_help_is_safe(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/perf_ingestion_stream_workload.py'), '--help'],
                                env={}, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0)

    def test_unowned_database_rejected_before_io(self):
        with mock.patch.object(perf, 'query') as query, mock.patch.object(perf.subprocess, 'Popen') as popen:
            with self.assertRaises(ValueError):
                perf.run({'MELODEE__DATABASE__URL': 'postgres://host/prod'}, Path('/missing'), Path('/missing'), 1, 10, 30)
        query.assert_not_called()
        popen.assert_not_called()

    def test_limits_and_storage_fail_closed(self):
        for values in ((0, 10, 30), (201, 10, 30), (1, 9, 30), (1, 901, 30), (1, 10, 29), (1, 10, 1801)):
            with self.assertRaises(ValueError):
                perf.validate_options(*values)
        for fs in (None, 'tmpfs', 'ramfs', 'devtmpfs', 'overlay'):
            with mock.patch.object(perf, 'temporary_storage_info', return_value={'filesystem': fs}):
                with self.assertRaises(ValueError):
                    perf.require_disk(Path('.'))

    def test_only_requests_wholly_in_observed_active_window_are_counted(self):
        samples = [{'started_seconds': 0, 'finished_seconds': 2},
                   {'started_seconds': 1, 'finished_seconds': 2},
                   {'started_seconds': 2, 'finished_seconds': 4}]
        self.assertEqual(perf.active_samples(samples, 1, 3), [samples[1]])
        self.assertEqual(perf.active_samples(samples, None, None), [])

    def test_stream_first_byte_and_truncation(self):
        connection = mock.Mock()
        response = connection.getresponse.return_value
        response.status = 200
        response.read.side_effect = [b'x', b'y' * 65535]
        with mock.patch.object(perf.http.client, 'HTTPConnection', return_value=connection):
            sample = perf.stream_sample(123, 'secret', 'abc')
        self.assertIsNone(sample['error'])
        self.assertEqual(sample['bytes'], 65536)
        self.assertEqual(response.read.call_args_list, [mock.call(1), mock.call(65535)])
        connection.close.assert_called_once()
        self.assertNotIn('secret', str(sample))

    def test_truncated_stream_and_exception_are_failure_samples(self):
        connection = mock.Mock()
        response = connection.getresponse.return_value
        response.status = 200
        response.read.side_effect = [b'x', b'']
        with mock.patch.object(perf.http.client, 'HTTPConnection', return_value=connection):
            self.assertIsNotNone(perf.stream_sample(123, 'secret', 'abc')['error'])
        connection.request.side_effect = TimeoutError('secret')
        with mock.patch.object(perf.http.client, 'HTTPConnection', return_value=connection):
            sample = perf.stream_sample(123, 'secret', 'abc')
        self.assertEqual(sample['error'], 'TimeoutError')
        self.assertNotIn('secret', str(sample))


if __name__ == '__main__':
    unittest.main()
