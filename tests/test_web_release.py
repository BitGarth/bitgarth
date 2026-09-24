"""Smoke-test an extracted web release: python3 tests/test_web_release.py DIRECTORY."""

import argparse
from contextlib import closing, contextmanager
from datetime import datetime, timezone
from http.cookiejar import CookieJar
import json
import os
from pathlib import Path
import re
import shutil
import socket
import sqlite3
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


def child_environment(project):
    env = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("BITGARTH_", "DIOXUS_"))
    }
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    env.update(IP="127.0.0.1", PORT=str(port), BITGARTH_PROJECT_DIR=str(project),
               RUST_LOG="info")
    assert "BITGARTH_CHANNEL" not in env
    return env


@contextmanager
def healthy_server(binary, package, env, *, check_metadata=False):
    base_url = f"http://127.0.0.1:{env['PORT']}"
    client = build_opener(ProxyHandler({}), HTTPCookieProcessor(CookieJar()))
    with tempfile.TemporaryFile() as log:
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
            yield client, base_url
        except BaseException:
            if not check_metadata:
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


def run_failed_start(binary, package, env):
    try:
        result = subprocess.run(
            [str(binary)], cwd=package, env=env,
            capture_output=True, timeout=30, check=False,
        )
    except subprocess.TimeoutExpired as error:
        output = (error.stdout or b"") + (error.stderr or b"")
        raise AssertionError("failed startup did not exit within 30 seconds:\n" +
                             output.decode("utf-8", errors="replace")) from error
    output = result.stdout.decode("utf-8", errors="replace")
    error_output = result.stderr.decode("utf-8", errors="replace")
    assert result.returncode == 1, output + error_output
    assert "tasks: task manager started" not in output + error_output
    assert "tasks: task manager loop started" not in output + error_output
    assert "Starting app=" not in output + error_output
    return error_output


def app_snapshot(path):
    with closing(sqlite3.connect(path)) as connection:
        return list(connection.iterdump())


def check_report(root, error_output, category):
    report = (root / "startup-error.txt").read_bytes()
    assert len(report) <= 16 * 1024
    text = report.decode("utf-8")
    assert f"Category: {category}\n" in text, text
    assert text in error_output, error_output
    confirmation = f"Startup diagnostic saved to: {root / 'startup-error.txt'}\n"
    assert error_output.endswith(confirmation), error_output
    assert error_output.index(confirmation) >= error_output.index(text) + len(text), error_output
    assert confirmation not in text, "saved file must contain only the original report"
    return report


