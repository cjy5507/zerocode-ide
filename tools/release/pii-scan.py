#!/usr/bin/env python3
"""The re-entry gate for private values: reads every tracked file and refuses
the ones that name a real person, machine, office or credential.

This repository publishes a release snapshot, so a value that points at the
machine it was written on — a home directory, an office address, a colleague's
mailbox, a live key — must not be in the tree. Finding them once is a sweep;
keeping them out is this file. `just pii-check` runs it, root `verify` includes
it, and it exits 1 while it holds a finding.

Everything it knows is the table in `pii-rules.json`, beside this file: one row
per category, each row carrying what it finds, what it forgives and which paths
it does not read. Two rows are names, not shapes, and only this gate reads them,
so they stand below the loader: the words this tree was cleared of (`BANISHED`,
digests) and this machine's own account (read at each run, written nowhere).
Nothing else in this file names a value — a new exception is a new entry in
that row's `allow`, never a branch somewhere further down. `--table` prints it.

A single site that cannot be rewritten says so where it stands, by carrying
`pii-scan: allow <category> — <why>` on its own line or the line above it. A
waiver is visible in the diff that adds it; a silent branch in here would not
be.

    tools/release/pii-scan.py [--root DIR] [--json] [--table] [--quiet]

Run: python3 tools/release/pii-scan.py   (stdlib only)
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path, PurePath, PureWindowsPath

try:
    import pwd  # the password database; Windows has none
except ImportError:
    pwd = None

# --- the table -------------------------------------------------------------
#
# `find`  what the category looks like, anywhere in the file's text.
# `allow` a full match against the found text that forgives it: the placeholder
#         vocabulary this repository already writes, and the domains the RFCs
#         reserve for documentation (RFC 2606/6761 `.test` `.example`
#         `.invalid` `.local`). The RFC 5737 documentation addresses
#         (192.0.2/198.51.100/203.0.113) need no entry — they are not private,
#         so `find` never reaches them, and that is what a rewritten office
#         address should become.
# `skip`  paths the row does not read, each with its reason. A third-party tree
#         carries its authors' real addresses on purpose and is not ours to
#         rewrite, so the rows about people skip `ui/vendor/`.
#
# The table lives in `pii-rules.json`, beside this file, because it has two
# readers: this gate, over tracked files, and `private_data.rs` in the core
# crate, over the copy of a worker's report the ledger keeps. One file means the
# shape the gate refuses and the shape the keeper masks cannot drift apart.
RULES_FILE = Path(__file__).with_name("pii-rules.json")


def load_rules(path=RULES_FILE):
    """The rows of the table, as the dicts the rest of this file reads."""
    verified = globals().get("VERIFIED_RULES_BYTES")
    table = json.loads(path.read_text(encoding="utf-8") if verified is None else verified)
    return tuple(
        dict(
            name=row["name"],
            says=row["says"],
            find=row["find"],
            allow=row["allow"],
            skip=tuple((prefix, why) for prefix, why in row["skip"]),
        )
        for row in table["rules"]
    )


RULES = load_rules()

# --- the words that left ----------------------------------------------------
#
# A customer's name, a product's, an internal host's: a name has no shape a
# regex can know, so this table carries the SHA-256 of each lowercased word
# instead. The gate refuses the word without this file ever spelling it, and
# the digest is one-way, so publishing the table publishes nothing. `length`
# comes first because only a token of that length is ever hashed, which keeps
# the pass to about two seconds over the whole tree.
#
# It matches a whole word — a name bounded by `/`, `@`, a quote or a space,
# which is how a path, a host or a project name carries one. A name welded
# into a longer identifier is past it; that one is caught by reading the diff.
BANISHED = (
    # (length, sha256 of the lowercased word)
    (5, "f752d2a637e0afd26a4726c9f9ddc8d2df64add8b301f915aff6da24eeb59f20"),
    (7, "996cad1feec66b10a2bf3ae4eed7929808df6e9952d076429cffa78f6eba08d9"),
)
BANISHED_SAYS = "a name this tree was cleared of once — a customer, a product, an internal host"
BANISHED_NAME = "banished-word"
TOKEN = re.compile(r"[A-Za-z][A-Za-z0-9]{2,31}")

# --- the account this machine runs as ----------------------------------------
#
# The person a home directory names goes by the same name without the path: in
# prose (`… and <name> as the uid's own account` — audit 29-04, which commit
# 3e4ce4a97 took out by hand), as a fixture's user, in a branch prefix. No shape
# row sees a bare name, and the account cannot join `BANISHED` either: a short
# word's digest is undone by hashing every word of its length (17,576 lowercase
# words of three letters), so its digest would publish it. So the row reads the
# account from the machine at each run, holds it in memory only, and every
# output says `ACCOUNT_SHOWN` where it stands.
#
# What it reads: the uid's login name and its home folder's name, from the
# password database — not `USER`, `LOGNAME` or `HOME`: a child the window starts
# has carried `LOGNAME=root` (accounts.rs, MEASURED 2026-09-20), and a gate or a
# measurement points `HOME` at a folder of its own. Windows has no password
# database; there the two are `USERNAME` and the last part of `USERPROFILE`.
# git's `user.name` and `user.email` are not read: on the machine this tree is
# published from they are the public repository's owner, which the updater
# feed, the installer and LICENSE name on purpose (audit 12-39), and a whole
# address is the email row's already.
#
# It finds the account as a whole word in any case: a letter or a digit on
# either side makes another word (`<name>s`, `x<name>`, `<name>7`), while `-`,
# `_`, `.`, `/`, `@`, a quote and a space only separate one.
#
# `allow` fully matches an account to forgive it, and a forgiven account is not
# looked for at all, since every hit would be that same word: the CI runners'
# accounts (`runner` on macOS, `runneradmin` on Windows, as the 2026-11-18
# verify logs name their home folders), which this tree writes as a word
# (`package-runner-test`). So is every name home-path forgives as a home folder
# (`dev`, `user`, one or two letters): that is the placeholder vocabulary,
# written on purpose and no person's.
ACCOUNT_NAME = "account-name"
ACCOUNT_SAYS = "this machine's own account — its login name, its home folder's name — as a whole word"
ACCOUNT_ALLOW = re.compile(r"(?i)(?:runner|runneradmin)")
ACCOUNT_SKIP = (("ui/vendor/", "third-party bundles name their own authors"),)
ACCOUNT_SHOWN = "[this machine's account name]"

# A site that cannot be rewritten waives itself, on its own line or the one above.
WAIVER = re.compile(r"pii-scan:\s*allow\s+([a-z-]+)")

MAX_PRINTED = 40  # per category, so a first run stays readable


def categories():
    """Every category the gate reports, in the order it reports them."""
    return [rule["name"] for rule in RULES] + [BANISHED_NAME, ACCOUNT_NAME]


def rows():
    """The regex table, compiled once."""
    for rule in RULES:
        yield (
            rule["name"],
            rule["says"],
            re.compile(rule["find"]),
            re.compile(rule["allow"]),
            rule["skip"],
        )


def banished_words(text):
    """Every whole word in `text` whose digest is in `BANISHED`, with its offset."""
    lengths = {length for length, _digest in BANISHED}
    digests = {digest for _length, digest in BANISHED}
    for hit in TOKEN.finditer(text):
        word = hit.group(0)
        if len(word) not in lengths:
            continue
        if hashlib.sha256(word.lower().encode()).hexdigest() in digests:
            yield hit


def machine_accounts():
    """This machine's account names, each once, as the system spells them."""
    if pwd is not None:
        record = pwd.getpwuid(os.getuid())
        said = (record.pw_name, PurePath(record.pw_dir).name)
    else:
        said = (os.environ.get("USERNAME", ""), PureWindowsPath(os.environ.get("USERPROFILE", "")).name)
    names = []
    for name in said:
        if name and name.lower() not in {held.lower() for held in names}:
            names.append(name)
    return tuple(names)


