import importlib.util
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[1] / 'analysis' / 'benchmark.py'
spec = importlib.util.spec_from_file_location('benchmark_analysis', MODULE)
b = importlib.util.module_from_spec(spec)
spec.loader.exec_module(b)


class AnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.capture = self.root / 'capture.csv'
        self.capture.write_text('timestamp_seconds,frame_time_ms\n0,1000\n1,1000\n2,1000\n3,1000\n4,1000\n')
        self.manifest = {'adapter': 'per-frame-csv', 'raw_capture': 'capture.csv',
                         'markers': {'capture_start_seconds': 1, 'capture_end_seconds': 3,
                                     'scenario_start_seconds': 100, 'scenario_end_seconds': 102,
                                     'alignment_method': 'synchronized recording'},
                         'checks': {key: True for key in b.CHECKS},
                         'metadata': {key: 'recorded' for key in b.META}}
        self.manifest['metadata']['planned_duration_seconds'] = 2
        self.path = self.root / 'manifest.json'

    def run_analysis(self, name='out'):
        self.path.write_text(json.dumps(self.manifest))
        out = self.root / name
        out.mkdir()
        return b.analyze(self.path, out)

    def test_window_metrics_and_graph(self):
        result = self.run_analysis()
        self.assertTrue(result['valid'])
        self.assertEqual(result['metrics']['frame_count'], 2)
        self.assertEqual(result['metrics']['average_fps'], 1)
        self.assertEqual(result['metrics']['one_percent_low_fps'], 1)
        self.assertEqual(result['metrics']['p99_frame_time_ms'], 1000)
        self.assertTrue((self.root / 'out' / 'frame-times.svg').exists())
        self.assertTrue(self.capture.exists())

    def test_percentile_and_ceil_low_count(self):
        result = b.metrics([(i, i) for i in range(1, 102)], 10)
        self.assertEqual(result['one_percent_sample_count'], 2)
        self.assertEqual(result['p99_frame_time_ms'], 100)
        self.assertAlmostEqual(result['one_percent_low_fps'], 1000 / 100.5)

    def test_invalid_checks_and_coverage_do_not_score(self):
        self.manifest['checks']['no_unexpected_pause'] = False
        self.manifest['markers']['capture_end_seconds'] = 5
        result = self.run_analysis()
        self.assertFalse(result['valid'])
        self.assertNotIn('metrics', result)
        self.assertEqual(len(result['invalid_reasons']), 2)

    def test_non_monotonic_and_nonfinite_are_rejected(self):
        for body in ('0,10\n1,20\n1,30\n4,40\n', '0,10\n1,nan\n4,40\n'):
            self.capture.write_text('timestamp_seconds,frame_time_ms\n' + body)
            with self.assertRaises(ValueError):
                b.read_frames(self.capture, 'per-frame-csv')

    def test_presentmon_columns_and_semantics(self):
        self.capture.write_text('TimeInSeconds,MsBetweenPresents\n0,1000\n1,1000\n2,1000\n3,1000\n4,1000\n')
        self.manifest['adapter'] = 'presentmon'
        self.manifest['metadata']['frame_semantics'] = 'presented'
        self.assertTrue(self.run_analysis()['valid'])

    def test_empty_mods_and_presentmon_initial_undefined(self):
        self.capture.write_text('TimeInSeconds,MsBetweenPresents\n0,0\n1,1000\n2,1000\n3,1000\n4,1000\n')
        self.manifest['adapter'] = 'presentmon'
        self.manifest['metadata']['frame_semantics'] = 'presented'
        self.manifest['metadata']['mods'] = []
        self.assertTrue(self.run_analysis()['valid'])

    def test_missing_frame_interval_is_rejected(self):
        self.capture.write_text('timestamp_seconds,frame_time_ms\n0,1000\n1,1000\n3,1000\n4,1000\n')
        with self.assertRaisesRegex(ValueError, 'does not match'):
            b.read_frames(self.capture, 'per-frame-csv')

    def test_graph_write_failure_does_not_produce_valid_score(self):
        with patch.object(b, 'graph', side_effect=OSError('disk full')):
            result = self.run_analysis()
        self.assertFalse(result['valid'])
        self.assertNotIn('metrics', result)
        self.assertTrue(any('disk full' in reason for reason in result['invalid_reasons']))
        saved = json.loads((self.root / 'out' / 'run.json').read_text())
        self.assertEqual(saved, result)

    def test_malformed_manifest_cli_fails_without_traceback(self):
        for i, malformed in enumerate(([], {'invalid_reasons': 'not a list'})):
            self.path.write_text(json.dumps(malformed))
            process = subprocess.run([sys.executable, str(MODULE), 'run', str(self.path),
                                      '--output', str(self.root / f'bad{i}')],
                                     text=True, capture_output=True, check=False)
            self.assertEqual(process.returncode, 2)
            self.assertIn('error:', process.stderr)
            self.assertNotIn('Traceback', process.stderr)

    def test_cli_invalid_run_persists_reason_without_altering_raw(self):
        self.manifest['checks']['no_lost_capture_data'] = False
        self.path.write_text(json.dumps(self.manifest))
        before = self.capture.read_bytes()
        out = self.root / 'cli-invalid'
        process = subprocess.run([sys.executable, str(MODULE), 'run', str(self.path), '--output', str(out)],
                                 text=True, capture_output=True, check=False)
        self.assertEqual(process.returncode, 2, process.stderr)
        saved = json.loads((out / 'run.json').read_text())
        self.assertFalse(saved['valid'])
        self.assertIn('check not confirmed: no_lost_capture_data', saved['invalid_reasons'])
        self.assertNotIn('metrics', saved)
        self.assertFalse((out / 'frame-times.svg').exists())
        self.assertEqual(self.capture.read_bytes(), before)
        retry = subprocess.run([sys.executable, str(MODULE), 'run', str(self.path), '--output', str(out)],
                               text=True, capture_output=True, check=False)
        self.assertEqual(retry.returncode, 2)
        self.assertEqual(json.loads((out / 'run.json').read_text()), saved)

    def test_cli_report_end_to_end(self):
        run_files = []
        for i in range(3):
            capture = self.root / f'capture{i}.csv'
            capture.write_text('timestamp_seconds,frame_time_ms,run\n' +
                               ''.join(f'{j},1000,{i}\n' for j in range(5)))
            self.manifest['raw_capture'] = capture.name
            self.path.write_text(json.dumps(self.manifest))
            out = self.root / f'run{i}'
            run = subprocess.run([sys.executable, str(MODULE), 'run', str(self.path), '--output', str(out)],
                                 text=True, capture_output=True, check=False)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertTrue((out / 'frame-times.svg').exists())
            run_files.append(str(out / 'run.json'))
        report_path = self.root / 'report.json'
        process = subprocess.run([sys.executable, str(MODULE), 'report', *run_files,
                                  '--output', str(report_path)], text=True, capture_output=True, check=False)
        self.assertEqual(process.returncode, 0, process.stderr)
        report = json.loads(report_path.read_text())
        self.assertEqual(report['metrics']['average_fps'],
                         {'runs': [1.0, 1.0, 1.0], 'median': 1.0, 'min': 1.0, 'max': 1.0})

    def test_report_requires_independent_matching_runs(self):
        results = []
        for i in range(3):
            self.capture.write_text(f'timestamp_seconds,frame_time_ms,extra\n0,1000,{i}\n1,1000,{i}\n2,1000,{i}\n3,1000,{i}\n4,1000,{i}\n')
            self.manifest['markers']['capture_end_seconds'] = 3 + i * 0.015
            self.manifest['markers']['scenario_end_seconds'] = 102 + i * 0.02
            self.run_analysis(f'out{i}')
            results.append(str(self.root / f'out{i}' / 'run.json'))
        summary = b.report(results)
        self.assertEqual(summary['metrics']['p99_frame_time_ms']['runs'], [1000, 1000, 1000])
        self.assertAlmostEqual(summary['observed_durations_seconds'][1]['capture'], 2.015)
        self.assertAlmostEqual(summary['observed_durations_seconds'][1]['scenario'], 2.02)
        with self.assertRaises(ValueError):
            b.report([results[0]] * 3)
        run = json.loads(Path(results[1]).read_text())
        run['manifest']['markers']['capture_end_seconds'] = 3.015
        run['manifest']['markers']['scenario_end_seconds'] = 102.02
        Path(results[1]).write_text(json.dumps(run))
        self.assertEqual(len(b.report(results)['run_files']), 3)
        run['manifest']['metadata']['planned_duration_seconds'] = 60
        Path(results[1]).write_text(json.dumps(run))
        with self.assertRaisesRegex(ValueError, 'metadata'):
            b.report(results)
        run['manifest']['metadata']['planned_duration_seconds'] = 2
        run['manifest']['metadata']['game_build'] = 'different'
        Path(results[1]).write_text(json.dumps(run))
        with self.assertRaises(ValueError):
            b.report(results)


if __name__ == '__main__':
    unittest.main()
