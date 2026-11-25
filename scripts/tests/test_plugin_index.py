import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[2]

class PluginIndex(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=os.environ['TMPDIR'])
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        (self.root / 'plugins').mkdir()
        (self.root / 'crates/hoh-authored-plugins/src').mkdir(parents=True)
        shutil.copyfile(SOURCE / 'scripts/generate-plugin-index.py', self.root / 'scripts/generate-plugin-index.py')
        (self.root / 'crates/hoh-authored-plugins/Cargo.toml').write_text('[dependencies]\n# BEGIN GENERATED PLUGINS\n# END GENERATED PLUGINS\n')

    def tearDown(self):
        self.temp.cleanup()

    def plugin(self, name):
        path = self.root / 'plugins' / name
        path.mkdir()
        (path / 'hoh-plugin.json').write_text(json.dumps({'id': name, 'entry': 'AuthorPlugin'}))
        (path / 'Cargo.toml').write_text(f'[package]\nname = "hoh-plugin-{name}"\n')
        return path

    def generate(self):
        return subprocess.run(['python3', str(self.root / 'scripts/generate-plugin-index.py')], capture_output=True, text=True)

    def test_discovers_all_plugins_and_is_idempotent(self):
        self.plugin('second')
        self.plugin('first')
        self.assertEqual(self.generate().returncode, 0)
        index = self.root / 'crates/hoh-authored-plugins/src/generated.rs'
        before = index.read_text()
        self.assertLess(before.index('plugin-first'), before.index('plugin-second'))
        self.assertIn('plugin_0::AuthorPlugin', before)
        self.assertEqual(self.generate().returncode, 0)
        self.assertEqual(before, index.read_text())

    def test_rejects_untrusted_paths_and_mismatched_ids_before_writing(self):
        path = self.plugin('actual')
        (path / 'hoh-plugin.json').write_text('{"id":"../escape","entry":"AuthorPlugin"}')
        self.assertNotEqual(self.generate().returncode, 0)
        self.assertFalse((self.root / 'crates/hoh-authored-plugins/src/generated.rs').exists())

if __name__ == '__main__':
    unittest.main()