def account_forgiven(name):
    """A CI runner's account, or a name home-path forgives as a home folder."""
    home = next(rule for rule in RULES if rule["name"] == "home-path")
    return bool(ACCOUNT_ALLOW.fullmatch(name) or re.fullmatch(home["allow"], f"Users/{name}", re.IGNORECASE))


def account_finder(names):
    """The row's `find` for these accounts, or None when it looks for none.

    Each name opens its own alternative, and the check that no letter or digit
    stands before it follows it, as a look-behind over the name itself: a
    pattern that opens on a literal is one the regex engine scans for, which
    took 0.4 s of CPU over this tree's 3,376 text files against 1.4 s for the
    same matches with the look-behind first (2026-11-18)."""
    looked_for = sorted({name for name in names if not account_forgiven(name)}, key=len, reverse=True)
    if not looked_for:
        return None
    either = "|".join(f"{re.escape(name)}(?<![A-Za-z0-9]{re.escape(name)})" for name in looked_for)
    return re.compile(rf"(?:{either})(?![A-Za-z0-9])", re.IGNORECASE)


def tracked(root: Path):
    """Every file git tracks under `root`, in git's own order."""
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"],
        capture_output=True,
        check=True,
    ).stdout
    return [name.decode("utf-8") for name in out.split(b"\0") if name]


def _waived(lines, number, category):
    """True when the finding's own line, or the line above it, waives it."""
    for index in (number - 1, number - 2):
        if 0 <= index < len(lines):
            said = WAIVER.search(lines[index])
            if said and said.group(1) == category:
                return True
    return False


