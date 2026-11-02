#!/usr/bin/env python3
"""Report release readiness without publishing or changing anything."""

import argparse
import json
import os
from pathlib import Path
import sys
from urllib.request import Request, urlopen

from common import (Pending, ReleaseError, api, binary_release, docker_release,
                    require_latest, require_workflow, source_file, version)


def get_json(url):
    with urlopen(Request(url, headers={"User-Agent": "bitgarth-release-check"}), timeout=30) as response:
        return json.load(response)


def check_channels(tag):
    channels = get_json("https://bitgarth.com/api/v2/latest-app-version")["channels"]
    for name in ("default", "docker", "umbrel"):
        if channels.get(name, {}).get("latest") != tag[1:]:
            raise Pending(f"BitGarth's servers: {name} has not reached {tag}")
    for name, item in channels.items():
        if name.startswith("snap"):
            continue
        if item.get("latest") != tag[1:]:
            raise Pending(f"BitGarth's servers: {name} has not reached {tag}")


def check_homebrew(tag):
    repo = "FernTrail/homebrew-tap"
    sha = api("commits/main", repo)["sha"]
    for product in ("cli", "web"):
        formula = source_file(f"Formula/bitgarth-{product}.rb", sha, repo).decode()
        for platform in ("macos-aarch64", "linux-x86_64-gnu"):
            if f"/download/{tag}/bitgarth-{product}-{tag}-{platform}.tar.gz" not in formula:
                raise Pending(f"Homebrew {product}/{platform} has not reached {tag}")
    require_workflow("test.yml", sha, "main", repo)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--stage", choices=("binary", "docker", "all"), default="all")
    parser.add_argument("--expected-sha", help="require this exact public commit")
    parser.add_argument("--require-latest", action="store_true")
    parser.add_argument("--github-output", action="store_true", help="write validated tag/sha job outputs")
    args = parser.parse_args(argv)
    failed = False

    def check(label, action):
        nonlocal failed
        try:
            result = action()
            print(f"PASS {label}")
            return result
        except Pending as error:
            print(f"PENDING {label}: {error}")
        except (ReleaseError, OSError, ValueError, KeyError, TypeError) as error:
            print(f"FAIL {label}: {error}")
        failed = True
        return None

    if check("Release tag", lambda: version(args.tag)) is None:
        return 1
    evidence = check("Public CI, binary assets and notarization",
                     lambda: binary_release(args.tag, args.expected_sha))
    if args.require_latest:
        check("Latest stable release", lambda: require_latest(args.tag))
    if args.stage in ("docker", "all"):
        if evidence:
            check("Docker platforms and public source identity",
                  lambda: docker_release(args.tag, evidence[0]))
        else:
            print("PENDING Docker: public release evidence is required")
    if args.stage == "all":
        check("Homebrew formula versions and exact-commit CI", lambda: check_homebrew(args.tag))
        check("Published channel versions", lambda: check_channels(args.tag))
    print("DEFERRED Snap: Store approval pending; not a release gate")
    if args.github_output and not failed:
        output = os.environ.get("GITHUB_OUTPUT")
        if not output:
            print("FAIL GITHUB_OUTPUT is not set")
            return 1
        with Path(output).open("a") as stream:
            stream.write(f"tag={args.tag}\nsha={evidence[0]}\n")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
