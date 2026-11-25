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
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR"))
        self.root = Path(self.temp.name)
        (self.root / 'scripts').mkdir()
        (self.root / 'plugins').mkdir()
        (self.root / 'crates/hoh-authored-plugins/src').mkdir(parents=True)
        (self.root / 'plugins/index.json').write_text('[]')
        shutil.copyfile(SOURCE / 'scripts/generate-plugin-index.py', self.root / 'scripts/generate-plugin-index.py')
        (self.root / 'crates/hoh-authored-plugins/Cargo.toml').write_text('[dependencies]\n# BEGIN GENERATED PLUGINS\n# END GENERATED PLUGINS\n')

    def tearDown(self):
        self.temp.cleanup()

    def plugin(self, name):
        path = self.root / 'plugins' / name
        path.mkdir()
        (path / 'hoh-plugin.json').write_text(json.dumps({'id': name, 'entry': 'AuthorPlugin', 'version': '0.1.0'}))
        (path / 'Cargo.toml').write_text(f'[package]\nname = "hoh-plugin-{name}"\n')
        return path

    def release(self, *names):
        (self.root / 'plugins/index.json').write_text(json.dumps(names))

    def generate(self, *args):
        return subprocess.run(['python3', str(self.root / 'scripts/generate-plugin-index.py'), *args],
                              capture_output=True, text=True)

    def test_release_order_is_deterministic_and_idempotent(self):
        self.plugin('second')
        self.plugin('first')
        self.release('second', 'first')
        self.assertEqual(self.generate().returncode, 0)
        index = self.root / 'crates/hoh-authored-plugins/src/generated.rs'
        before = index.read_text()
        self.assertLess(before.index('plugin-first'), before.index('plugin-second'))
        self.assertIn('plugin_0::AuthorPlugin', before)
        self.assertEqual(self.generate().returncode, 0)
        self.assertEqual(before, index.read_text())

    def test_rejects_untrusted_paths_and_mismatched_ids_before_writing(self):
        path = self.plugin('actual')
        self.release('actual')
        (path / 'hoh-plugin.json').write_text('{"id":"../escape","entry":"AuthorPlugin"}')
        self.assertNotEqual(self.generate().returncode, 0)
        self.assertFalse((self.root / 'crates/hoh-authored-plugins/src/generated.rs').exists())

    def test_only_explicit_candidate_is_previewed_without_writes(self):
        self.plugin('released')
        self.release('released')
        self.plugin('current')
        broken = self.plugin('other-draft')
        (broken / 'Cargo.toml').write_text('broken')
        result = self.generate('--candidate', 'current', '--dry-run', '--json')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['plugins'], ['current', 'released'])
        self.assertFalse((self.root / 'crates/hoh-authored-plugins/src/generated.rs').exists())
        self.assertEqual(self.generate('--candidate', 'current').returncode, 0)
        index = (self.root / 'crates/hoh-authored-plugins/src/generated.rs').read_text()
        self.assertIn('plugin-current', index)
        self.assertNotIn('other-draft', index)
        self.assertEqual(self.generate().returncode, 0)
        self.assertNotIn('plugin-current', (self.root / 'crates/hoh-authored-plugins/src/generated.rs').read_text())

    def test_check_and_index_agree_on_invalid_package_names(self):
        plugin = self.plugin('candidate')
        (plugin / 'Cargo.toml').write_text('[package]\nname = "wrong"\n')
        for operation in ['--check', '--candidate']:
            result = self.generate(operation, 'candidate', '--json')
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(json.loads(result.stdout)['issues'][0]['code'], 'cargo_package_name')

    def test_reserved_id_cannot_be_written_to_the_index(self):
        self.plugin('inventory')
        self.release('inventory')
        result = self.generate('--json')
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout)['issues'][0]['code'], 'reserved_id')
        self.assertFalse((self.root / 'crates/hoh-authored-plugins/src/generated.rs').exists())

if __name__ == '__main__':
    unittest.main()
