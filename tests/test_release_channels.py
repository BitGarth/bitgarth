import importlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/release"))
NOTES = """# BitGarth 1.2.3

## Highlights

- The *Wallets* page now shows A & B.
  The wording stays unchanged.

## Fixed

- Use `--help` and **read** [the guide](https://example.com/guide).
"""
LANDING = '''<main><div class="changelog">
          <article class="release" id="1.2.2">
            <span class="latest-tag">Latest</span>
            <p>Previous release stays here.</p>
          </article>
</div></main>'''
UMBREL = '''id: ferntrail-bitgarth
version: "1.2.2"
gallery:
  - gallery/1.jpg
releaseNotes: |
  Old wording.

  Full release notes at https://bitgarth.app/releases.html#1.2.2
submitter: FernTrail B.V.
'''
COMPOSE = '''services:
  web:
    image: bitgarth/bitgarth:1.2.2@sha256:OLD
    environment:
      BITGARTH_CHANNEL: umbrel
'''


class ChannelTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue((ROOT / "scripts/release/prepare_channels.py").is_file(),
                        "channel generator must exist")
        self.module = importlib.import_module("prepare_channels")

    def test_notes_preserve_every_item_and_reject_unrecognized_blocks(self):
        sections = self.module.parse_notes(NOTES, "1.2.3")
        self.assertEqual(sections[0], ("Highlights", [
            "The *Wallets* page now shows A & B. The wording stays unchanged."]))
        for notes in (NOTES.replace("1.2.3", "1.2.4"), NOTES + "\n## Surprise\n- Text\n",
                      NOTES + "\nA paragraph outside a bullet.\n"):
            with self.assertRaises(self.module.ReleaseError):
                self.module.parse_notes(notes, "1.2.3")

    def test_landing_escapes_text_and_is_idempotent(self):
        sections = self.module.parse_notes(NOTES, "1.2.3")
        result = self.module.render_landing(LANDING, sections, "1.2.3", "2026-11-02")
        self.assertIn("<em>Wallets</em>", result)
        self.assertIn("A &amp; B. The wording stays unchanged.", result)
        self.assertIn('<a href="https://example.com/guide">the guide</a>', result)
        self.assertIn("Previous release stays here.", result)
        self.assertEqual(result.count('class="latest-tag"'), 1)
        self.assertEqual(result, self.module.render_landing(result, sections, "1.2.3", "2026-11-02"))
        self.assertEqual(self.module.inline_html('<script>alert("x")</script>'),
                         '&lt;script&gt;alert("x")&lt;/script&gt;')
        with self.assertRaises(self.module.ReleaseError):
            self.module.render_landing(result, sections, "1.2.2", "2026-11-02")

    def test_umbrel_preserves_metadata_and_full_notes_link(self):
        sections = self.module.parse_notes(NOTES, "1.2.3")
        result = self.module.render_umbrel(UMBREL, sections, "1.2.3")
        self.assertIn('version: "1.2.3"', result)
        self.assertIn("  - The *Wallets* page now shows A & B. The wording stays unchanged.", result)
        self.assertIn("  Fixed:\n\n", result)
        self.assertNotIn("Highlights:", result)
        self.assertIn("releases.html#1.2.3", result)
        self.assertIn("gallery:\n  - gallery/1.jpg", result)
        self.assertTrue(result.endswith("submitter: FernTrail B.V.\n"))
        self.assertEqual(result, self.module.render_umbrel(result, sections, "1.2.3"))

    def test_bad_layout_leaves_all_files_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            landing = root / "landing"
            umbrel = root / "umbrel"
            landing.mkdir()
            umbrel.mkdir()
            files = {landing / "releases.html": LANDING,
                     umbrel / "umbrel-app.yml": UMBREL,
                     umbrel / "docker-compose.yml": "unexpected layout\n"}
            for path, text in files.items():
                path.write_text(text)
            with patch.object(self.module, "binary_release", return_value=("a" * 40, {"published_at": "2026-11-02T12:00:00Z"})), \
                    patch.object(self.module, "require_latest"), \
                    patch.object(self.module, "source_file", return_value=NOTES.encode()), \
                    patch.object(self.module, "docker_release", return_value="sha256:" + "b" * 64):
                with self.assertRaises(self.module.ReleaseError):
                    self.module.prepare("v1.2.3", landing, umbrel)
            for path, text in files.items():
                self.assertEqual(path.read_text(), text)

    def test_generate_valid_files_then_rerun_without_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            landing, umbrel = root / "landing", root / "umbrel"
            landing.mkdir()
            umbrel.mkdir()
            files = {landing / "releases.html": LANDING,
                     umbrel / "umbrel-app.yml": UMBREL,
                     umbrel / "docker-compose.yml": COMPOSE}
            for path, text in files.items():
                path.write_text(text)
            with patch.object(self.module, "binary_release", return_value=("a" * 40, {"published_at": "2026-11-02T12:00:00Z"})), \
                    patch.object(self.module, "require_latest"), \
                    patch.object(self.module, "source_file", return_value=NOTES.encode()), \
                    patch.object(self.module, "docker_release", return_value="sha256:" + "b" * 64):
                self.module.prepare("v1.2.3", landing, umbrel)
                first = {path: path.read_text() for path in files}
                self.module.prepare("v1.2.3", landing, umbrel)
            self.assertEqual(first, {path: path.read_text() for path in files})
            self.assertIn("1.2.3@sha256:" + "b" * 64, first[umbrel / "docker-compose.yml"])


if __name__ == "__main__":
    unittest.main()
