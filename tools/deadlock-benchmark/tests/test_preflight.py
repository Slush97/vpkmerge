import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'runtime' / 'preflight.py'
spec = importlib.util.spec_from_file_location('preflight', SCRIPT)
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.game = self.root / 'game-root'
        self.stage = self.root / 'stage'
        self.write(self.game / 'game/citadel/steam.inf', 'ClientVersion=6860\nSourceRevision=11211860\n')
        for name in p.CSDK13_FILES:
            self.write(self.stage / name, 'fixture ' + name)
        self.write(self.stage / 'read_me.txt', 'Reduced CSDK 13 fixture')
        self.write(self.stage / 'game/citadel/steam.inf', 'ClientVersion=6836\nSourceRevision=11193655\n')

    def write(self, path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def check(self, *args):
        run = subprocess.run([sys.executable, str(SCRIPT), '--game', str(self.game),
                              '--stage', str(self.stage), *args], capture_output=True, text=True)
        return run.returncode, json.loads(run.stdout)

    def test_csdk13_complete_but_version_mismatch_is_not_compatibility(self):
        code, data = self.check('--profile', 'csdk13')
        self.assertEqual(code, 0)
        self.assertFalse(data['versions_match'])
        self.assertTrue(data['compiler_inventory']['complete'])
        self.assertEqual(data['compiler_output_compatibility'], 'unverified')
        self.assertEqual(data['runtime_compatibility'], 'unverified')
        self.assertNotEqual(data['compiler_inventory']['sha256']['game/bin/win64/resourcecompiler.exe'],
                            data['compiler_inventory']['sha256']['game/bin/win64/resourcecompiler.dll'])

    def test_csdk13_missing_compiler_dll_or_marker_fails(self):
        (self.stage / 'game/bin/win64/resourcecompiler.dll').unlink()
        code, data = self.check('--profile', 'csdk13')
        self.assertEqual(code, 1)
        self.assertIn('game/bin/win64/resourcecompiler.dll', data['compiler_inventory']['missing'])
        self.write(self.stage / 'read_me.txt', 'not a package marker')
        code, data = self.check('--profile', 'csdk13')
        self.assertEqual(code, 1)
        self.assertFalse(data['compiler_inventory']['package_marker_present'])

    def test_default_legacy_rejects_mismatch_and_missing_compiler(self):
        self.assertEqual(self.check()[0], 1)
        self.write(self.stage / 'game/citadel/steam.inf', 'ClientVersion=6860\nSourceRevision=11211860\n')
        self.assertEqual(self.check()[0], 0)
        (self.stage / 'game/bin/win64/resourcecompiler.exe').unlink()
        self.assertEqual(self.check()[0], 1)


if __name__ == '__main__':
    unittest.main()
