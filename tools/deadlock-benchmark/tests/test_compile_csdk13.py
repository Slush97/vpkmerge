"""Failure-path tests using a fake compiler; no game or Proton is started."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'runtime/compile_csdk13.py'


class CompileFailureTests(unittest.TestCase):
    def test_failed_or_missing_output_is_rejected(self):
        for summary in ('0 compiled, 1 failed', '9 compiled, 0 failed'):
            with self.subTest(summary=summary), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                stage, sources, steam = (root / x for x in ('stage', 'sources', 'steam'))
                binary = stage / 'game/bin/win64/resourcecompiler.exe'
                binary.parent.mkdir(parents=True)
                binary.write_text('fixture')
                citadel = stage / 'game/citadel'
                citadel.mkdir()
                (citadel / 'steam.inf').write_text('ClientVersion=1\n')
                (citadel / 'readonly_tools_asset_info.bin').write_text('fixture')
                sources.mkdir()
                for name in ('dl_benchmark_lab_0.vmap', 'dl_benchmark_lab_16.vmap'):
                    (sources / name).write_text('fixture')
                steam.mkdir()
                proton = root / 'fake-proton'
                proton.write_text('#!/usr/bin/env python3\nimport pathlib,sys\n'
                                  'p=sys.argv[sys.argv.index("-con_logfile")+1]\n'
                                  'p=pathlib.Path(p[2:].replace("\\\\","/"))\n'
                                  f'p.write_text({summary!r})\n'
                                  'addon=sys.argv[sys.argv.index("-addon")+1]\n'
                                  'out=pathlib.Path.cwd().parents[1]/"citadel_addons"/addon/"maps/dl_benchmark_lab_0.vpk"\n'
                                  + ('out.write_bytes(b"stale-looking-output")\n' if summary.startswith('0 compiled') else ''))
                proton.chmod(0o755)
                results = root / 'results'
                cmd = [sys.executable, str(SCRIPT), '--stage', str(stage), '--sources', str(sources),
                       '--proton', str(proton), '--steam', str(steam), '--results', str(results),
                       '--acknowledge-isolated-stage']
                run = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
                self.assertNotEqual(run.returncode, 0)
                report = json.loads((results / 'manifest.json').read_text())
                self.assertEqual(len(report['runs']), 1)
                self.assertFalse(report['runs'][0]['success'])
                if summary.startswith('0 compiled'):
                    self.assertIsNotNone(report['runs'][0]['output_sha256'])
                else:
                    self.assertIsNone(report['runs'][0]['output_sha256'])
                self.assertTrue(Path(report['runs'][0]['log']).is_file())


if __name__ == '__main__':
    unittest.main()
