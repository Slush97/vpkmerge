"""Check preparation boundaries using temporary files only."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / 'runtime/prepare_csdk13.py'


class PrepareTests(unittest.TestCase):
    def test_idempotent_and_rejects_symlinked_game(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            stage, retail = root / 'stage', root / 'retail'
            for game in (stage / 'game', retail):
                for mod in ('citadel', 'core'):
                    (game / mod).mkdir(parents=True)
                (game / 'citadel/steam.inf').write_text('ClientVersion=1\n')
            compiler = stage / 'game/bin/win64/resourcecompiler.exe'
            compiler.parent.mkdir(parents=True)
            compiler.write_text('fixture')
            original = stage / 'game/citadel/readonly_tools_asset_info.binOFF'
            original.write_bytes(b'original')
            asset = retail / 'citadel/pak01_dir.vpk'
            asset.write_bytes(b'retail')
            cmd = [sys.executable, str(SCRIPT), '--stage', str(stage), '--retail-game', str(retail),
                   '--acknowledge-isolated-stage']
            for _ in range(2):
                self.assertEqual(subprocess.run(cmd, capture_output=True).returncode, 0)
            link = stage / 'game/citadel/pak01_dir.vpk'
            self.assertTrue(link.is_symlink())
            self.assertEqual(link.resolve(), asset)
            self.assertEqual(original.read_bytes(), b'original')
            self.assertEqual((stage / 'game/citadel/readonly_tools_asset_info.bin').read_bytes(), b'original')
            bad_stage = root / 'bad-stage'
            bad_stage.mkdir()
            (bad_stage / 'game').symlink_to(retail, target_is_directory=True)
            cmd[cmd.index(str(stage))] = str(bad_stage)
            result = subprocess.run(cmd, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('must not be a symlink', result.stderr)
            self.assertEqual(asset.read_bytes(), b'retail')
            self.assertFalse((retail / 'citadel/readonly_tools_asset_info.bin').exists())


if __name__ == '__main__':
    unittest.main()
