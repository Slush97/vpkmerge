import csv
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

RUNTIME = Path(__file__).resolve().parents[1]/'runtime'
sys.path.insert(0, str(RUNTIME))
from benchmark_ledger import Ledger, config_diff, date_filter, import_session
from benchmark_capture import Capture, normalize_mangohud, probe
from benchmark_recording import recover


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.run = self.root/'session'
        self.run.mkdir()
        self.db = self.root/'ledger.sqlite3'
        self.ledger = Ledger(self.db)
        self.addCleanup(self.ledger.close)

    def test_configuration_snapshot_notes_and_artifact_integrity(self):
        config = {'resolution': [1920, 1080], 'scale': .75}
        run = self.ledger.create(self.run, config, suite='A-B', label='baseline')
        config['scale'] = 1
        file = self.run/'video.txt'
        file.write_text('video settings')
        self.ledger.artifact(run, file, 'configuration')
        self.ledger.event(run, 'scenario_marker', {'pass_index': 1, 'alignment_verified': False})
        self.ledger.finish(run, 'completed', {'status': 'captured'})
        before = self.ledger.export(run)
        self.ledger.note(run, 'Boundary needs validation', pass_index=1)
        after = self.ledger.export(run)
        self.assertEqual(after['configuration']['scale'], .75)
        self.assertEqual(before['events'], after['events'][:-1])
        self.assertEqual(after['events'][-1]['payload']['scope'], 'pass')
        self.assertIsNone(after['events'][-1]['monotonic_ns'])
        self.assertFalse(after['result']['measurement_valid'])
        self.assertFalse(after['result']['comparison_eligible'])
        self.assertTrue(self.ledger.verify(run)['integrity_ok'])
        file.write_text('changed')
        self.assertFalse(self.ledger.verify(run)['integrity_ok'])
        file.unlink()
        self.assertIn('missing artifact', self.ledger.verify(run)['errors'][0])
        with self.assertRaisesRegex(ValueError, 'finished'):
            self.ledger.finish(run, 'completed', {})
        with self.assertRaisesRegex(ValueError, 'finished'):
            self.ledger.event(run, 'phase', {})

    def test_config_deduplication_date_queries_and_diff(self):
        first = self.ledger.create(self.run, {'display': {'scale': .75}}, 'suite', recorded_utc='2026-11-07T19:00:00-08:00')
        other = self.root/'other'; other.mkdir()
        second = self.ledger.create(other, {'display': {'scale': .75}}, 'suite')
        self.assertEqual(self.ledger.export(first)['config_hash'], self.ledger.export(second)['config_hash'])
        rows = self.ledger.list(since='2026-11-08T02:00:00Z', until='2026-11-08T03:01:00Z', suite='suite')
        self.assertIn(first, [row['id'] for row in rows])
        self.assertEqual(config_diff({'display': {'scale': .75}}, {'display': {'scale': 1}})[0]['field'], 'display.scale')
        with self.assertRaises(ValueError):
            date_filter('2026-11-07T19:00:00')
        with self.assertRaises(sqlite3.IntegrityError):
            self.ledger.create(self.run, {})

    def test_interrupted_recovery_does_not_fabricate_capture(self):
        run_id = self.ledger.create(self.run, {})
        (self.run/'server-launch.log').write_text('partial')
        recover({'logs': str(self.run), 'ledger_run_id': run_id})
        result = self.ledger.export(run_id)
        self.assertEqual(result['result']['outcome'], 'interrupted')
        self.assertEqual(result['result']['capture']['status'], 'unverified')
        recover({'logs': str(self.run), 'ledger_run_id': run_id})
        self.assertEqual(result['events'], self.ledger.export(run_id)['events'])

    def test_import_retains_unknown_historical_hardware_and_time(self):
        (self.run/'display.json').write_text('{"requested":"game"}')
        (self.run/'result.json').write_text('{"status":"completed","score_eligible":false}')
        run_id = import_session(self.ledger, self.run, 'historical', 'previous run')
        result = self.ledger.export(run_id)
        self.assertIsNone(result['recorded_utc'])
        self.assertIsNone(result['configuration']['hardware'])
        self.assertEqual(result['result']['outcome'], 'imported')
        self.assertTrue(self.ledger.verify(run_id)['integrity_ok'])

    def test_artifact_cannot_escape_session_and_bad_payload_is_atomic(self):
        run_id = self.ledger.create(self.run, {})
        outside = self.root/'outside'; outside.write_text('not part of run')
        (self.run/'link').symlink_to(outside)
        with self.assertRaises(ValueError):
            self.ledger.artifact(run_id, self.run/'link')
        before = self.ledger.export(run_id)['events']
        with self.assertRaises(ValueError):
            self.ledger.event(run_id, 'phase', {'value': float('nan')})
        self.assertEqual(before, self.ledger.export(run_id)['events'])

    def test_cli_export_note_and_verify(self):
        run_id = self.ledger.create(self.run, {'resolution': '1920x1080'})
        prefix = [sys.executable, str(RUNTIME/'benchmark_ledger.py'), '--db', str(self.db)]
        note = subprocess.run([*prefix, 'note', run_id, 'Tested the launch', '--pass', '2'], capture_output=True, text=True)
        self.assertEqual(note.returncode, 0, note.stderr)
        output = self.root/'export.json'
        export = subprocess.run([*prefix, 'export', run_id, '--output', str(output)], capture_output=True, text=True)
        self.assertEqual(export.returncode, 0, export.stderr)
        self.assertEqual(json.loads(output.read_text())['events'][-1]['payload']['pass_index'], 2)
        retry = subprocess.run([*prefix, 'export', run_id, '--output', str(output)], capture_output=True, text=True)
        self.assertEqual(retry.returncode, 2)


