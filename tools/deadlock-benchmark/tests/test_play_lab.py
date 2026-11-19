import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'runtime'))

SPEC = importlib.util.spec_from_file_location('play_lab', Path(__file__).resolve().parents[1] / 'runtime/play_lab.py')
lab = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(lab)


class DisplayTests(unittest.TestCase):
    def test_borderless_override_and_next_run_restore_retail_settings(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            retail, isolated, run = root/'retail', root/'isolated', root/'run'
            source = retail/'game/citadel/cfg/video.txt'
            source.parent.mkdir(parents=True)
            run.mkdir()
            original = '"video.cfg"\n{\n"setting.defaultres" "2560"\n"setting.coop_fullscreen" "1"\n"setting.mat_viewportscale" "0.75"\n}\n'
            source.write_text(original)
            flags = lab.configure_video(isolated, retail, run, '1920x1080')
            self.assertEqual(flags, ['-windowed', '-w', '1920', '-h', '1080'])
            settings = json.loads((run/'display.json').read_text())['settings']
            self.assertEqual(settings['setting.coop_fullscreen'], '0')
            self.assertEqual(settings['setting.nowindowborder'], '0')
            self.assertEqual(settings['setting.defaultresheight'], '1080')
            self.assertEqual(settings['setting.mat_viewportscale'], '0.75')
            self.assertEqual(source.read_text(), original)
            self.assertEqual(lab.configure_video(isolated, retail, run, 'game'), [])
            self.assertEqual((isolated/'cfg/video.txt').read_text(), original)

    def test_refuses_video_link_into_retail(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root/'retail/game/citadel/cfg/video.txt'
            source.parent.mkdir(parents=True)
            source.write_text('original')
            isolated = root/'isolated'
            (isolated/'cfg').mkdir(parents=True)
            (isolated/'cfg/video.txt').symlink_to(source)
            with self.assertRaisesRegex(RuntimeError, 'symlink'):
                lab.configure_video(isolated, root/'retail', root, '1920x1080')
            self.assertEqual(source.read_text(), 'original')


if __name__ == '__main__':
    unittest.main()
