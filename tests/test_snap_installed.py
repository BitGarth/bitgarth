#!/usr/bin/env python3
"""Smoke-test installed BitGarth snaps with synthetic private state."""

import argparse
import base64
from http.cookiejar import CookieJar
import json
import os
from pathlib import Path
import re
import secrets
import selectors
import shlex
import socket
import subprocess
import sys
import time
from urllib.error import URLError
from urllib.request import HTTPCookieProcessor, ProxyHandler, Request, build_opener, urlopen


BASE = "http://127.0.0.1:8080"
SERVICE = "snap.bitgarth-web.bitgarth-web.service"


def run(*args, timeout=30, input=None):
    return subprocess.run(args, input=input, capture_output=True, text=True,
                          timeout=timeout, check=True)


def snap_row(name):
    lines = run("snap", "list", name).stdout.strip().splitlines()
    if len(lines) != 2:
        raise AssertionError(f"snap list did not identify {name}")
    fields = lines[1].split()
    if fields[0] != name or len(fields) < 5:
        raise AssertionError(f"unexpected snap list row for {name}")
    if "devmode" in lines[1] or "classic" in lines[1]:
        raise AssertionError(f"{name} is not strictly installed")
    return fields[1], fields[2]


def check_install(expected_web, expected_cli, allow_partial):
    confinement = run("snap", "debug", "confinement").stdout.strip()
    if confinement != "strict":
        if not allow_partial or confinement != "partial":
            raise AssertionError(f"host confinement is {confinement}, expected strict")
        print("functional checks only; strict confinement not established")
    for name, expected in (("bitgarth-web", expected_web), ("bitgarth-cli", expected_cli)):
        version, revision = snap_row(name)
        if version != expected:
            raise AssertionError(f"{name} version {version}, expected {expected}")
        metadata = (Path("/snap") / name / "current/meta/snap.yaml").read_text()
        for field in ("base: core24", "confinement: strict", "amd64"):
            if field not in metadata:
                raise AssertionError(f"{name} installed metadata missing {field}")
        if f"  {name}:" not in metadata:
            raise AssertionError(f"{name} app missing from installed metadata")
        print(f"{name} {version} revision {revision}: strict snap metadata")
    for name, expected in (("bitgarth-cli", ("network",)),
                           ("bitgarth-web", ("network", "network-bind"))):
        connections = run("snap", "connections", name).stdout
        for interface in expected:
            if not re.search(rf"(?m)^{interface}\s+{name}:{interface}\s+", connections):
                raise AssertionError(f"{name} {interface} interface is not connected")
        for broad in ("home", "personal-files", "system-files"):
            if re.search(rf"(?m)^{broad}\s+{name}:", connections):
                raise AssertionError(f"unexpected broad {name} interface: {broad}")
    return confinement


def client():
    return build_opener(ProxyHandler({}), HTTPCookieProcessor(CookieJar()))


def get_json(browser, path, payload):
    request = Request(BASE + path, data=json.dumps(payload).encode(), method="GET",
                      headers={"Content-Type": "application/json"})
    with browser.open(request, timeout=30) as response:
        if response.status != 200:
            raise AssertionError(f"GET {path} returned HTTP {response.status}")
        return json.load(response)


def post_json(browser, path, payload):
    request = Request(BASE + path, data=json.dumps(payload).encode(), method="POST",
                      headers={"Content-Type": "application/json", "Origin": BASE})
    with browser.open(request, timeout=30) as response:
        if response.status != 200:
            raise AssertionError(f"POST {path} returned HTTP {response.status}")
        return json.load(response)


