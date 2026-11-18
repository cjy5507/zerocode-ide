#!/usr/bin/env python3
"""Contract for `tools/release/pii-scan.py`, the re-entry gate for private values.

Every case runs the real script over a throwaway git repository built here, so
each row of the scanner's table is pinned from both sides: the value it must
catch and the placeholder it must forgive. The rest is the machinery a gate
lives or dies by — only tracked files are read, a binary is not read at all, a
row's `skip` holds, a waiver at the site holds, and the exit code is 1 while
anything is found.

The caught values below are fictional and belong to nobody; each one carries
its own waiver so this file does not trip the very gate it tests. The account
row reads the machine it runs on, so its cases swap that reader for a fictional
account in-process: none of them reads, writes or prints a real one.

Run: python3 tools/release/tests/test_pii_scan.py   (stdlib only)
"""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

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


def account(*names):
    """The account row's reader, swapped for fictional accounts while a case runs."""
    return mock.patch.object(pii_scan, "machine_accounts", return_value=names)


def printed(*argv):
    """What the script prints and returns, run in-process (so a swapped reader holds)."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = pii_scan.main(list(argv))
    return code, out.getvalue()


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

    def test_an_installed_app_client_secret_is_a_credential_and_its_placeholder_is_not(self):
        """Google's installed-app OAuth client secret (`GOCSPX-` and a body) left
        the source once (audit 40-26): the release lane builds it in. A whole one
        is caught as a credential wherever it comes back; the prefix alone — the
        lane counts it in a built binary — and a placeholder that says so are not.
        The caught value is assembled here, so this file carries no whole one."""
        prefix = "GOCSPX" + "-"
        caught = prefix + "Qz7Rk2Wm9Lp4" * 3
        placeholder = prefix + "example" * 4
        self.checkout.write("caught.txt", f"client_secret = {caught}\n")
        self.checkout.write("placeholder.txt", f"client_secret = {placeholder}\n")
        self.checkout.write("prefix.sh", f"grep -c '{prefix}' zo\n")
        hits = found(self.checkout.root)
        self.assertEqual(
            [(h["path"], h["text"]) for h in hits["credential"]],
            [("caught.txt", caught)],
            hits,
        )
        self.assertEqual(sum(len(v) for v in hits.values()), 1, hits)

    def test_the_table_is_the_shared_json_and_the_script_names_no_value_itself(self):
        """One table, two readers (t-32798): the gate reads `pii-rules.json`,
        and so does `private_data.rs`, the mask every kept worker hand-in passes
        through — so what this gate refuses and what the keeper masks cannot
        drift apart. The script carries the loader and none of the patterns."""
        source = SCAN.read_text(encoding="utf-8")
        shared = REPO / "tools" / "release" / "pii-rules.json"
        self.assertIn("pii-rules.json", source, "the gate does not read the shared table")
        rows = json.loads(shared.read_text(encoding="utf-8"))["rules"]
        for row in rows:
            self.assertNotIn(row["find"], source, f"{row['name']}: the pattern is copied into the script")
        self.assertEqual(
            [row["name"] for row in pii_scan.load_rules()],
            [row["name"] for row in rows],
            "the gate's rows are not the shared table's, in its order",
        )
        self.assertEqual(pii_scan.RULES, pii_scan.load_rules())
        # Every row carries what the Rust reader needs, and nothing the two
        # regex dialects read differently.
        for row in rows:
            for key in ("name", "says", "find", "allow", "skip", "why"):
                self.assertIn(key, row, f"{row.get('name')}: missing `{key}`")
            for dialect_trap in ("(?<", "(?P<", "\\1", "(?="):
                self.assertNotIn(dialect_trap, row["find"] + row["allow"], f"{row['name']}: {dialect_trap}")

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

    def test_a_home_directory_is_caught_without_its_leading_separator(self):
        """A comment that quotes what a person pasted may drop the first
        separator — the path after a backtick or a quote. The account in it
        is as real as in the whole path; a word that merely ends in `Users`
        is no home directory, and the placeholder is forgiven here too."""
        name = CAUGHT["home-path"].rsplit("/", 1)[-1]
        self.checkout.write(
            "a.rs",
            f"/// a command named `Users/{name}/Desktop/web.config`\n"
            f'let q = "Users\\\\{name}\\\\y";\n'
            "/// a command named `Users/dev/Desktop/web.config`\n"
            f"let all = AllUsers/{name};\n",
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

    # --- this machine's own account ------------------------------------------

    def test_the_machines_account_is_caught_as_a_whole_word_in_any_case(self):
        """Audit 29-04 found the account as a bare word in prose (`… and <name>
        as the uid's own account`), where no shape row looks. The row finds it
        as a whole word wherever it stands — prose, a quoted value, any case,
        beside a mailbox's `@`, between path separators, joined by `-` or `_` —
        and a letter or a digit on either side makes it another word."""
        self.checkout.write(
            "caught.txt",
            "the uid's own account was acme\n"
            'SshEndpoint::new("h.example.test", 22, "Acme")\n'
            "ACME@example.com /srv/acme/x acme-kim acme_home\n",
        )
        self.checkout.write("edges.txt", "acme opens the file and closes it: acme")
        self.checkout.write("near.txt", "acmes xacme acme7 9acme acmeCorp\n")
        with account("acme"):
            hits = pii_scan.scan(self.checkout.root)
        self.assertEqual(
            sorted((h["path"], h["line"]) for h in hits[pii_scan.ACCOUNT_NAME]),
            [("caught.txt", 1), ("caught.txt", 2)] + [("caught.txt", 3)] * 4 + [("edges.txt", 1)] * 2,
            hits,
        )
        self.assertEqual(sum(len(v) for v in hits.values()), 8, f"no other row reads these lines: {hits}")

    def test_the_account_is_printed_masked_in_every_output(self):
        """The gate's lines land in gate logs, the release lane's log and a
        worker's hand-in: a finding of this row says where it stands, never what
        it is — in the lines, in `--json`, in `--quiet` and in the refusal."""
        self.checkout.write("a.txt", "one\nthe uid's own account was Acme\n")
        root = str(self.checkout.root)
        with account("acme"):
            said = {mode: printed("--root", root, *mode) for mode in ((), ("--json",), ("--quiet",))}
        for mode, (code, text) in said.items():
            self.assertEqual(code, 1, mode)
            self.assertNotIn("acme", text.lower(), f"{mode} prints the account")
        self.assertIn(f"a.txt:2: {pii_scan.ACCOUNT_SHOWN}", said[()][1])
        self.assertIn("refused", said[()][1])
        self.assertEqual(
            [(h["path"], h["line"], h["text"]) for h in json.loads(said[("--json",)][1])[pii_scan.ACCOUNT_NAME]],
            [("a.txt", 2, pii_scan.ACCOUNT_SHOWN)],
        )
        self.assertRegex(said[("--quiet",)][1], rf"(?m)^{re.escape(pii_scan.ACCOUNT_NAME)}\s+1\s+1$")

    def test_the_table_describes_the_account_row_without_reading_the_account(self):
        """`--table` says what the row reads and forgives; it never asks the
        machine, so it cannot print the account."""
        with mock.patch.object(pii_scan, "machine_accounts", side_effect=AssertionError("read")) as reader:
            code, table = printed("--table")
        self.assertEqual(code, 0)
        self.assertFalse(reader.called, "the table read the account")
        self.assertIn(f"{pii_scan.ACCOUNT_NAME}: ", table)

    def test_a_role_account_or_a_placeholder_is_not_looked_for(self):
        """The CI runners' accounts (`runner` on macOS, `runneradmin` on Windows,
        as the 2026-11-18 verify logs name their home folders) and the
        placeholder vocabulary home-path forgives (`dev`, `user`, a name of one
        or two letters) are words this tree writes on purpose —
        `package-runner-test`, `/Users/dev` — and no person's name."""
        self.checkout.write("words.txt", "package-runner-test runneradmin /Users/dev/x the user pi\n")
        for name in ("runner", "runneradmin", "RUNNER", "dev", "Dev", "user", "pi"):
            with self.subTest(account=name), account(name):
                self.assertEqual(pii_scan.scan(self.checkout.root)[pii_scan.ACCOUNT_NAME], [])

    def test_the_account_can_be_waived_at_its_site_and_vendor_text_is_not_read(self):
        """The row keeps the table's two ways out: a waiver naming it, at the
        site, and the third-party tree the rows about people leave unread."""
        self.checkout.write(
            "waived.txt",
            "acme  # pii-scan: allow home-path — another row's waiver\n"
            "\n"
            f"acme  # pii-scan: allow {pii_scan.ACCOUNT_NAME} — why\n",
        )
        self.checkout.write("ui/vendor/lib.js", "/* by acme */\n")
        with account("acme"):
            hits = pii_scan.scan(self.checkout.root)[pii_scan.ACCOUNT_NAME]
        self.assertEqual([(h["path"], h["line"]) for h in hits], [("waived.txt", 1)], hits)

    def test_the_account_comes_from_the_password_database_not_the_environment(self):
        """A child the window starts has carried `LOGNAME=root` (accounts.rs,
        MEASURED 2026-09-20), and a gate or a measurement points `HOME` at a
        folder of its own: on macOS and Linux the row asks the password database
        for the uid's login name and home folder, each name once. Windows has
        none; there they are `USERNAME` and `USERPROFILE`."""
        decoy = {"USER": "root", "LOGNAME": "root", "HOME": "/tmp/gate-home", "USERNAME": "root", "USERPROFILE": "/tmp/gate-home"}
        # pii-scan: allow home-path — a fictional account the reader is tested with
        for home, expected in (("/Users/acme", ("acme",)), ("/Users/acme-old", ("acme", "acme-old"))):
            record = types.SimpleNamespace(pw_name="acme", pw_dir=home)
            database = types.SimpleNamespace(getpwuid=lambda uid, record=record: record)
            with self.subTest(home=home), mock.patch.object(pii_scan, "pwd", database), \
                    mock.patch.object(pii_scan.os, "getuid", return_value=501, create=True), \
                    mock.patch.dict(os.environ, decoy):
                self.assertEqual(pii_scan.machine_accounts(), expected)
        # pii-scan: allow home-path — a fictional account the reader is tested with
        windows = {"USERNAME": "acme", "USERPROFILE": "C:\\Users\\acme-old", "USER": "root", "HOME": "/tmp/gate-home"}
        with mock.patch.object(pii_scan, "pwd", None), mock.patch.dict(os.environ, windows):
            self.assertEqual(pii_scan.machine_accounts(), ("acme", "acme-old"))


