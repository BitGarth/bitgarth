"""Contract checks for the release-to-snap packaging boundary."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import hashlib
import importlib.util
import io
import json
from http.server import BaseHTTPRequestHandler, HTTPServer
import shutil
import tarfile
import threading


ROOT = Path(__file__).resolve().parents[1]
SNAP = ROOT / "packaging/snap"
BUILD_PATH = ROOT / "scripts/snap/build.py"
VERIFY_PATH = ROOT / "scripts/snap/verify_manifest.py"
WORKFLOW_PATH = ROOT / ".github/workflows/snap-release.yml"


def load_build():
    spec = importlib.util.spec_from_file_location("snap_build", BUILD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_installed():
    path = ROOT / "tests/test_snap_installed.py"
    spec = importlib.util.spec_from_file_location("snap_installed", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_verify():
    spec = importlib.util.spec_from_file_location("snap_verify", VERIFY_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def archive(path, files):
    with tarfile.open(path, "w:gz") as package:
        for name, data in files.items():
            item = tarfile.TarInfo(name)
            item.size = len(data)
            item.mode = 0o755 if name.startswith("bitgarth") else 0o644
            package.addfile(item, io.BytesIO(data))


def fake_snapctl_env(root, **values):
    """Put a snapctl stand-in on PATH that answers from FAKE_* variables."""
    bin_dir = root / "fake-bin"
    bin_dir.mkdir()
    snapctl = bin_dir / "snapctl"
    snapctl.write_text(
        "#!/bin/sh\n"
        "case \"$1\" in\n"
        "  get) printf '%s\\n' \"$FAKE_PORT\" ;;\n"
        "  services) printf 'Service Startup Current Notes\\n%s enabled %s -\\n'"
        " \"$2\" \"$FAKE_STATE\" ;;\n"
        "  restart) printf 'restart %s\\n' \"$2\" >> \"$FAKE_LOG\" ;;\n"
        "  *) exit 2 ;;\n"
        "esac\n")
    snapctl.chmod(0o755)
    return {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", **values}


class RecipeTests(unittest.TestCase):
    def test_candidate_publisher_accepts_verified_marker(self):
        workflow = WORKFLOW_PATH.read_text()
        self.assertIn(r"ferntrail(?:\+|\*{1,2})?", workflow)

    def test_cli_recipe_and_launcher(self):
        recipe = (SNAP / "cli/snapcraft.yaml.in").read_text()
        self.assertEqual(recipe.count("@VERSION@"), 1)
        for expected in ("name: bitgarth-cli", "base: core24", "grade: stable",
                         "confinement: strict", "command: bin/launch-cli",
                         "plugs: [network]", "source: payload/", "source: launcher/"):
            self.assertIn(expected, recipe)
        self.assertNotIn("daemon:", recipe)
        self.assertNotIn("network-bind", recipe)
        launcher = SNAP / "cli/launch-cli"
        self.assertTrue(launcher.stat().st_mode & 0o111)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Unlike a #! script, bash reading stdin sees the argv[0] the launcher sets as $0.
            (root / "bitgarth").symlink_to("/bin/bash")
            user = root / "user"
            user.mkdir()
            env = {**os.environ, "SNAP": str(root), "SNAP_USER_COMMON": str(user)}
            result = subprocess.run([str(launcher), "-s", "pair"], env=env,
                                    input='printf "%s\\n" "$XDG_CONFIG_HOME" "$0" "$1"\n',
                                    capture_output=True, text=True, check=True)
            self.assertEqual(result.stdout.splitlines(),
                             [str(user / ".config"), "bitgarth-cli", "pair"])

    def test_web_recipe_and_launcher(self):
        recipe = (SNAP / "web/snapcraft.yaml.in").read_text()
        self.assertEqual(recipe.count("@VERSION@"), 1)
        for expected in ("name: bitgarth-web", "base: core24", "grade: stable",
                         "confinement: strict", "command: bin/launch-web",
                         "daemon: simple", "plugs: [network, network-bind]",
                         "BITGARTH_CHANNEL: snap", "libssl3t64", "source: payload/",
                         "source: launcher/"):
            self.assertIn(expected, recipe)
        launcher = SNAP / "web/launch-web"
        self.assertTrue(launcher.stat().st_mode & 0o111)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bitgarth-web"
            binary.write_text("#!/bin/sh\nprintf '%s\\n' \"$BITGARTH_PROJECT_DIR\" \"$IP\" \"$PORT\" \"$PWD\"\n")
            binary.chmod(0o755)
            common = root / "common"
            common.mkdir()
            env = fake_snapctl_env(root, SNAP=str(root), SNAP_COMMON=str(common))
            for configured, port in (("", "8080"), ("8081", "8081")):
                result = subprocess.run([str(launcher)], env={**env, "FAKE_PORT": configured},
                                        capture_output=True, text=True, check=True)
                self.assertEqual(result.stdout.splitlines(),
                                 [str(common / "bitgarth"), "127.0.0.1", port, str(root)])

    def test_web_configure_hook_validates_port_and_restarts_running_service(self):
        hook = SNAP / "web/configure"
        self.assertTrue(hook.stat().st_mode & 0o111)
        cases = (
            ("", "active", True, True), ("8081", "active", True, True),
            ("1", "inactive", True, False), ("65535", "inactive", True, False),
            ("0", "active", False, False), ("65536", "active", False, False),
            ("08080", "active", False, False), ("123456", "active", False, False),
            ("80a", "active", False, False), ("-1", "active", False, False),
            (" 8081", "active", False, False),
        )
        for port, state, accepted, restarted in cases:
            with self.subTest(port=port, state=state), \
                 tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                log = root / "restarts"
                env = fake_snapctl_env(root, SNAP_INSTANCE_NAME="bitgarth-web",
                                       FAKE_PORT=port, FAKE_STATE=state, FAKE_LOG=str(log))
                result = subprocess.run([str(hook)], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, accepted, result.stderr)
                self.assertEqual(log.read_text() if log.exists() else "",
                                 "restart bitgarth-web.bitgarth-web\n" if restarted else "")


class BuildContractTests(unittest.TestCase):
    def test_pairing_review_sends_code_in_get_json_body(self):
        installed = load_installed()
        observed = {}

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                observed["path"] = self.path
                observed["body"] = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"status":"ready"}')

            def log_message(self, *_args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            with patch.object(installed, "BASE", f"http://127.0.0.1:{server.server_port}"):
                self.assertEqual(installed.get_json(installed.client(), "/_app/pairings/review",
                                                    {"code": "AAAA-BBBB"}), {"status": "ready"})
            self.assertEqual(observed, {"path": "/_app/pairings/review",
                                        "body": {"code": "AAAA-BBBB"}})
        finally:
            server.shutdown()
            server.server_close()
            worker.join()

    def test_pairing_code_handles_two_lines_in_one_pipe_read(self):
        installed = load_installed()
        process = subprocess.Popen(
            ["python3", "-c", "import sys,time; sys.stderr.write('Approve at: url\\nPairing code: 1234-5678\\n'); sys.stderr.flush(); time.sleep(2)"],
            stderr=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(installed.pairing_code(process, timeout=0.5), "1234-5678")
        finally:
            process.kill()
            process.wait()
            process.stdout.close()
            process.stderr.close()

    def test_pairing_code_accepts_letters_used_by_server(self):
        installed = load_installed()
        process = subprocess.Popen(
            ["python3", "-c", "import sys,time; sys.stderr.write('Pairing code: A2CD-EF7H\\n'); sys.stderr.flush(); time.sleep(2)"],
            stderr=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(installed.pairing_code(process, timeout=0.5), "A2CD-EF7H")
        finally:
            process.kill()
            process.wait()
            process.stdout.close()
            process.stderr.close()

    def test_public_release_lookup_needs_no_gh_auth(self):
        build = load_build()
        response = {"tag_name": "v0.4.0", "draft": False, "prerelease": False,
                    "html_url": "https://github.com/BitGarth/bitgarth/releases/tag/v0.4.0",
                    "assets": [{"name": "SHA256SUMS"}]}
        with patch.object(build, "urlopen", return_value=io.BytesIO(json.dumps(response).encode())), \
             patch.object(build.subprocess, "run", side_effect=AssertionError("gh called")):
            release = build.fetch_release("v0.4.0")
        self.assertEqual(release["tagName"], "v0.4.0")
        self.assertEqual(release["assets"], [{"name": "SHA256SUMS"}])

    def test_rejects_malformed_tags(self):
        build = load_build()
        for tag in ("0.4.0", "v0.4.0-rc1", "v0.4", "v1.2.3;echo bad"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                build.validate_tag(tag)
        self.assertEqual(build.validate_tag("v0.4.0"), "0.4.0")

    def test_checksum_lines_are_unique_and_complete(self):
        build = load_build()
        names = ("cli.tar.gz", "web.tar.gz")
        one = "a" * 64 + "  cli.tar.gz\n"
        two = "b" * 64 + "  web.tar.gz\n"
        self.assertEqual(build.parse_checksums(one + two, names),
                         {"cli.tar.gz": "a" * 64, "web.tar.gz": "b" * 64})
        for value in (one, one + one + two, "bad  cli.tar.gz\n" + two):
            with self.subTest(value=value), self.assertRaises(ValueError):
                build.parse_checksums(value, names)

    def test_archive_rejects_traversal_and_links(self):
        build = load_build()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("../outside", "/absolute"):
                path = root / "bad.tar.gz"
                archive(path, {name: b"bad"})
                with self.subTest(name=name), self.assertRaises(ValueError):
                    build.extract_archive(path, root / "out")
            path = root / "link.tar.gz"
            with tarfile.open(path, "w:gz") as package:
                link = tarfile.TarInfo("escape")
                link.type = tarfile.SYMTYPE
                link.linkname = "../../outside"
                package.addfile(link)
            with self.assertRaises(ValueError):
                build.extract_archive(path, root / "out")
            self.assertFalse((root / "outside").exists())

    def test_web_requires_catalog_and_public_assets(self):
        build = load_build()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "bitgarth-web"
            binary.write_bytes(b"binary")
            binary.chmod(0o755)
            (root / "public").mkdir()
            with self.assertRaisesRegex(ValueError, "public"):
                build.validate_payload("web", root)
            (root / "public/index.html").write_text("html")
            with self.assertRaisesRegex(ValueError, "unsynced_asset_catalog.json"):
                build.validate_payload("web", root)

    def test_extraction_preserves_binary_mode_and_content(self):
        build = load_build()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "cli.tar.gz"
            archive(path, {"bitgarth": b"#!/bin/sh\necho ok\n"})
            output = root / "payload"
            build.extract_archive(path, output)
            build.validate_payload("cli", output)
            self.assertEqual((output / "bitgarth").read_bytes(), b"#!/bin/sh\necho ok\n")
            self.assertTrue((output / "bitgarth").stat().st_mode & 0o111)

    def test_template_has_exactly_one_version_token(self):
        build = load_build()
        self.assertEqual(build.render_template("version: '@VERSION@'", "0.4.0"),
                         "version: '0.4.0'")
        for template in ("version: 1", "@VERSION@ @VERSION@"):
            with self.assertRaises(ValueError):
                build.render_template(template, "0.4.0")

    def test_bad_checksum_never_reaches_snapcraft(self):
        build = load_build()
        tag = "v0.4.0"
        names = [f"bitgarth-{product}-{tag}-linux-x86_64-gnu.tar.gz"
                 for product in ("cli", "web")]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            inputs = root / "inputs"
            inputs.mkdir()
            for name in names:
                archive(inputs / name, {"bitgarth": b"binary"})
            (inputs / "SHA256SUMS").write_text(
                "0" * 64 + f"  {names[0]}\n" +
                hashlib.sha256((inputs / names[1]).read_bytes()).hexdigest() +
                f"  {names[1]}\n")
            release = {"tagName": tag, "isDraft": False, "isPrerelease": False,
                       "url": "https://example.invalid/release",
                       "assets": [{"name": name} for name in (*names, "SHA256SUMS")]}

            def download(_tag, name, destination):
                shutil.copy2(inputs / name, destination / name)

            with patch.object(build, "fetch_release", return_value=release), \
                 patch.object(build, "download_asset", side_effect=download), \
                 patch.object(build.sys, "platform", "linux"), \
                 patch.object(build.platform, "machine", return_value="x86_64"), \
                 patch.object(build, "pack_product") as pack:
                with self.assertRaisesRegex(ValueError, "checksum"):
                    build.build_release(tag, root / "output")
                pack.assert_not_called()


class ManifestTests(unittest.TestCase):
    def test_artifact_manifest_rejects_tampered_snap(self):
        verify = load_verify()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            packages = {}
            for name in ("bitgarth-cli", "bitgarth-web"):
                filename = f"{name}_0.4.0_amd64.snap"
                (root / filename).write_bytes(name.encode())
                packages[name] = {"file": filename, "version": "0.4.0",
                                  "architecture": "amd64",
                                  "sha256": hashlib.sha256(name.encode()).hexdigest()}
            (root / "manifest.json").write_text(json.dumps({
                "release_tag": "v0.4.0", "packages": packages}))
            verify.verify(root, "v0.4.0")
            (root / "bitgarth-cli_0.4.0_amd64.snap").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "hash"):
                verify.verify(root, "v0.4.0")


if __name__ == "__main__":
    unittest.main()