def check_web():
    if run("systemctl", "is-active", SERVICE).stdout.strip() != "active":
        raise AssertionError("web snap service is not active")
    browser = client()
    deadline = time.monotonic() + 60
    while True:
        try:
            with browser.open(BASE + "/health", timeout=2) as response:
                if response.status != 200:
                    raise AssertionError("web health endpoint failed")
            break
        except URLError:
            if time.monotonic() >= deadline:
                raise AssertionError("web health endpoint did not become ready")
            time.sleep(0.2)
    public = Path("/snap/bitgarth-web/current/public")
    catalog = Path("/snap/bitgarth-web/current/assets/catalog/unsynced_asset_catalog.json")
    if not catalog.is_file():
        raise AssertionError("installed web catalog missing")
    with browser.open(BASE + "/", timeout=10) as response:
        if response.status != 200 or b"<script" not in response.read():
            raise AssertionError("installed web root is not application HTML")
    for pattern in ("*.js", "*.wasm"):
        asset = next(public.rglob(pattern), None)
        if asset is None:
            raise AssertionError(f"installed web {pattern} asset missing")
        with browser.open(BASE + "/" + asset.relative_to(public).as_posix(), timeout=30) as response:
            if response.status != 200 or response.read() != asset.read_bytes():
                raise AssertionError(f"served web asset differs: {asset.relative_to(public)}")
    listeners = run("ss", "-ltn", "( sport = :8080 )").stdout.splitlines()[1:]
    if not listeners or any("127.0.0.1:8080" not in line for line in listeners):
        raise AssertionError("web listener is absent or not loopback-only")
    pid = run("systemctl", "show", SERVICE, "--property=MainPID", "--value").stdout.strip()
    if not pid.isdecimal() or pid == "0":
        raise AssertionError("web daemon has no MainPID")
    environment = subprocess.run(["sudo", "-n", "cat", f"/proc/{pid}/environ"],
                                 capture_output=True, timeout=15, check=True).stdout
    channels = [item for item in environment.split(b"\0") if item.startswith(b"BITGARTH_CHANNEL=")]
    if channels != [b"BITGARTH_CHANNEL=snap"]:
        raise AssertionError("web daemon did not receive BITGARTH_CHANNEL=snap")
    if subprocess.run(["sudo", "-n", "test", "-f",
                       "/var/snap/bitgarth-web/common/bitgarth/app/data/app.db"],
                      timeout=15).returncode != 0:
        raise AssertionError("persistent web app database missing")
    print("web service, assets, data, channel, and loopback listener passed")


def wait_for_health(base):
    browser = client()
    deadline = time.monotonic() + 60
    while True:
        try:
            with browser.open(base + "/health", timeout=2) as response:
                if response.status == 200:
                    return
        except OSError:
            pass
        if time.monotonic() >= deadline:
            raise AssertionError(f"{base} did not become healthy")
        time.sleep(0.2)


def check_port_config():
    rejected = subprocess.run(["sudo", "-n", "snap", "set", "bitgarth-web", "port=0"],
                              capture_output=True, text=True, timeout=60)
    if rejected.returncode == 0:
        raise AssertionError("web snap accepted an invalid port")
    run("sudo", "-n", "snap", "set", "bitgarth-web", "port=8081", timeout=120)
    try:
        wait_for_health("http://127.0.0.1:8081")
    finally:
        run("sudo", "-n", "snap", "unset", "bitgarth-web", "port", timeout=120)
    wait_for_health(BASE)
    print("web port change and reset passed")


def check_cli(expected):
    version = run("snap", "run", "bitgarth-cli", "--version").stdout.strip()
    if version != f"bitgarth {expected}":
        raise AssertionError(f"CLI reports {version!r}, expected {expected}")
    help_text = run("snap", "run", "bitgarth-cli", "--help").stdout
    if "balancesheet" not in help_text or "pair" not in help_text:
        raise AssertionError("installed CLI help lacks expected commands")
    if not Path("/snap/bin/bitgarth-cli").exists():
        raise AssertionError("bitgarth-cli command missing")
    print("installed CLI version and help passed")


def legal_versions(web_version):
    tag = "v" + web_version
    request = Request(
        f"https://api.github.com/repos/BitGarth/bitgarth/contents/src/legal.rs?ref={tag}",
        headers={"User-Agent": "BitGarth-Snap-Test"})
    with urlopen(request, timeout=30) as result:
        response = json.load(result)
    if response.get("encoding") != "base64":
        raise AssertionError("legal source response is not base64")
    source = base64.b64decode(response["content"]).decode()
    versions = {}
    for name in ("TERMS_VERSION", "PRIVACY_VERSION"):
        match = re.search(rf'^pub\(crate\) const {name}: &str = "([^"]+)";', source, re.MULTILINE)
        if match is None:
            raise AssertionError(f"{name} missing from installed release tag")
        versions[name] = match.group(1)
    return versions


def pairing_code(process, timeout=60):
    with selectors.DefaultSelector() as ready:
        ready.register(process.stderr, selectors.EVENT_READ)
        deadline = time.monotonic() + timeout
        output = b""
        while time.monotonic() < deadline:
            if not ready.select(timeout=max(0, deadline - time.monotonic())):
                break
            chunk = os.read(process.stderr.fileno(), 4096)
            output += chunk
            match = re.search(rb"Pairing code: ([A-Z0-9]{4}-[A-Z0-9]{4})", output)
            if match:
                return match.group(1).decode()
            if not chunk or len(output) > 65536 or process.poll() is not None:
                break
    raise AssertionError(f"CLI pairing code was not emitted within {timeout:g} seconds")


