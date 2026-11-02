import importlib
import io
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/release"))


class StatusTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue((ROOT / "scripts/release/status.py").is_file(), "status command must exist")
        self.status = importlib.import_module("status")

    def test_full_status_reports_snap_deferred_without_querying_store(self):
        output = io.StringIO()
        with patch.object(self.status, "binary_release", return_value=("a" * 40, {})), \
                patch.object(self.status, "docker_release", return_value="sha256:" + "b" * 64), \
                patch.object(self.status, "check_channels"), \
                patch.object(self.status, "check_homebrew"), patch("sys.stdout", output):
            self.assertEqual(self.status.main(["v1.2.3"]), 0)
        self.assertIn("DEFERRED Snap", output.getvalue())

    def test_pending_or_failed_evidence_exits_nonzero(self):
        for error, label in ((self.status.Pending("still building"), "PENDING"),
                             (self.status.ReleaseError("bad evidence"), "FAIL")):
            with patch.object(self.status, "binary_release", side_effect=error), \
                    patch("sys.stdout", new_callable=io.StringIO) as output:
                self.assertNotEqual(self.status.main(["v1.2.3", "--stage", "binary"]), 0)
                self.assertIn(label, output.getvalue())

    def test_channel_check_rejects_missing_or_old_channel(self):
        good = {"channels": {name: {"latest": "1.2.3"}
                             for name in ("default", "docker", "umbrel")}}
        with patch.object(self.status, "get_json", return_value=good):
            self.status.check_channels("v1.2.3")
        good["channels"]["docker"]["latest"] = "1.2.2"
        with patch.object(self.status, "get_json", return_value=good):
            with self.assertRaises(self.status.Pending):
                self.status.check_channels("v1.2.3")


if __name__ == "__main__":
    unittest.main()