class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def sample(self, name='deadlock.exe_test.csv'):
        path = self.root/name
        path.write_text('os,cpu,gpu\nLinux,CPU,GPU\nfps,frametime,cpu_load,gpu_load,elapsed\n'
                        '100,10,20,70,10000000\n100,10,20,70,20000000\n50,20,21,75,40000000\n')
        return path

    def test_preserves_raw_normalizes_frames_and_labels_whole_capture_statistics(self):
        source = self.sample()
        raw = source.read_bytes()
        output = self.root/'normalized'; output.mkdir()
        result = normalize_mangohud(source, output)
        self.assertEqual(source.read_bytes(), raw)
        self.assertEqual(result['frame_count'], 3)
        self.assertAlmostEqual(result['diagnostic_metrics']['average_fps'], 2/.03)
        self.assertEqual(result['diagnostic_metrics']['p99_frame_time_ms'], 20)
        self.assertIn('INCLUDING loading', result['metric_scope'])
        self.assertFalse(result['measurement_valid'])
        with (output/'telemetry.csv').open() as stream:
            rows = list(csv.DictReader(stream))
        self.assertEqual(rows[-1]['gpu_load'], '75')

    def test_corrupt_or_missing_frames_are_rejected_without_deleting_raw(self):
        for last in ('100,10,20,70,40000000', '100,nan,20,70,20000000', 'partial'):
            source = self.sample()
            source.write_text('\n'.join(source.read_text().splitlines()[:4])+'\n'+last+'\n')
            with self.assertRaises(ValueError):
                normalize_mangohud(source, self.root)
            self.assertTrue(source.is_file())
            self.assertFalse((self.root/'frames.csv').exists())

    def test_platform_stubs_and_explicit_backend_failure(self):
        for system in ('Windows', 'Darwin'):
            plan = probe('auto', system)
            self.assertFalse(plan['implemented'])
            self.assertFalse(plan['available'])
        with patch('benchmark_capture.platform.system', return_value='Windows'):
            with self.assertRaisesRegex(ValueError, 'scaffold'):
                Capture(self.root, 'presentmon')
        with patch('benchmark_capture.shutil.which', return_value=None):
            self.assertFalse(probe('mangohud', 'Linux')['available'])

    def test_private_config_and_missing_stream_do_not_claim_success(self):
        plan = {'backend': 'mangohud', 'available': True, 'reason': None}
        with patch('benchmark_capture.probe', return_value=plan):
            capture = Capture(self.root, 'mangohud')
        env = capture.environment({'MANGOHUD_CONFIG': 'upload_logs,fps_limit=30'})
        self.assertNotIn('MANGOHUD_CONFIG', env)
        self.assertEqual(env['MANGOHUD'], '1')
        self.assertIn('permit_upload=0', Path(env['MANGOHUD_CONFIGFILE']).read_text())
        self.assertEqual(capture.finish()['status'], 'failed')


if __name__ == '__main__':
    unittest.main()
