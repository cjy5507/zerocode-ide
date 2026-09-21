#!/usr/bin/env python3
"""Contract for `tools/release/pii-scan.py`, the re-entry gate for private values.

Every case runs the real script over a throwaway git repository built here, so
each row of the scanner's table is pinned from both sides: the value it must
catch and the placeholder it must forgive. The rest is the machinery a gate
lives or dies by — only tracked files are read, a binary is not read at all, a
row's `skip` holds, a waiver at the site holds, and the exit code is 1 while
anything is found.

The caught values below are fictional and belong to nobody; each one carries
its own waiver so this file does not trip the very gate it tests.

Run: python3 tools/release/tests/test_pii_scan.py   (stdlib only)
"""

import hashlib
import importlib.util
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
SCAN = REPO / "tools" / "release" / "pii-scan.py"

_spec = importlib.util.spec_from_file_location("pii_scan", SCAN)
pii_scan = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(pii_scan)

# One row per category: what the gate must catch, and what it must forgive.
# The caught column is fictional — a name, a subnet, a mailbox and a key id
# that exist nowhere — and each is waived on its own line.
CASES = (
    # (category, caught, allowed)   — the literals live here and nowhere else
    ("home-path", "/Users/mallory", "/Users/dev"),  # pii-scan: allow home-path — 이 게이트의 시험 픽스처
    ("private-ip", "10.99.44.7", "10.0.0.5"),  # pii-scan: allow private-ip — 이 게이트의 시험 픽스처
    ("email", "chief@northwind-holdings.co.kr", "one@example.com"),  # pii-scan: allow email — 이 게이트의 시험 픽스처
    ("credential", "AKIAQ7RVBNMLKJHGFDSZ", "AKIAIOSFODNN7EXAMPLE"),  # pii-scan: allow credential — 이 게이트의 시험 픽스처
)

CAUGHT = {category: caught for category, caught, _allowed in CASES}
ALLOWED = {category: allowed for category, _caught, allowed in CASES}


def run(root, *args):
    """The real script against `root`, as the recipe runs it."""
    return subprocess.run(
        [sys.executable, str(SCAN), "--root", str(root), *args],
        capture_output=True,
        text=True,
    )


def found(root, *args):
    """`--json` findings, keyed by category."""
    done = run(root, "--json", *args)
    assert done.returncode in (0, 1), done.stderr
    return json.loads(done.stdout)


class Checkout:
    """A throwaway git repository the scanner can read."""

    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)

    def write(self, name, text, *, track=True):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(text, bytes):
            path.write_bytes(text)
        else:
            path.write_text(text, encoding="utf-8")
        if track:
            subprocess.run(["git", "add", "-f", name], cwd=self.root, check=True)
        return path

    def close(self):
        self.tmp.cleanup()