def scan(root: Path):
    """Findings for every row of the table, over every tracked text file.

    A file git tracks that does not decode as UTF-8 is an icon or a binary
    fixture; it holds no line for a person to read, so no row reads it. The
    search runs over the whole text, not line by line, so a PEM body that
    spans lines is one finding reported at the line it opens on.
    """
    table = list(rows())
    account = account_finder(machine_accounts())
    found = {name: [] for name in categories()}
    for name in tracked(root):
        try:
            text = (root / name).read_text(encoding="utf-8")
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        lines = None

        def keep(category, hit, shown=None):
            """One finding, unless the site it stands on waives it."""
            nonlocal lines
            number = text.count("\n", 0, hit.start()) + 1
            if lines is None:
                lines = text.splitlines()
            if _waived(lines, number, category):
                return
            if shown is None:
                said = hit.group(0)
                shown = said if len(said) <= 60 else said[:57] + "…"
            found[category].append(
                {"path": name, "line": number, "text": shown.replace("\n", "\\n")}
            )

        for category, _says, find, allow, skip in table:
            if any(name.startswith(prefix) for prefix, _why in skip):
                continue
            for hit in find.finditer(text):
                if allow.fullmatch(hit.group(0)):
                    continue
                keep(category, hit)
        for hit in banished_words(text):
            keep(BANISHED_NAME, hit)
        if account is not None and not any(name.startswith(prefix) for prefix, _why in ACCOUNT_SKIP):
            for hit in account.finditer(text):
                keep(ACCOUNT_NAME, hit, shown=ACCOUNT_SHOWN)
    return found


def table_text():
    """What the gate knows, as the table it is."""
    out = []
    for rule in RULES:
        out.append(f"{rule['name']}: {rule['says']}")
        out.append(f"  find  {rule['find']}")
        out.append(f"  allow {rule['allow']}")
        for prefix, why in rule["skip"]:
            out.append(f"  skip  {prefix} — {why}")
    out.append(f"{BANISHED_NAME}: {BANISHED_SAYS}")
    out.append(f"  find  {TOKEN.pattern}, hashed and matched against {len(BANISHED)} digest(s)")
    out.append(f"{ACCOUNT_NAME}: {ACCOUNT_SAYS}")
    out.append(
        "  find  the uid's login name and home folder's name from the password database "
        "(USERNAME and USERPROFILE on Windows), read at each run and printed nowhere, "
        "as a whole word in any case"
    )
    out.append(f"  allow {ACCOUNT_ALLOW.pattern}, and every name home-path forgives as a home folder")
    for prefix, why in ACCOUNT_SKIP:
        out.append(f"  skip  {prefix} — {why}")
    out.append(f"waiver: {WAIVER.pattern} on the finding's line or the one above it")
    return "\n".join(out)


def report(found, quiet=False):
    """The findings, then the count per category. Returns the total."""
    total = 0
    for name in categories():
        hits = found[name]
        total += len(hits)
        if hits and not quiet:
            print(f"\n{name}: {len(hits)} finding(s)")
            for hit in hits[:MAX_PRINTED]:
                print(f"  {hit['path']}:{hit['line']}: {hit['text']}")
            if len(hits) > MAX_PRINTED:
                print(f"  … and {len(hits) - MAX_PRINTED} more")
    print()
    print(f"{'category':<14} {'findings':>8} {'files':>6}")
    for name in categories():
        hits = found[name]
        print(f"{name:<14} {len(hits):>8} {len({h['path'] for h in hits}):>6}")
    print(
        f"{'total':<14} {total:>8} "
        f"{len({h['path'] for hits in found.values() for h in hits}):>6}"
    )
    return total


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=None, help="the checkout to read (default: this one)")
    parser.add_argument("--json", action="store_true", help="print the findings as JSON")
    parser.add_argument("--table", action="store_true", help="print the table and stop")
    parser.add_argument("--quiet", action="store_true", help="print the counts, not the lines")
    args = parser.parse_args(argv)

    if args.table:
        print(table_text())
        return 0

    root = Path(args.root) if args.root else Path(__file__).resolve().parents[2]
    found = scan(root)
    if args.json:
        print(json.dumps(found, indent=2, ensure_ascii=False))
        total = sum(len(hits) for hits in found.values())
    else:
        total = report(found, quiet=args.quiet)
    if total:
        if not args.json:
            print(
                "\nrefused: a published snapshot must name no real person, machine or office.\n"
                "Rewrite the value to the placeholder vocabulary, add the exception to the row's\n"
                "`allow` in tools/release/pii-scan.py, or waive the one site where it stands."
            )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