# --- one corpus over every pattern the gate runs -------------------------------
#
# The 2026-11-19 measurement of this gate (t-115074, reports/rust-anything-166)
# counts thirteen patterns it runs: each row's `find` and `allow` (eight),
# home-path's `allow` read without case for the account (`account_forgiven`),
# `TOKEN`, `ACCOUNT_ALLOW`, `account_finder` and `WAIVER`. The corpus gives each
# one a value it takes and one it passes over, and every alternative of every
# `find` a value of its own, so a change that leaves a file or a word unread
# shows here as a finding gone missing, and one that reads too much as a finding
# too many. Every value is fictional, and the caught ones are assembled where the
# corpus is written: this file carries none of them whole.
CORPUS_ACCOUNTS = ("acme", "DEV", "RunnerAdmin", "runner2")  # two looked for; home-path forgives one, ACCOUNT_ALLOW one
CORPUS_WORDS = ("gizmo", "frobozz")  # a banished word of each length the shipped table hashes


def dotted(*octets):
    """An address written from its parts, so this file never spells it whole."""
    return ".".join(str(octet) for octet in octets)


def corpus():
    """The corpus's files, and every finding the gate reads off them, in the order it reports them."""
    home = CAUGHT["home-path"]
    name = home.rsplit("/", 1)[-1]
    domain = CAUGHT["email"].split("@", 1)[1]
    s, b = "/", "\\"
    body = "Qz7Rk2Wm9Lp4"  # twelve characters that spell no word a row's `allow` forgives
    keys = (  # one per alternative of the credential row's `find`, in its order
        "sk-ant-" + "api03-" + body * 2,
        "sk-proj-" + body * 2,
        "gh" + "p_" + body * 3,
        "github_" + "pat_" + body * 5,
        CAUGHT["credential"],
        "xox" + "b-" + body * 2,
        "AI" + "za" + (body * 3)[:35],
        "GOCSPX" + "-" + body * 2,
        "ya29" + "." + body * 3,
    )
    pem_head = "-----BEGIN " + "RSA PRIVATE KEY-----"
    pem_body = "\n".join("MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC1" for _ in range(3))
    files = {
        "account.txt": (
            "the account was acme and ACME@h and /srv/acme/x acme-kim acme_home\n"
            "runner2 ran\n"
            "misses acmes xacme acme7 9acme runner RunnerAdmin dev DEV runner22\n"
        ),
        "home.txt": (
            f"path {home}{s}x\n"
            f"win C:{b}Users{b}{name}{b}y\n"
            f'rust "C:{b}{b}Users{b}{b}{name}"\n'
            f"quoted `Users{s}{name}{s}Desktop`\n"
            f"slug -Users-{name}-proj\n"
            f"placeholders {s}Users{s}dev {b}Users{b}pi Users-you {s}Users{s}dev.local\n"
            f"near {s}Users{s}devops\n"
            f"misses AllUsers{s}{name} Users:{name} {s}Users{s} the Users table\n"
        ),
        "ip.txt": (
            f"hosts {dotted(10, 99, 44, 7)} and {dotted(172, 20, 9, 9)} and {dotted(192, 168, 77, 5)}\n"
            "allowed 10.0.0.5 10.1.2.3 172.16.0.1 192.168.1.20\n"
            f"near {dotted(172, 16, 0, 2)}\n"
            "misses 172.15.0.1 110.99.44.7 10.1.2 version 10.4 192.169.1.1\n"
        ),
        "keys.txt": "".join(f"{key}\n" for key in keys) + f"{pem_head}\n{pem_body}\n" + (
            "short AKIA" + "Q7RVBNMLKJHGFDS\n"
            "header -----BEGIN PRIVATE KEY----- and no body\n"
            "forgiven AKIAIOSFODNN7EXAMPLE gh" + "p_" + "x" * 36 + "\n"
        ),
        "mail.txt": (
            f"write to chief@{domain}\n"
            "allowed one@example.com ONE@EXAMPLE.ORG git@github.com icon@2x.png\n"
            "misses user@localhost a@b @mention name@host.c\n"
        ),
        # No shape row's literal stands in this file: the banished and account rows still read it.
        "plain.txt": "a gizmo by acme\n",
        "ui/vendor/lib.js": f"/* by chief@{domain}, acme, {dotted(10, 99, 44, 8)} */\n",
        "waived.txt": (
            f"{home}  # pii-scan: allow home-path — on its own line\n"
            "# pii-scan:allow  home-path — the line below\n"
            f"{home}\n"
            f"{home}  # pii-scan: allow email — another row\n"
            "# pii-scan: allow home-path — two lines above\n"
            "unrelated\n"
            f"{home}\n"
            f"{home}  # pii-scan allow home-path — no colon\n"
        ),
        "words.txt": (
            "a gizmo here and GIZMO there\n"
            "frobozz/x and 7gizmo and gizmo_x\n"
            + "a" * 32 + "gizmo\n"  # TOKEN takes 32 characters at most: the five after them are a word
            "misses gizmos xgizmo gizmo7 " + "a" * 31 + "gizmo frobozzz\n"
        ),
    }
    expected = [
        ("home-path", "home.txt", 1, home),
        ("home-path", "home.txt", 2, f"{b}Users{b}{name}"),
        ("home-path", "home.txt", 3, f"{b}{b}Users{b}{b}{name}"),
        ("home-path", "home.txt", 4, f"Users{s}{name}"),
        ("home-path", "home.txt", 5, f"Users-{name}"),
        ("home-path", "home.txt", 7, f"{s}Users{s}devops"),
        ("home-path", "waived.txt", 4, home),
        ("home-path", "waived.txt", 7, home),
        ("home-path", "waived.txt", 8, home),
        ("private-ip", "ip.txt", 1, dotted(10, 99, 44, 7)),
        ("private-ip", "ip.txt", 1, dotted(172, 20, 9, 9)),
        ("private-ip", "ip.txt", 1, dotted(192, 168, 77, 5)),
        ("private-ip", "ip.txt", 3, dotted(172, 16, 0, 2)),
        ("private-ip", "ui/vendor/lib.js", 1, dotted(10, 99, 44, 8)),
        ("email", "mail.txt", 1, f"chief@{domain}"),
        # A finding is shown in at most 60 characters: 57 and an ellipsis past that.
        *(("credential", "keys.txt", line, key if len(key) <= 60 else key[:57] + "…") for line, key in enumerate(keys, start=1)),
        ("credential", "keys.txt", len(keys) + 1, f"{pem_head}\\n{pem_body[:25]}…"),
        (pii_scan.BANISHED_NAME, "plain.txt", 1, "gizmo"),
        (pii_scan.BANISHED_NAME, "words.txt", 1, "gizmo"),
        (pii_scan.BANISHED_NAME, "words.txt", 1, "GIZMO"),
        (pii_scan.BANISHED_NAME, "words.txt", 2, "frobozz"),
        (pii_scan.BANISHED_NAME, "words.txt", 2, "gizmo"),
        (pii_scan.BANISHED_NAME, "words.txt", 2, "gizmo"),
        (pii_scan.BANISHED_NAME, "words.txt", 3, "gizmo"),
        *((pii_scan.ACCOUNT_NAME, "account.txt", 1, pii_scan.ACCOUNT_SHOWN),) * 5,
        (pii_scan.ACCOUNT_NAME, "account.txt", 2, pii_scan.ACCOUNT_SHOWN),
        (pii_scan.ACCOUNT_NAME, "plain.txt", 1, pii_scan.ACCOUNT_SHOWN),
    ]
    return files, expected


class Corpus(unittest.TestCase):
    """The corpus above, read by the gate as it stands."""

    def setUp(self):
        self.files, self.expected = corpus()
        self.checkout = Checkout()
        self.addCleanup(self.checkout.close)
        for path, text in self.files.items():
            self.checkout.write(path, text)

    def scan(self):
        """The gate over the corpus, with its fictional accounts and banished words."""
        table = tuple((len(word), hashlib.sha256(word.encode()).hexdigest()) for word in CORPUS_WORDS)
        with account(*CORPUS_ACCOUNTS), mock.patch.object(pii_scan, "BANISHED", table):
            return pii_scan.scan(self.checkout.root)

    def test_every_pattern_takes_its_value_and_passes_over_its_neighbour(self):
        """Each finding the corpus holds, in the gate's order, and not one more."""
        found = self.scan()
        self.assertEqual(
            [(category, hit["path"], hit["line"], hit["text"]) for category in pii_scan.categories() for hit in found[category]],
            self.expected,
        )


if __name__ == "__main__":
    unittest.main(verbosity=1)