def check_failed_starts(binary, package, fixture):
    mutations = [
        ("UPDATE refinery_schema_history SET name = 'payment_product_options_cache' WHERE version = 8", "schema-divergent"),
        ("UPDATE refinery_schema_history SET checksum = '0' WHERE version = 8", "schema-divergent"),
        ("UPDATE refinery_schema_history SET version = 2147483647 WHERE version = (SELECT MAX(version) FROM refinery_schema_history)", "schema-missing"),
        ("DELETE FROM refinery_schema_history WHERE version = 8", "schema-missing"),
        ("UPDATE refinery_schema_history SET checksum = 'invalid-checksum' WHERE version = 8", "migration-failed"),
        ("UPDATE refinery_schema_history SET applied_on = 'invalid-timestamp' WHERE version = 8", "migration-failed"),
    ]
    for mutation, category in mutations:
        with tempfile.TemporaryDirectory() as data:
            root = Path(data)
            database = root / "app/data/app.db"
            database.parent.mkdir(parents=True)
            shutil.copyfile(fixture, database)
            sentinel = root / "users/preserve-me.txt"
            sentinel.parent.mkdir()
            sentinel.write_text("preserve this user data", encoding="utf-8")
            with closing(sqlite3.connect(database)) as connection, connection:
                assert connection.execute(mutation).rowcount == 1, mutation
            before = app_snapshot(database)
            error_output = run_failed_start(binary, package, child_environment(root))
            check_report(root, error_output, category)
            assert "invalid-checksum" not in error_output, error_output
            assert "invalid-timestamp" not in error_output, error_output
            assert app_snapshot(database) == before, mutation
            assert sentinel.read_text(encoding="utf-8") == "preserve this user data"
            assert not (root / "app/data/prices/prices.db").exists()
            print(f"Startup rejection passed: {mutation}")

    with tempfile.TemporaryDirectory() as data:
        root = Path(data)
        obstruction = root / "app"
        obstruction.write_text("test-owned obstruction", encoding="utf-8")
        error_output = run_failed_start(binary, package, child_environment(root))
        report = check_report(root, error_output, "database-path")
        assert obstruction.read_text(encoding="utf-8") == "test-owned obstruction"
        obstruction.unlink()
        with healthy_server(binary, package, child_environment(root)):
            pass
        assert (root / "startup-error.txt").read_bytes() == report
        print("Startup path rejection and successful restart passed; report preserved")

    with tempfile.TemporaryDirectory() as data:
        root = Path(data)
        (root / "app").write_text("test-owned obstruction", encoding="utf-8")
        destination = root / "startup-error.txt"
        destination.mkdir()
        error_output = run_failed_start(binary, package, child_environment(root))
        assert "Category: database-path\n" in error_output, error_output
        assert error_output.count("Could not save startup diagnostic") == 1, error_output
        assert destination.is_dir()
        assert (root / "app").read_text(encoding="utf-8") == "test-owned obstruction"
        print("Unsafe diagnostic destination preserved; original failure reported")

    with tempfile.TemporaryDirectory() as data:
        obstruction = Path(data) / "configured-root-file"
        obstruction.write_text("test-owned root obstruction", encoding="utf-8")
        for root in (obstruction, obstruction / "child"):
            error_output = run_failed_start(binary, package, child_environment(root))
            assert "Category: database-path\n" in error_output, error_output
            assert f"Project directory: {root}\n" in error_output, error_output
            assert f"Database: {root / 'app/data/app.db'}\n" in error_output, error_output
            assert "IO kind:" in error_output.split("Recovery:\n")[0], error_output
            assert "OS code: Some(" in error_output.split("Recovery:\n")[0], error_output
            assert error_output.count("Could not save startup diagnostic") == 1, error_output
            assert obstruction.read_text(encoding="utf-8") == "test-owned root obstruction"
        print("Known configured roots retained with filesystem causes")

    with tempfile.TemporaryDirectory(dir=package) as data:
        root = Path(data)
        error_output = run_failed_start(binary, package, child_environment(root.name))
        assert "Category: database-path\n" in error_output, error_output
        assert "Project directory: unavailable\n" in error_output, error_output
        assert error_output.count("Could not save startup diagnostic") == 1, error_output
        assert not list(root.iterdir()), "unresolved root must not receive a report or database"
        print("Relative project root rejected with stderr-only diagnostic")


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

    with tempfile.TemporaryDirectory() as data:
        with healthy_server(binary, package, child_environment(data)) as (client, base_url):
            with client.open(base_url + "/", timeout=10) as response:
                assert response.status == 200
                assert b"<script" in response.read(), "HTML has no browser initialization"
            for asset in (wasm, javascript):
                with client.open(base_url + "/" + asset.relative_to(public).as_posix(),
                                 timeout=10) as response:
                    assert response.status == 200
                    assert response.read() == asset.read_bytes(), f"incorrect asset: {asset}"
        fixture = Path(data) / "app/data/app.db"
        assert fixture.is_file(), "successful startup did not create app.db"
        with closing(sqlite3.connect(fixture)) as connection:
            assert connection.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()[0] == 0
        check_failed_starts(binary, package, fixture)

    if args.check_metadata:
        with tempfile.TemporaryDirectory() as data:
            with healthy_server(binary, package, child_environment(data),
                                check_metadata=True) as (client, base_url):
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
    print("Web release smoke check passed: startup, health, HTML, WASM, JavaScript, "
          "offline startup failures and recovery")


if __name__ == "__main__":
    main()
