import hashlib
import importlib
import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/release"))


def formula(product):
    return f'''class Bitgarth{product.title()} < Formula
  if OS.mac?
    url "https://github.com/BitGarth/bitgarth/releases/download/v1.2.2/bitgarth-{product}-v1.2.2-macos-aarch64.tar.gz"
    sha256 "{'a' * 64}"
  else
    url "https://github.com/BitGarth/bitgarth/releases/download/v1.2.2/bitgarth-{product}-v1.2.2-linux-x86_64-gnu.tar.gz"
    sha256 "{'b' * 64}"
  end
  resource "license" do
    url "https://raw.githubusercontent.com/BitGarth/bitgarth/v1.2.2/LICENSE.md"
    sha256 "{'c' * 64}"
  end
  # Keep launchers and services byte-for-byte.
  ENV["BITGARTH_CHANNEL"] = "homebrew"
end
'''


class HomebrewTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue((ROOT / "scripts/release/prepare_homebrew.py").is_file(),
                        "Homebrew generator must exist")
        self.module = importlib.import_module("prepare_homebrew")

    def seed_download(self, tag, directory):
        checksums = []
        for name in self.module.archive_names(tag):
            data = name.encode()
            (directory / name).write_bytes(data)
            checksums.append(f"{hashlib.sha256(data).hexdigest()}  {name}\n")
        (directory / "SHA256SUMS").write_text("".join(checksums))

    def test_updates_both_platforms_license_and_preserves_other_content(self):
        hashes = {name: "d" * 64 for name in self.module.archive_names("v1.2.3")}
        for product in ("cli", "web"):
            result = self.module.render_formula(formula(product), product, "v1.2.3", hashes, "e" * 64)
            self.assertEqual(result.count("/download/v1.2.3/"), 2)
            self.assertIn("v1.2.3/LICENSE.md", result)
            self.assertIn('sha256 "' + "e" * 64 + '"', result)
            self.assertEqual(formula(product).split("  # Keep")[1], result.split("  # Keep")[1])
            self.assertEqual(result, self.module.render_formula(result, product, "v1.2.3", hashes, "e" * 64))

    def test_checksums_reject_corruption_missing_and_duplicate_entries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.seed_download("v1.2.3", root)
            names = self.module.archive_names("v1.2.3")
            self.assertEqual(len(self.module.verify_archives(root, names)), 4)
            (root / names[0]).write_bytes(b"corrupt")
            with self.assertRaises(self.module.ReleaseError):
                self.module.verify_archives(root, names)
            self.seed_download("v1.2.3", root)
            sums = root / "SHA256SUMS"
            sums.write_text(sums.read_text() * 2)
            with self.assertRaises(self.module.ReleaseError):
                self.module.verify_archives(root, names)
            sums.write_text("")
            with self.assertRaises(self.module.ReleaseError):
                self.module.verify_archives(root, names)

    def test_bad_archive_leaves_both_formulae_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            tap = Path(directory)
            (tap / "Formula").mkdir()
            for product in ("cli", "web"):
                (tap / f"Formula/bitgarth-{product}.rb").write_text(formula(product))

            def corrupt(tag, directory):
                self.seed_download(tag, directory)
                (directory / self.module.archive_names(tag)[-1]).write_bytes(b"corrupt")

            with patch.object(self.module, "binary_release", return_value=("a" * 40, {})), \
                    patch.object(self.module, "require_latest"), \
                    patch.object(self.module, "download_archives", side_effect=corrupt), \
                    patch.object(self.module, "source_file", return_value=b"license"):
                with self.assertRaises(self.module.ReleaseError):
                    self.module.prepare("v1.2.3", tap)
            for product in ("cli", "web"):
                self.assertEqual((tap / f"Formula/bitgarth-{product}.rb").read_text(), formula(product))

    def test_successful_generation_and_rerun(self):
        with tempfile.TemporaryDirectory() as directory:
            tap = Path(directory)
            (tap / "Formula").mkdir()
            paths = [tap / f"Formula/bitgarth-{product}.rb" for product in ("cli", "web")]
            for path, product in zip(paths, ("cli", "web")):
                path.write_text(formula(product))
            with patch.object(self.module, "binary_release", return_value=("a" * 40, {})), \
                    patch.object(self.module, "require_latest"), \
                    patch.object(self.module, "download_archives", side_effect=self.seed_download), \
                    patch.object(self.module, "source_file", return_value=b"license"), \
                    patch("sys.stdout", io.StringIO()):
                self.module.prepare("v1.2.3", tap)
                first = [path.read_text() for path in paths]
                self.module.prepare("v1.2.3", tap)
            self.assertEqual(first, [path.read_text() for path in paths])
            self.assertTrue(all("v1.2.2" not in text for text in first))


if __name__ == "__main__":
    unittest.main()
