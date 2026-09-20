"""Smoke-test an extracted web release: python3 tests/test_web_release.py DIRECTORY."""

import argparse
from datetime import datetime, timezone
from http.cookiejar import CookieJar
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import tempfile
import time
from urllib.error import URLError
from urllib.request import HTTPCookieProcessor, ProxyHandler, Request, build_opener


def legal_version(name):
    legal = (Path(__file__).resolve().parents[1] / "src/legal.rs").read_text()
    match = re.search(rf'^pub\(crate\) const {name}: &str = "([^"]+)";', legal, re.MULTILINE)
    assert match, f"missing {name} declaration in src/legal.rs"
    return match.group(1)


def post_json(client, url, payload):
    request = Request(url, data=json.dumps(payload).encode(),
                      headers={"Content-Type": "application/json"}, method="POST")
    with client.open(request, timeout=30) as response:
        assert response.status == 200
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--check-metadata", action="store_true")
    parser.add_argument("--expected-platform")
    parser.add_argument("--expected-version")
    args = parser.parse_args()
    if args.check_metadata and not (args.expected_platform and args.expected_version):
        parser.error("--check-metadata requires --expected-platform and --expected-version")
    package = args.package.resolve()
    binary = package / ("bitgarth-web.exe" if os.name == "nt" else "bitgarth-web")
    public = package / "public"
    assert binary.is_file(), binary
    assert (public / "index.html").is_file(), "missing browser entry point"
    assert (package / "assets/catalog/unsynced_asset_catalog.json").is_file(), "missing catalog"
    wasm = next(public.rglob("*.wasm"), None)
    javascript = next(public.rglob("*.js"), None)
    assert wasm and javascript, "missing browser WASM or JavaScript"

    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    base_url = f"http://127.0.0.1:{port}"
    client = build_opener(ProxyHandler({}), HTTPCookieProcessor(CookieJar()))
    with tempfile.TemporaryDirectory() as data, tempfile.TemporaryFile() as log:
        env = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("BITGARTH_", "DIOXUS_"))
        }
        env.update(IP="127.0.0.1", PORT=str(port), BITGARTH_PROJECT_DIR=data)
        assert "BITGARTH_CHANNEL" not in env
        server = subprocess.Popen([str(binary)], cwd=package, env=env,
                                  stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 60
            while True:
                assert server.poll() is None, f"server exited with {server.returncode}"
                try:
                    with client.open(base_url + "/health", timeout=2) as response:
                        assert response.status == 200
                    break
                except URLError:
                    assert time.monotonic() < deadline, "server did not become healthy"
                    time.sleep(0.2)
            with client.open(base_url + "/", timeout=10) as response:
                assert response.status == 200
                assert b"<script" in response.read(), "HTML has no browser initialization"
            for asset in (wasm, javascript):
                with client.open(base_url + "/" + asset.relative_to(public).as_posix(),
                                 timeout=10) as response:
                    assert response.status == 200
                    assert response.read() == asset.read_bytes(), f"incorrect asset: {asset}"
            if args.check_metadata:
                post_json(client, base_url + "/_app/auth/register", {
                    "username": "release-smoke", "password": "ReleaseSmoke123!",
                    "legal_acknowledgement": {
                        "accepted_terms_version": legal_version("TERMS_VERSION"),
                        "accepted_privacy_version": legal_version("PRIVACY_VERSION"),
                    },
                })
                started = datetime.now(timezone.utc).isoformat()
                status = post_json(client, base_url + "/_app/updates/refresh", {"force": True})
                finished = datetime.now(timezone.utc).isoformat()
                assert status["channel"] == "web", status["channel"]
                assert status["current"] == args.expected_version.removeprefix("v"), status["current"]
                print(f"Update request UTC window: {started} to {finished}; expected version: "
                      f"{args.expected_version}; expected platform: {args.expected_platform}; "
                      f"release selected: {bool(status.get('latest'))}")
        except BaseException:
            if not args.check_metadata:
                log.seek(0)
                print(log.read().decode(errors="replace"), file=sys.stderr)
            raise
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
    print("Web release smoke check passed: startup, health, HTML, WASM, JavaScript")


if __name__ == "__main__":
    main()