class PiiScan(unittest.TestCase):
    def setUp(self):
        self.checkout = Checkout()
        self.addCleanup(self.checkout.close)

    # --- the table, from both sides ----------------------------------------

    def test_every_row_catches_its_value_and_forgives_its_placeholder(self):
        """One caught case and one allowed case per row of `RULES`."""
        for category, caught, allowed in CASES:
            with self.subTest(category=category):
                checkout = Checkout()
                self.addCleanup(checkout.close)
                checkout.write("caught.txt", f"a line naming {caught} here\n")
                checkout.write("allowed.txt", f"a line naming {allowed} here\n")
                hits = found(checkout.root)
                mine = hits[category]
                self.assertEqual(
                    [(h["path"], h["text"]) for h in mine],
                    [("caught.txt", caught)],
                    f"{category}: {hits}",
                )
                self.assertEqual(
                    sum(len(v) for v in hits.values()), 1,
                    f"{category}: the placeholder must be forgiven, not merely outranked: {hits}",
                )

    def test_a_home_directory_is_caught_on_either_separator(self):
        """Windows writes `C:\\Users\\dev`, and a Rust string doubles the
        backslash; a short placeholder is forgiven on every spelling.

        The prose above spells the placeholder, not a name: this file is read
        by the gate it tests, and the gate is right to refuse either one."""
        name = CAUGHT["home-path"].rsplit("/", 1)[-1]
        self.checkout.write(
            "a.rs",
            f'let p = "C:\\\\Users\\\\{name}\\\\x";\n'
            f'let q = r"C:\\Users\\{name}\\y";\n'
            'let ok = r"C:\\Users\\pi\\z";\n',
        )
        hits = found(self.checkout.root)["home-path"]
        self.assertEqual([h["line"] for h in hits], [1, 2], hits)

    def test_a_retina_asset_name_is_not_a_mailbox(self):
        """`icon@2x.png` and `128x128@2x.png` end in a scale suffix, not a
        domain: the mailbox row forgives that whole family of asset names."""
        self.checkout.write(
            "assets.rs",
            'const ICON: &str = "icons/icon@2x.png";\n'
            'const TILE: &str = "128x128@2x.png";\n'
            'const LOGO: &str = "logo@3x.webp";\n',
        )
        self.assertEqual(found(self.checkout.root).get("email", []), [])

    def test_a_finding_is_reported_at_its_own_line(self):
        caught = CAUGHT["home-path"]
        self.checkout.write("a.txt", f"one\ntwo\n{caught}\nfour\n")
        self.assertEqual(found(self.checkout.root)["home-path"][0]["line"], 3)

    def test_a_pem_body_spanning_lines_is_one_finding_at_the_line_it_opens_on(self):
        """A key is the only PEM that counts — a bare header is a shape a test
        writes freely, and the body is what makes it a secret."""
        body = "\n".join("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC1" for _ in range(3))
        self.checkout.write("key.txt", f"first\n-----BEGIN PRIVATE KEY-----\n{body}\n")
        self.checkout.write("header.txt", "cat <<EOF\n-----BEGIN OPENSSH PRIVATE KEY-----\nEOF\n")
        hits = found(self.checkout.root)["credential"]
        self.assertEqual([(h["path"], h["line"]) for h in hits], [("key.txt", 2)], hits)

    # --- the machinery a gate lives on -------------------------------------

    def test_only_tracked_files_are_read(self):
        caught = CAUGHT["home-path"]
        self.checkout.write("tracked.txt", f"{caught}\n")
        self.checkout.write("untracked.txt", f"{caught}\n", track=False)
        hits = found(self.checkout.root)["home-path"]
        self.assertEqual([h["path"] for h in hits], ["tracked.txt"], hits)

    def test_a_file_that_is_not_text_is_skipped_rather_than_crashing(self):
        caught = CAUGHT["home-path"]
        self.checkout.write("icon.bin", b"\x89PNG\r\n\x1a\n\xff\xfe" + caught.encode() + b"\xff")
        self.checkout.write("plain.txt", f"{caught}\n")
        done = run(self.checkout.root)
        self.assertEqual(done.returncode, 1, done.stdout + done.stderr)
        hits = json.loads(run(self.checkout.root, "--json").stdout)["home-path"]
        self.assertEqual([h["path"] for h in hits], ["plain.txt"], hits)

    def test_a_rows_skip_holds_and_belongs_to_that_row_alone(self):
        """`ui/vendor/` is third-party text: the rows about people do not read
        it, and the rows about machines still do."""
        self.checkout.write(
            "ui/vendor/bundle.js", f"{CAUGHT['email']}\n{CAUGHT['private-ip']}\n"
        )
        hits = found(self.checkout.root)
        self.assertEqual(hits["email"], [])
        self.assertEqual([h["path"] for h in hits["private-ip"]], ["ui/vendor/bundle.js"])

    def test_a_waiver_holds_on_its_own_line_and_the_line_above(self):
        caught = CAUGHT["home-path"]
        self.checkout.write(
            "waived.txt",
            f"{caught}  # pii-scan: allow home-path — why\n"
            f"# pii-scan: allow home-path — why\n{caught}\n",
        )
        self.assertEqual(found(self.checkout.root)["home-path"], [])

    def test_a_waiver_speaks_only_for_the_category_it_names(self):
        caught = CAUGHT["home-path"]
        self.checkout.write("waived.txt", f"{caught}  # pii-scan: allow email — wrong row\n")
        self.assertEqual(len(found(self.checkout.root)["home-path"]), 1)

    # --- what the recipe sees ----------------------------------------------

    def test_a_clean_checkout_is_rc_0_and_a_dirty_one_is_rc_1(self):
        caught, allowed = CAUGHT["home-path"], ALLOWED["home-path"]
        self.checkout.write("clean.txt", f"{allowed}\n")
        self.assertEqual(run(self.checkout.root).returncode, 0)
        self.checkout.write("dirty.txt", f"{caught}\n")
        done = run(self.checkout.root)
        self.assertEqual(done.returncode, 1)
        self.assertIn("dirty.txt", done.stdout)
        self.assertIn("refused", done.stdout)

    def test_the_count_table_is_printed_even_when_nothing_is_found(self):
        done = run(self.checkout.root)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        for category in pii_scan.categories():
            self.assertRegex(done.stdout, rf"(?m)^{re.escape(category)}\s+0\s+0$")
        self.assertRegex(done.stdout, r"(?m)^total\s+0\s+0$")

    def test_quiet_prints_the_counts_without_the_lines(self):
        caught = CAUGHT["home-path"]
        self.checkout.write("dirty.txt", f"{caught}\n")
        done = run(self.checkout.root, "--quiet")
        self.assertEqual(done.returncode, 1)
        self.assertNotIn("dirty.txt", done.stdout)
        self.assertRegex(done.stdout, r"(?m)^home-path\s+1\s+1$")

    def test_the_table_names_every_row_and_reads_nothing(self):
        done = run(self.checkout.root, "--table")
        self.assertEqual(done.returncode, 0, done.stderr)
        for category in pii_scan.categories():
            self.assertIn(f"{category}:", done.stdout)
        self.assertIn("ui/vendor/", done.stdout)
        self.assertIn("pii-scan", done.stdout)

    # --- the words that left ------------------------------------------------

    def test_a_banished_word_is_caught_whole_and_its_neighbours_are_not(self):
        """The row carries digests, so this case brings its own word: the
        mechanism is pinned without any real name being written down here."""
        word = "gizmo7"
        table = ((len(word), hashlib.sha256(word.encode()).hexdigest()),)
        self.checkout.write("caught.txt", f"a path /repos/{word}/x and a host {word.upper()}@h\n")
        self.checkout.write("near.txt", "gizmo8 gizmos gizmo7x xgizmo7\n")
        held = pii_scan.BANISHED
        pii_scan.BANISHED = table
        try:
            hits = pii_scan.scan(self.checkout.root)[pii_scan.BANISHED_NAME]
        finally:
            pii_scan.BANISHED = held
        self.assertEqual(
            sorted((h["path"], h["text"]) for h in hits),
            sorted([("caught.txt", word), ("caught.txt", word.upper())]),
            f"a whole word in either case, and nothing welded to one: {hits}",
        )

    def test_the_shipped_row_carries_digests_and_never_a_word(self):
        """A digest is one-way; a word pasted here would publish the very name
        the row exists to keep out."""
        self.assertTrue(pii_scan.BANISHED, "the row is empty")
        for length, digest in pii_scan.BANISHED:
            self.assertRegex(digest, r"^[0-9a-f]{64}$")
            self.assertGreaterEqual(length, 3)
        self.assertNotIn(
            pii_scan.BANISHED_NAME,
            [rule["name"] for rule in pii_scan.RULES],
            "the banished row is reported beside the regex rows, not inside them",
        )

    def test_a_banished_word_can_be_waived_at_its_site(self):
        word = "gizmo7"
        table = ((len(word), hashlib.sha256(word.encode()).hexdigest()),)
        self.checkout.write(
            "waived.txt", f"{word}  # pii-scan: allow {pii_scan.BANISHED_NAME} — why\n"
        )
        held = pii_scan.BANISHED
        pii_scan.BANISHED = table
        try:
            hits = pii_scan.scan(self.checkout.root)[pii_scan.BANISHED_NAME]
        finally:
            pii_scan.BANISHED = held
        self.assertEqual(hits, [])


if __name__ == "__main__":
    unittest.main(verbosity=1)
