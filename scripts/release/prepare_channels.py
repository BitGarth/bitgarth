#!/usr/bin/env python3
"""Generate uncommitted landing/Umbrel edits from a verified published release."""

import argparse
from datetime import date
from html import escape
from pathlib import Path
import re
import sys
from textwrap import indent

from common import (ReleaseError, binary_release, command, docker_release,
                    read_editable, require_latest, source_file, version, write_edits)

SECTIONS = ("Highlights", "New", "Fixed", "Notes")


def parse_notes(text, release_version):
    lines = text.splitlines()
    if not lines or lines[0] != f"# BitGarth {release_version}":
        raise ReleaseError("release-note title does not match the release version")
    sections = []
    for line in lines[1:]:
        if not line.strip():
            continue
        if line.startswith("## "):
            heading = line[3:]
            if (heading not in SECTIONS or
                    (sections and SECTIONS.index(heading) <= SECTIONS.index(sections[-1][0]))):
                raise ReleaseError("notes must use Highlights, New, Fixed, Notes in order")
            sections.append((heading, []))
        elif line.startswith("- ") and sections:
            sections[-1][1].append(line[2:])
        elif line.startswith("  ") and sections and sections[-1][1]:
            if re.match(r"\s*(?:[-*#>]|\d+\.)\s", line):
                raise ReleaseError("nested lists/blocks are not supported in release notes")
            sections[-1][1][-1] += " " + line.strip()
        else:
            raise ReleaseError(f"unsupported release-note line: {line}")
    if not sections or any(not items or any(not item.strip() for item in items)
                           for _, items in sections):
        raise ReleaseError("release notes contain an empty section or item")
    return sections


def inline_html(text):
    # The release-note guide uses plain prose, emphasis, code and HTTPS links.
    # Escape everything else, including raw HTML, before it reaches the page.
    pattern = r"`([^`]+)`|\*\*([^*]+)\*\*|\*([^*]+)\*|\[([^\]]+)\]\((https://[^\s)]+)\)"
    result, end = [], 0
    for match in re.finditer(pattern, text):
        result.append(escape(text[end:match.start()], quote=False))
        code, bold, emphasis, label, url = match.groups()
        if code is not None:
            result.append(f"<code>{escape(code, quote=False)}</code>")
        elif bold is not None:
            result.append(f"<strong>{inline_html(bold)}</strong>")
        elif emphasis is not None:
            result.append(f"<em>{inline_html(emphasis)}</em>")
        else:
            result.append(f'<a href="{escape(url, quote=True)}">{inline_html(label)}</a>')
        end = match.end()
    result.append(escape(text[end:], quote=False))
    return "".join(result)


def render_landing(text, sections, release_version, published_date):
    articles = list(re.finditer(r'(?m)^([ \t]*)<article class="release" id="([^"]+)">.*?</article>',
                                text, re.DOTALL))
    if not articles:
        raise ReleaseError("landing changelog article layout not found")
    target = version("v" + release_version)
    versions = [version("v" + article[2]) for article in articles]
    if max(versions) > target or versions.count(target) > 1:
        raise ReleaseError("landing update would downgrade or duplicate a release")
    if target in versions and versions[0] != target:
        raise ReleaseError("existing release must be the first article")
    published = date.fromisoformat(published_date)
    months = ("January February March April May June July August September October November December").split()
    display_date = f"{published.day} {months[published.month - 1]} {published.year}"
    lines = [f'<article class="release" id="{release_version}">',
             '  <header class="release-head">', '    <h2 class="release-version">',
             f'      {release_version}',
             f'      <a class="release-anchor" href="#{release_version}" aria-label="Copy link to version {release_version}">#</a>',
             '    </h2>', '    <span class="latest-tag">Latest</span>',
             f'    <p class="release-date">{display_date}</p>', '  </header>']
    for heading, items in sections:
        lines += ["", f'  <section class="change-group {heading.lower()}">', f"    <h3>{heading}</h3>"]
        if heading == "Highlights":
            lines += [f'    <p class="highlight">{inline_html(item)}</p>' for item in items]
        else:
            lines += ['    <ul class="change-list">']
            lines += [f"      <li>{inline_html(item)}</li>" for item in items]
            lines += ["    </ul>"]
        lines += ["  </section>"]
    lines += ["", "</article>"]
    first = articles[0]
    block = indent("\n".join(lines), first[1])
    if versions[0] == target:
        return text[:first.start()] + block + text[first.end():]
    previous = re.sub(r'(?m)^[ \t]*<span class="latest-tag">Latest</span>\n', "", first[0])
    return text[:first.start()] + block + "\n\n" + previous + text[first.end():]


def replace_once(pattern, replacement, text, label):
    result, count = re.subn(pattern, lambda _: replacement, text, flags=re.MULTILINE)
    if count != 1:
        raise ReleaseError(f"expected exactly one {label}; found {count}")
    return result


def render_umbrel(text, sections, release_version):
    current = re.search(r'^version: "([^"]+)"$', text, re.MULTILINE)
    if not current or version("v" + current[1]) > version("v" + release_version):
        raise ReleaseError("invalid Umbrel version or attempted downgrade")
    block = re.search(r"(?ms)^releaseNotes: \|\n(.*?)(?=^[^\s]|\Z)", text)
    if not block:
        raise ReleaseError("Umbrel releaseNotes block not found")
    links = re.findall(r"^  (Full release notes at https://bitgarth\.app/releases(?:\.html)?#)[0-9.]+$",
                       block[1], re.MULTILINE)
    if len(links) != 1:
        raise ReleaseError("Umbrel full-notes link not found")
    lines = ["releaseNotes: |"]
    for index, (heading, items) in enumerate(sections):
        if index:
            lines.append("")
        if heading != "Highlights":
            lines += [f"  {heading}:", ""]
        lines += [f"  - {item}" for item in items]
    lines += ["", f"  {links[0]}{release_version}", ""]
    result = text[:block.start()] + "\n".join(lines) + text[block.end():]
    return replace_once(r'^version: "[^"]+"$', f'version: "{release_version}"', result, "Umbrel version")


def prepare(tag, landing, umbrel):
    sha, release = binary_release(tag)
    require_latest(tag)
    sections = parse_notes(source_file(f"docs/release-notes/{tag[1:]}.md", sha).decode(), tag[1:])
    digest = docker_release(tag, sha)
    landing_path, app_path, compose_path = landing / "releases.html", umbrel / "umbrel-app.yml", umbrel / "docker-compose.yml"
    page, app, compose = map(read_editable, (landing_path, app_path, compose_path))
    new_page = render_landing(page, sections, tag[1:], release["published_at"][:10])
    new_app = render_umbrel(app, sections, tag[1:])
    if not re.search(r"^\s+BITGARTH_CHANNEL: umbrel$", compose, re.MULTILINE):
        raise ReleaseError("Umbrel channel metadata is absent")
    new_compose = replace_once(r"^    image: bitgarth/bitgarth:[^\n]+$",
                               f"    image: bitgarth/bitgarth:{tag[1:]}@{digest}", compose, "Umbrel image")
    for content in (new_app, new_compose):
        command("ruby", "-e", "require 'yaml'; YAML.safe_load(STDIN.read, aliases: true)", input=content)
    write_edits([(landing_path, page, new_page), (app_path, app, new_app),
                 (compose_path, compose, new_compose)])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--landing", type=Path, required=True, help="landing repository checkout")
    parser.add_argument("--umbrel", type=Path, required=True, help="ferntrail-bitgarth app directory")
    args = parser.parse_args()
    try:
        prepare(args.tag, args.landing, args.umbrel)
    except (ReleaseError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
