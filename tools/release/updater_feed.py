#!/usr/bin/env python3
"""The release feed's shapes, in one place (docs/design/versioned-auto-update.md §2.2·§2.7).

`lane.sh` calls these through the sub-commands below; `tests/test_lane.py`
calls the functions. Nothing here reaches the network or the lane's state —
every function takes text or paths and returns text.

  latest      the Tauri v2 updater feed:
              {version, notes, pub_date, platforms{<platform>{signature, url}}}
  section     the CHANGELOG.md body under `## [<version>] — …` (the release notes)
  fingerprint the minisign key id of a public key — the .pub box text, or
              tauri's base64 of it as tauri.conf.json plugins.updater.pubkey carries
  conf-pubkey plugins.updater.pubkey out of a tauri.conf.json (a path or the text)
  version     [workspace.package] version of a Cargo.toml
  manifest    the old zo CLI's manifest.txt (schema=1 / version= / base= / asset=…×3)
  sums        SHA256SUMS lines (`<sha256>  <basename>`), the old CLI's shape

Run: python3 tools/release/updater_feed.py <sub-command> …   (stdlib only)
"""

import argparse
import base64
import hashlib
import json
import sys
from pathlib import Path

SECTION_HEAD = "## ["
FEED_KEYS = ("version", "notes", "pub_date", "platforms")


def latest_json(version, notes, pub_date, platforms):
    """The feed document, keys in the updater's order, one trailing newline."""
    doc = {"version": version, "notes": notes, "pub_date": pub_date, "platforms": platforms}
    return json.dumps(doc, indent=2, ensure_ascii=False) + "\n"


def changelog_section(text, version):
    """The lines under `## [<version>] …` up to the next section, blank edges
    trimmed; None when the changelog has no such section."""
    head = f"{SECTION_HEAD}{version}] "
    lines = text.splitlines(keepends=True)
    for i, line in enumerate(lines):
        if not line.startswith(head):
            continue
        body = []
        for rest in lines[i + 1:]:
            if rest.startswith(SECTION_HEAD):
                break
            body.append(rest)
        while body and not body[0].strip():
            body.pop(0)
        while body and not body[-1].strip():
            body.pop()
        if body and not body[-1].endswith("\n"):
            body[-1] += "\n"
        return "".join(body)
    return None


def fingerprint(text):
    """The 8-byte minisign key id, hex, of a public key given either as the
    .pub box (an untrusted-comment line and a base64 key) or as that box
    base64-encoded whole, which is what `tauri signer generate` writes and
    what plugins.updater.pubkey carries. None when it is neither."""
    box = text.strip()
    if "untrusted comment" not in box:
        try:
            box = base64.b64decode(box, validate=True).decode()
        except Exception:
            return None
    lines = [line.strip() for line in box.splitlines() if line.strip()]
    if len(lines) < 2 or not lines[0].startswith("untrusted comment"):
        return None
    try:
        raw = base64.b64decode(lines[1], validate=True)
    except Exception:
        return None
    if len(raw) < 10 or raw[:2] != b"Ed":
        return None
    return raw[2:10].hex()


def conf_pubkey(conf_text, base_dir=None):
    """plugins.updater.pubkey of a tauri.conf.json — the text itself, or the
    contents of the file it names (tauri accepts a path there too)."""
    try:
        conf = json.loads(conf_text)
        pub = conf["plugins"]["updater"]["pubkey"]
    except (ValueError, KeyError, TypeError):
        return None
    if not isinstance(pub, str) or not pub.strip():
        return None
    candidate = Path(base_dir or ".") / pub
    if candidate.is_file():
        return candidate.read_text()
    return pub


def workspace_version(cargo_text):
    """`version = "…"` under [workspace.package] — that section's own line."""
    in_section = False
    for line in cargo_text.splitlines():
        if line.startswith("["):
            in_section = line.strip() == "[workspace.package]"
            continue
        if in_section and line.split("=", 1)[0].strip() == "version":
            return line.split('"')[1]
    return None


def manifest_text(version, base, bins):
    """The old install.sh's manifest: exactly schema, version, base and one
    asset row per target in the order given — `<triple>|zo-v<version>-<triple>|<sha256>|<bytes>`."""
    rows = [f"schema=1\n", f"version={version}\n", f"base={base}\n"]
    for triple, path in bins.items():
        data = Path(path).read_bytes()
        rows.append(f"asset={triple}|zo-v{version}-{triple}|{hashlib.sha256(data).hexdigest()}|{len(data)}\n")
    return "".join(rows)


def sha256sums(paths):
    """`<sha256>  <basename>` per path, in the order given."""
    return "".join(f"{hashlib.sha256(Path(p).read_bytes()).hexdigest()}  {Path(p).name}\n" for p in paths)


def main(argv):
    ap = argparse.ArgumentParser(prog="updater_feed.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("latest")
    p.add_argument("--version", required=True)
    p.add_argument("--notes-file", required=True)
    p.add_argument("--pub-date", required=True)
    p.add_argument("--platform", required=True)
    p.add_argument("--sig-file", required=True)
    p.add_argument("--url", required=True)
    p = sub.add_parser("section")
    p.add_argument("--changelog", required=True)
    p.add_argument("--version", required=True)
    p = sub.add_parser("fingerprint")
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument("--file")
    g.add_argument("--text")
    p = sub.add_parser("conf-pubkey")
    p.add_argument("conf")
    p = sub.add_parser("version")
    p.add_argument("cargo")
    p = sub.add_parser("manifest")
    p.add_argument("--version", required=True)
    p.add_argument("--base", required=True)
    p.add_argument("asset", nargs="+", help="<triple>=<path>, in install.sh's order")
    p = sub.add_parser("sums")
    p.add_argument("path", nargs="+")
    a = ap.parse_args(argv)

    if a.cmd == "latest":
        notes = Path(a.notes_file).read_text()
        signature = Path(a.sig_file).read_text().strip()
        sys.stdout.write(latest_json(a.version, notes, a.pub_date, {a.platform: {"signature": signature, "url": a.url}}))
        return 0
    if a.cmd == "section":
        path = Path(a.changelog)
        body = changelog_section(path.read_text(), a.version) if path.is_file() else None
        if body is None:
            sys.stderr.write(f"{a.changelog}: no [{a.version}] section\n")
            return 1
        sys.stdout.write(body)
        return 0
    if a.cmd == "fingerprint":
        text = Path(a.file).read_text() if a.file else a.text
        fp = fingerprint(text)
        if fp is None:
            return 1
        print(fp)
        return 0
    if a.cmd == "conf-pubkey":
        path = Path(a.conf)
        pub = conf_pubkey(path.read_text(), path.parent) if path.is_file() else None
        if pub is None:
            return 1
        sys.stdout.write(pub.strip() + "\n")
        return 0
    if a.cmd == "version":
        v = workspace_version(Path(a.cargo).read_text())
        if v is None:
            return 1
        print(v)
        return 0
    if a.cmd == "manifest":
        bins = {}
        for spec in a.asset:
            triple, _, path = spec.partition("=")
            bins[triple] = path
        sys.stdout.write(manifest_text(a.version, a.base, bins))
        return 0
    if a.cmd == "sums":
        sys.stdout.write(sha256sums(a.path))
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
