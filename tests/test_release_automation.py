"""Release gates: no network, credentials, or registry writes."""

import importlib
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts" / "release"))
SHA = "a" * 40
TAG = "v1.2.3"
DIGEST = "sha256:" + "b" * 64


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue((ROOT / "scripts/release/common.py").is_file(),
                        "release validation module must exist")
        self.common = importlib.import_module("common")

    def release(self):
        return {"tag_name": TAG, "draft": False, "prerelease": False,
                "body": "Approved notes", "published_at": "2026-11-02T12:00:00Z",
                "assets": [{"name": name} for name in self.common.asset_names(TAG)]}

    def test_complete_release_required(self):
        self.common.validate_release(self.release(), TAG)
        for field, value in (("draft", True), ("prerelease", True),
                             ("tag_name", "v9.9.9"), ("assets", []),
                             ("body", "> [!WARNING]\nnotarization incomplete")):
            with self.subTest(field=field):
                release = self.release()
                release[field] = value
                with self.assertRaises(self.common.ReleaseError):
                    self.common.validate_release(release, TAG)

    def test_version_is_numeric_and_strict(self):
        self.assertGreater(self.common.version("v1.10.0"), self.common.version("v1.9.9"))
        for tag in ("main", "1.2.3", "v1.2.3-rc1", "v1.2.3\n", "v01.2.3"):
            with self.subTest(tag=tag), self.assertRaises(self.common.ReleaseError):
                self.common.version(tag)

    def test_latest_ignores_drafts_and_prereleases_and_checks_all_pages(self):
        releases = [{"tag_name": "v1.2.3", "draft": False, "prerelease": False},
                    {"tag_name": "v9.0.0", "draft": True, "prerelease": False}]
        with patch.object(self.common, "pages", return_value=iter(releases)):
            self.common.require_latest(TAG)
        releases.append({"tag_name": "v1.10.0", "draft": False, "prerelease": False})
        with patch.object(self.common, "pages", return_value=iter(releases)):
            with self.assertRaisesRegex(self.common.ReleaseError, "latest"):
                self.common.require_latest(TAG)

    def test_workflow_requires_exact_commit_branch_and_latest_run_success(self):
        run = {"id": 1, "head_sha": SHA, "head_branch": TAG,
               "event": "push", "status": "completed", "conclusion": "success"}
        with patch.object(self.common, "pages", return_value=iter([run])):
            self.assertEqual(self.common.require_workflow("release.yml", SHA, TAG), 1)
        for change in ({"head_sha": "c" * 40}, {"head_branch": "v1.2.4"},
                       {"event": "workflow_dispatch"}, {"status": "in_progress"},
                       {"conclusion": "failure"}):
            with self.subTest(change=change):
                with patch.object(self.common, "pages", return_value=iter([{**run, **change}])):
                    with self.assertRaises(self.common.ReleaseError):
                        self.common.require_workflow("release.yml", SHA, TAG)
        with patch.object(self.common, "pages", return_value=iter([
                run, {**run, "id": 2, "conclusion": "failure"}])):
            with self.assertRaises(self.common.ReleaseError):
                self.common.require_workflow("release.yml", SHA, TAG)

    def test_annotated_public_tag_and_expected_commit(self):
        responses = [{"object": {"type": "tag", "sha": "c" * 40}},
                     {"tag": TAG, "object": {"type": "commit", "sha": SHA}}]
        with patch.object(self.common, "api", side_effect=responses):
            self.assertEqual(self.common.public_commit(TAG, SHA), SHA)
        with patch.object(self.common, "api", side_effect=responses):
            with self.assertRaises(self.common.ReleaseError):
                self.common.public_commit(TAG, "d" * 40)
        with patch.object(self.common, "api", return_value={"object": {"type": "commit", "sha": SHA}}):
            with self.assertRaises(self.common.ReleaseError):
                self.common.public_commit(TAG)

    def test_docker_requires_both_platforms_and_matching_labels(self):
        manifest = {"manifests": [
            {"digest": "sha256:" + arch[0] * 64,
             "platform": {"os": "linux", "architecture": arch}}
            for arch in ("amd64", "arm64")]}
        labels = json.dumps({"org.opencontainers.image.version": "1.2.3",
                             "org.opencontainers.image.revision": SHA})
        with patch.object(self.common, "command", side_effect=[json.dumps(manifest), DIGEST, labels, labels, DIGEST]):
            self.assertEqual(self.common.docker_release(TAG, SHA), DIGEST)
        with patch.object(self.common, "command", side_effect=[json.dumps(manifest), DIGEST, labels, labels, "sha256:" + "c" * 64]):
            with self.assertRaisesRegex(self.common.ReleaseError, "latest"):
                self.common.docker_release(TAG, SHA)
        with patch.object(self.common, "command", side_effect=[json.dumps(manifest), DIGEST, "{}"]):
            with self.assertRaises(self.common.ReleaseError):
                self.common.docker_release(TAG, SHA)
        manifest["manifests"].pop()
        with patch.object(self.common, "command", return_value=json.dumps(manifest)):
            with self.assertRaises(self.common.ReleaseError):
                self.common.docker_release(TAG, SHA)

    def test_pages_follow_pagination(self):
        with patch.object(self.common, "api", side_effect=[list(range(100)), [100]]) as api:
            self.assertEqual(list(self.common.pages("releases")), list(range(101)))
            self.assertIn("page=2", api.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