def check_profile(profile):
    path = Path.home() / "snap/bitgarth-cli/common/.config/BitGarth/cli/profiles.json"
    metadata = path.stat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise AssertionError("CLI profile has unsafe owner or permissions")
    if profile not in run("snap", "run", "bitgarth-cli", "profile", "list").stdout:
        raise AssertionError("CLI profile is not listed")
    print("CLI profile persists with private permissions")


def balancesheet(profile):
    result = run("snap", "run", "bitgarth-cli", "--profile", profile, "balancesheet",
                 timeout=60)
    if not result.stdout.strip():
        raise AssertionError("CLI balancesheet returned no output")
    print("authenticated balancesheet passed")


def seed(state_file, web_version):
    if state_file.exists():
        raise AssertionError(f"refusing to overwrite existing test state: {state_file}")
    suffix = secrets.token_hex(5)
    state = {"username": f"snaptest-{suffix}",
             "password": secrets.token_urlsafe(24) + "A1!",
             "profile": f"snaptest-{suffix}"}
    versions = legal_versions(web_version)
    browser = client()
    post_json(browser, "/_app/auth/register", {
        "username": state["username"], "password": state["password"],
        "legal_acknowledgement": {
            "accepted_terms_version": versions["TERMS_VERSION"],
            "accepted_privacy_version": versions["PRIVACY_VERSION"],
        },
    })
    process = subprocess.Popen(["snap", "run", "bitgarth-cli", "--profile", state["profile"],
                                "pair", BASE, "--allow-insecure-http"],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        code = pairing_code(process)
        review = get_json(browser, "/_app/pairings/review", {"code": code})
        pairing = review.get("pairing", {})
        if (review.get("status") != "ready" or pairing.get("code") != code or
                pairing.get("permissions") != ["balances_read"]):
            raise AssertionError("pairing review did not match requested capability")
        approved = post_json(browser, "/_app/pairings/approve", {
            "request": {
                "pairing_id": pairing["pairing_id"], "code": code,
                "permissions": ["balances_read"], "code_matches": True, "expires_at": None,
            },
        })
        if approved.get("status") != "approved":
            raise AssertionError("pairing approval failed")
        if process.wait(timeout=60) != 0:
            raise AssertionError("CLI pairing did not complete successfully")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        process.stdout.close()
        process.stderr.close()
    check_profile(state["profile"])
    balancesheet(state["profile"])
    state_file.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(state_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as output:
        json.dump(state, output)
    print("synthetic account and CLI pairing seeded")


def verify(state_file):
    if state_file.stat().st_mode & 0o077:
        raise AssertionError("test state file is not private")
    state = json.loads(state_file.read_text())
    browser = client()
    response = post_json(browser, "/_app/auth/login", {
        "username": state["username"], "password": state["password"]})
    if response.get("user", {}).get("username") != state["username"]:
        raise AssertionError("existing synthetic user could not log in")
    check_profile(state["profile"])
    balancesheet(state["profile"])
    print("existing account and pairing survived restart/refresh")


def check_home_isolation():
    sentinel = Path.home() / f".bitgarth-snap-test-{secrets.token_hex(6)}"
    descriptor = os.open(sentinel, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write("snap confinement sentinel\n")
        command = f"cat {shlex.quote(str(sentinel))}\nexit $?\n"
        result = subprocess.run(["snap", "run", "--shell", "bitgarth-cli"],
                                input=command, capture_output=True, text=True, timeout=20)
        if result.returncode == 0 or "snap confinement sentinel" in result.stdout:
            raise AssertionError("CLI snap could read a dotfile outside its home sandbox")
        print("strict home isolation sentinel denied")
    finally:
        sentinel.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("seed", "verify"))
    parser.add_argument("--expected-version", required=True)
    parser.add_argument("--expected-cli-version")
    parser.add_argument("--state-file", required=True, type=Path)
    parser.add_argument("--allow-partial", action="store_true")
    args = parser.parse_args()
    for version in (args.expected_version, args.expected_cli_version or args.expected_version):
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version):
            parser.error("expected versions must be MAJOR.MINOR.PATCH")
    if not args.state_file.is_absolute():
        parser.error("--state-file must be an absolute path")
    try:
        confinement = check_install(args.expected_version,
                                    args.expected_cli_version or args.expected_version,
                                    args.allow_partial)
        check_web()
        check_cli(args.expected_cli_version or args.expected_version)
        if args.mode == "seed":
            seed(args.state_file, args.expected_version)
        else:
            verify(args.state_file)
            check_port_config()
        if confinement == "strict":
            check_home_isolation()
    except (AssertionError, OSError, subprocess.CalledProcessError,
            subprocess.TimeoutExpired, URLError, ValueError) as error:
        print(f"installed snap check failed: {type(error).__name__}: {error}", file=sys.stderr)
        return 1
    print(f"installed snap {args.mode} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
