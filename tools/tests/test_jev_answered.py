#!/usr/bin/env python3
"""Contract for `tools/jev-answered/answered.py` — how each Jev seat's
judgments ended, by provider folder and time window, counted straight from
the ledgers' raw rows (t-55423).

It must count a row by its own fields: a judgment only when it carries an
outcome; sent and attempts by `requests`; a memo hit and an outcome decided
before sending apart from what was sent; answered and failures of what was
sent; applied, retried and probed rows; a row naming another folder's
provider, the request-time routes rows name, and a sent judgment naming
neither; every seat and place summed by provider folder and window; and per
file the corrupt and untimed lines beside the sha256 of the very bytes
counted. It prints no part of a row but those words, names windows in UTC,
the versions it was told and a window a version file changed inside,
and leaves the home it reads as it was. Every key, word and folder name is
read from the Rust it mirrors.

Run: python3 tools/tests/test_jev_answered.py   (stdlib only)
"""

from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import os
import plistlib
import stat
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from datetime import datetime
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location("jev_answered", REPO / "tools" / "jev-answered" / "answered.py")
answered = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = answered
_spec.loader.exec_module(answered)
replay = answered.replay

PROVIDER = "judgment-" + "ab" * 32
OTHER = "judgment-" + "cd" * 32


def rust(path: str) -> str:
    return (REPO / path).read_text()


def ms(text: str) -> int:
    return int(datetime.fromisoformat(text).timestamp() * 1000)


def write(path: Path, rows: list) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join((row if isinstance(row, str) else json.dumps(row)) + "\n" for row in rows))


class TheNamesAreTheRusts(unittest.TestCase):
    def test_every_key_is_the_summarys_spelling(self) -> None:
        summary = rust("crates/zerocode-core/src/jev/summary.rs")
        for name, key in [
            ("OUTCOME", answered.OUTCOME_KEY),
            ("REQUESTS", answered.REQUESTS_KEY),
            ("CACHED", answered.CACHED_KEY),
            ("APPLIED", answered.APPLIED_KEY),
            ("MODEL", answered.MODEL_KEY),
            ("CLIENT_ROUTE", answered.CLIENT_ROUTE_KEY),
            ("SERVER_ERROR_CODE", answered.SERVER_ERROR_CODE_KEY),
        ]:
            self.assertIn(f'pub const {name}: LedgerKey = LedgerKey {{\n    canonical: "{key}",\n    also: &[],', summary)
        for name, keys in [("AT", answered.AT_KEYS), ("RUBRIC_VERSION", answered.RUBRIC_KEYS)]:
            canonical, *also = keys
            spelled = ", ".join(f'"{one}"' for one in also)
            self.assertIn(f'pub const {name}: LedgerKey = LedgerKey {{\n    canonical: "{canonical}",\n    also: &[{spelled}],', summary)
        self.assertIn(f'pub const ANSWERED: &str = "{answered.ANSWERED}";', summary)

    def test_the_probe_word_and_the_provider_mark_are_the_products(self) -> None:
        self.assertIn(
            f'const PROBE_NOT_ASKED: &str = "{answered.PROBE_NOT_ASKED}";',
            rust("zo-ide/crates/tools/src/misc_tools/smart_router/decision_shadow.rs"),
        )
        self.assertIn(
            f'model.starts_with(&format!("{{}}{answered.PROVIDER_MARK}", provider.identity()))',
            rust("crates/zerocode-shell/src/typesafe_settings.rs"),
        )

    def test_the_provider_folder_is_the_one_the_evidence_goes_to(self) -> None:
        self.assertIn(
            f'base.join("{replay.PROVIDER_LEDGERS}").join(self.identity())',
            rust("crates/judgment-provider/src/lib.rs"),
        )

    def test_a_route_is_spelled_as_the_provider_spells_its_fingerprint(self) -> None:
        lib = rust("crates/judgment-provider/src/lib.rs")
        self.assertIn('let mut route = String::from("route-");', lib)
        self.assertIn('let _ = write!(route, "{byte:02x}");', lib)
        self.assertTrue(answered.ROUTE.fullmatch("route-" + "0f" * 8))
        self.assertFalse(answered.ROUTE.fullmatch("judgment-" + "0f" * 8))

    def test_the_503_word_is_the_one_both_clients_write(self) -> None:
        self.assertIn(
            f'assert_eq!(token_for(503), "{answered.HTTP_503}");',
            rust("crates/zerocode-shell/src/systemone/tests.rs"),
        )
        self.assertIn(
            f'assert_eq!(SystemOneFailure::Http(503).ledger_token(), "{answered.HTTP_503}");',
            rust("zo-ide/crates/api/src/systemone.rs"),
        )


class TheCount(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.home = Path(self.dir.name) / ".zo"
        window = self.home / replay.WINDOW_LEDGERS
        before, after = ms("2026-10-15T07:00"), ms("2026-10-15T07:30")
        own, other = f"{PROVIDER}:acme/judge", f"{OTHER}:acme/judge"
        self.route = "route-" + "ef" * 8
        self.ledger = window / replay.PROVIDER_LEDGERS / PROVIDER / "command-guard.jsonl"
        write(
            self.ledger,
            [
                {"at": before, "outcome": "answered", "requests": 1, "model": own, "command": "secret words", "rubricVersion": 3},
                {"at": before + 1, "outcome": "answered", "requests": 0, "cached": True, "model": own, "rubricVersion": 3},
                {"at": before + 2, "outcome": "http_503", "requests": 1, "rubricVersion": 3, "clientRoute": "a person's words"},
                {"at": before + 3, "outcome": "http_503", "requests": 2, "rubricVersion": 3},
                {"at": before + 4, "outcome": "timeout", "requests": 1, "rubric_version": 2, "clientRoute": self.route},
                {"at": before + 5, "outcome": "http_503", "requests": 0},
                {"at": before + 6, "outcome": "answered", "requests": 1, "model": other, "applied": True},
                {"at": before + 7, "label": "allow", "agreed": True},
                {"at": before + 8, "outcome": "answered"},
                {"at": after, "outcome": "answered", "requests": 1, "model": own},
                "{not json",
                {"outcome": "answered", "requests": 1},
            ],
        )
        write(window / "command-guard.jsonl", [{"at": before, "outcome": "answered", "requests": 1, "model": "jev-1.13.0"}])
        write(window / "decision-labels.draft.jsonl", [{"words": "a person's prompt"}])
        for slug, outcome in [("Users-dev-a-0123456789abcdef", "answered"), ("Users-dev-b-fedcba9876543210", "http_502")]:
            write(
                self.home / "projects" / slug / Path(*replay.PROJECT_LEDGERS) / replay.PROVIDER_LEDGERS / PROVIDER / "rerank-shadow.jsonl",
                [{"at": before, "outcome": outcome, "requests": 1}],
            )
        write(
            self.home / "projects" / "Users-dev-a-0123456789abcdef" / Path(*replay.PROJECT_LEDGERS) / replay.PROVIDER_LEDGERS / PROVIDER / "decision-shadow.jsonl",
            [
                {"at": before, "outcome": "answered", "requests": 1, "probe": answered.PROBE_NOT_ASKED, "model": own},
                {"at": before + 1, "outcome": "http_503", "requests": 1, "probe": "timeout"},
            ],
        )
        self.windows = [answered.window("before=2026-10-14T19:10..2026-10-15T07:10"), answered.window("after=2026-10-15T07:12..")]

    def tearDown(self) -> None:
        self.dir.cleanup()

    def table(self, projects=()) -> list[dict]:
        tallies, self.reads = answered.count(self.home, self.windows, list(projects))
        return answered.rows_of(tallies, self.windows)

    def test_a_judgment_is_counted_by_its_own_fields(self) -> None:
        rows = {(row["place"], row["seat"], row["folder"], row["window"]): row for row in self.table()}
        held = rows[("window", "command-guard", PROVIDER, "before")]
        self.assertEqual(held["judgments"], 8, "the label row is no judgment; the untimed one falls in no window")
        self.assertEqual((held["sent"], held["attempts"], held["answered"]), (5, 6, 2))
        self.assertEqual(held["share"], 0.4, "answered of what was sent; a memo hit is not sent")
        self.assertEqual(held["unsent"], {"cached": 1, "http_503": 1})
        self.assertEqual((held["noRequests"], held["retried"], held["applied"], held["probed"]), (1, 1, 1, 0))
        self.assertEqual((held["otherProvider"], held["unattributed"]), (1, 2), "a sent failure naming no route and no provider")
        self.assertEqual(held["routes"], {self.route: 1}, "a value that is not a route is none")
        self.assertEqual(list(held["failures"].items()), [("http_503", 2), ("timeout", 1)], "most first")
        self.assertEqual(held["rubrics"], [2, 3], "both spellings of the rubric")
        self.assertEqual(rows[("window", "command-guard", PROVIDER, "after")]["answered"], 1)
        jev = rows[("window", "command-guard", answered.JEV_FOLDER, "before")]
        self.assertEqual((jev["answered"], jev["unattributed"]), (1, 0), "in Jev's own folder the model names Jev")
        projects = rows[("projects", "rerank-shadow", PROVIDER, "before")]
        self.assertEqual((projects["sent"], projects["answered"]), (2, 1), "every project's folder is summed")
        self.assertEqual(rows[("projects", "decision-shadow", PROVIDER, "before")]["probed"], 1, "not_asked asked nothing")
        self.assertNotIn("decision-labels.draft", {key[1] for key in rows}, "a file with no judgment is no seat")

    def test_each_file_is_named_by_the_hash_of_the_bytes_counted(self) -> None:
        self.table()
        read = next(one for one in self.reads if one.path == str(self.ledger.relative_to(self.home)))
        data = self.ledger.read_bytes()
        self.assertEqual((read.bytes, read.sha256), (len(data), hashlib.sha256(data).hexdigest()))
        self.assertEqual((read.lines, read.corrupt, read.untimed), (12, 1, 1))

    def test_a_model_tier_row_written_with_atMs_counts_in_its_window(self) -> None:
        tiers = self.home / "projects" / "Users-dev-a-0123456789abcdef" / Path(*replay.PROJECT_LEDGERS) / replay.PROVIDER_LEDGERS / PROVIDER / "model-tiers.jsonl"
        at, own = ms("2026-10-15T07:00"), f"{PROVIDER}:acme/judge"
        write(
            tiers,
            [
                {"atMs": at, "provider": PROVIDER, "outcome": "answered", "judged": 3, "requests": 1, "model": own},
                {"at": at + 1, "provider": PROVIDER, "outcome": "answered", "judged": 3, "requests": 1, "model": own},
            ],
        )
        held = {(row["seat"], row["window"]): row for row in self.table()}[("model-tiers", "before")]
        self.assertEqual((held["judgments"], held["answered"]), (2, 2), "the old spelling of the time is still its time")
        read = next(one for one in self.reads if one.path.endswith("model-tiers.jsonl"))
        self.assertEqual((read.lines, read.untimed), (2, 0))

    def test_a_row_takes_the_version_its_writer_ran_when_it_was_written(self) -> None:
        switch, opened = ms("2026-10-15T07:00") + 4, ms("2026-10-14T19:10")
        out, kept = io.StringIO(), Path(self.dir.name) / "count.json"
        with redirect_stdout(out):
            answered.main(
                ["--home", str(self.home), "--window", "before=2026-10-14T19:10..2026-10-15T07:10", "--window", "after=2026-10-15T07:12..",
                 "--ran", f"window=1.1.66@{opened}..{switch}", "--ran", f"window=1.1.67@{switch}..",
                 "--ran", f"zo=1.1.66@{opened}..", "--ran", f"zo=1.1.67@{ms('2026-10-15T06:00')}..", "--json", str(kept)]
            )
        rows = {(row["place"], row["seat"], row["folder"], row["window"]): row for row in json.loads(kept.read_text())["rows"]}
        self.assertEqual(rows[("window", "command-guard", PROVIDER, "before")]["ran"], {"1.1.66": 4, "1.1.67": 4}, "a row at the switch is the new process's")
        self.assertEqual(rows[("window", "command-guard", PROVIDER, "after")]["ran"], {"1.1.67": 1})
        self.assertEqual(rows[("projects", "rerank-shadow", PROVIDER, "before")]["ran"], {answered.UNKNOWN_VERSION: 2}, "two zo versions ran at once")
        self.assertIn("| 1.1.66 4 · 1.1.67 4 |", out.getvalue())
        self.assertIn(f"window 1.1.67 {answered.utc(switch)}..open", out.getvalue())
        tallies, _ = answered.count(self.home, self.windows)
        self.assertEqual(dict(tallies[("window", "command-guard", PROVIDER, "before")].ran), {answered.UNKNOWN_VERSION: 8}, "an installed version names no row")
        with self.assertRaises(SystemExit):
            answered.ran("person=1.1.67@2026-10-15T07:00..")

    def test_the_table_says_counts_and_words_and_nothing_else_of_a_row(self) -> None:
        write(
            self.home / replay.WINDOW_LEDGERS / "mail-triage.jsonl",
            [{"at": ms("2026-10-15T07:00"), "outcome": "refused: the person's words here", "requests": 1}],
        )
        rows = self.table()
        changed = answered.utc(ms("2026-10-15T07:20"))
        counted = {"countedAt": "2026-10-15T00:00:00Z", "home": "/Users/dev/.zo", "versions": {"app": {"version": "9.9.9", "modified": changed}, "zo": None}}
        text = answered.render(rows, answered.totals_of(rows, self.windows), self.reads, self.windows, counted)
        self.assertIn(
            f"| command-guard | window | judgment-{'ab' * 4} | before | 2,3 | 8 | 5 | 6 | 2 | 40.0% | 1 "
            f"| cached 1 · http_503 1 · no requests count 1 | 1 | 0 | 1 | 2 | route-{'ef' * 4} 1 | http_503 2 · timeout 1 |",
            text,
        )
        self.assertIn(
            f"| judgment-{'ab' * 4} | before | 12 | 9 | 10 | 4 | 44.4% | cached 1 · http_503 1 · no requests count 1 "
            "| http_503 3 · http_502 1 · timeout 1 |",
            text,
            "every seat and place summed",
        )
        self.assertIn(f"| mail-triage | window | {answered.JEV_FOLDER} | before | — | 1 | 1 | 1 | 0 | 0.0% |", text)
        self.assertIn(f"{answered.NOT_A_WORD} 1", text)
        self.assertNotIn("secret", text)
        self.assertNotIn("person's", text)
        start, end = answered.utc(ms("2026-10-14T19:10")), answered.utc(ms("2026-10-15T07:10"))
        self.assertIn(
            f"Windows (UTC): before {start}..{end} · after {answered.utc(ms('2026-10-15T07:12'))}..open [app file changed inside]",
            text,
        )
        self.assertIn(f"· app 9.9.9 (file changed {changed}) · zo not named", text)
        data = self.ledger.read_bytes()
        self.assertIn(f"| {self.ledger.relative_to(self.home)} | {len(data):,} | 12 | 1 | 1 | {hashlib.sha256(data).hexdigest()[:answered.SHA_DIGITS]} |", text)
        places = [line.split(" | ")[1] for line in text.splitlines() if line.startswith(f"| {self.ledger.stem} | ") or line.startswith("| rerank-shadow | ")]
        self.assertEqual(places, sorted(places, key=lambda place: place != answered.WINDOW_PLACE), "the window's rows first")
        self.assertEqual(set(places), {answered.WINDOW_PLACE, answered.PROJECTS_PLACE})

    def test_a_503_is_split_by_the_servers_code_and_one_naming_none_is_never_busy(self) -> None:
        at = ms("2026-10-15T07:40")
        write(
            self.home / replay.WINDOW_LEDGERS / replay.PROVIDER_LEDGERS / PROVIDER / "notify-call.jsonl",
            [
                {"at": at, "outcome": answered.HTTP_503, "requests": 1, "serverErrorCode": "operation_busy"},
                {"at": at + 1, "outcome": answered.HTTP_503, "requests": 1, "serverErrorCode": "operation_busy"},
                {"at": at + 2, "outcome": answered.HTTP_503, "requests": 1, "serverErrorCode": "server_stopped"},
                {"at": at + 3, "outcome": answered.HTTP_503, "requests": 1},
                {"at": at + 4, "outcome": answered.HTTP_503, "requests": 1, "serverErrorCode": "the person's words"},
                {"at": at + 5, "outcome": answered.HTTP_503, "requests": 0, "serverErrorCode": "operation_busy"},
                {"at": at + 6, "outcome": "timeout", "requests": 1, "serverErrorCode": "operation_busy"},
            ],
        )
        rows = self.table()
        held = next(row for row in rows if (row["seat"], row["window"]) == ("notify-call", "after"))
        self.assertEqual(held["failures"], {answered.HTTP_503: 5, "timeout": 1}, "the 503 total stands as it was")
        coded = {"operation_busy": 2, "server_stopped": 1, answered.NO_CODE: 1, answered.NOT_A_WORD: 1}
        self.assertEqual(held["codes503"], coded, "sent 503s only; a row naming no code is none, never busy")
        old = next(row for row in rows if (row["seat"], row["folder"], row["window"]) == ("command-guard", PROVIDER, "before"))
        self.assertEqual(old["codes503"], {answered.NO_CODE: 2}, "rows written before the code read as none")
        totals = answered.totals_of(rows, self.windows)
        self.assertEqual(next(one for one in totals if (one["folder"], one["window"]) == (PROVIDER, "after"))["codes503"], coded)
        counted = {"countedAt": "2026-10-15T00:00:00Z", "home": "/Users/dev/.zo", "versions": {"app": None, "zo": None}}
        text = answered.render(rows, totals, self.reads, self.windows, counted)
        self.assertIn(f"| http_503 5 · timeout 1 | operation_busy 2 · {answered.NO_CODE} 1 · {answered.NOT_A_WORD} 1 · server_stopped 1 |", text)
        self.assertIn(answered.CODE_LIMIT, text)
        self.assertNotIn("person's", text)

    def test_a_named_project_is_read_alone_by_zos_slug_and_an_unknown_one_is_refused(self) -> None:
        named = self.table([Path("/Users/dev/a")])
        projects = sorted((row["seat"], row["sent"], row["answered"]) for row in named if row["place"] == answered.PROJECTS_PLACE)
        self.assertEqual(projects, [("decision-shadow", 2, 1), ("rerank-shadow", 1, 1)])
        self.assertTrue(any(row["place"] == answered.WINDOW_PLACE for row in named), "the window's folder is read still")
        with self.assertRaises(SystemExit):
            answered.count(self.home, self.windows, [Path("/Users/dev/elsewhere")])

    def test_a_window_holds_its_start_and_not_its_end_and_is_said_in_utc(self) -> None:
        one = answered.window("w=2026-10-15T07:00..2026-10-15T08:00")
        self.assertTrue(one.holds(ms("2026-10-15T07:00")))
        self.assertFalse(one.holds(ms("2026-10-15T08:00")))
        self.assertEqual(answered.window("w=1000..").end, None)
        self.assertEqual(answered.window("w=..1000"), answered.Window("w", None, 1000))
        zoned = answered.moment("2026-10-15T09:00:00+09:00")
        self.assertEqual(zoned, answered.moment("2026-10-15T00:00:00Z"))
        self.assertEqual(answered.utc(zoned), "2026-10-15T00:00:00Z")
        for bad in ["no-equals", "w=2026-10-15", "w=yesterday..today"]:
            with self.assertRaises(SystemExit):
                answered.window(bad)


class TheHomeIsOnlyRead(unittest.TestCase):
    def test_nothing_under_the_home_changes_and_only_the_named_file_is_written(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            home = Path(root) / ".zo"
            write(home / replay.WINDOW_LEDGERS / replay.PROVIDER_LEDGERS / PROVIDER / "notify-call.jsonl", [{"at": 5, "outcome": "answered", "requests": 1}])
            app = Path(root) / "Acme.app"
            app.joinpath(*answered.APP_INFO).parent.mkdir(parents=True)
            with app.joinpath(*answered.APP_INFO).open("wb") as handle:
                plistlib.dump({answered.APP_VERSION_KEY: "9.9.9"}, handle)
            zo = Path(root) / "zo"
            zo.write_text("#!/bin/sh\necho 'zo 9.9.9'\n")
            zo.chmod(zo.stat().st_mode | stat.S_IXUSR)
            before = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in home.rglob("*") if path.is_file()}
            out = Path(root) / "out"
            out.mkdir()
            with redirect_stdout(io.StringIO()) as printed:
                self.assertEqual(answered.main(["--home", str(home), "--app", str(app), "--zo", str(zo), "--json", str(out / "rows.json")]), 0)
            after = {path: (path.read_bytes(), path.stat().st_mtime_ns) for path in home.rglob("*") if path.is_file()}
            self.assertEqual(before, after, "the home changed")
            self.assertEqual(os.listdir(out), ["rows.json"])
            kept = json.loads((out / "rows.json").read_text())
            self.assertEqual({name: seen["version"] for name, seen in kept["versions"].items()}, {"app": "9.9.9", "zo": "zo 9.9.9"})
            self.assertEqual(kept["versions"]["zo"]["modified"], answered.utc(int(zo.stat().st_mtime * 1000)))
            self.assertEqual([(row["seat"], row["window"], row["share"]) for row in kept["rows"]], [("notify-call", answered.ALL_WINDOW, 1.0)])
            self.assertEqual([(total["folder"], total["judgments"], total["sent"], total["answered"]) for total in kept["totals"]], [(PROVIDER, 1, 1, 1)])
            self.assertEqual([(one["lines"], one["corrupt"], one["untimed"]) for one in kept["files"]], [(1, 0, 0)])
            self.assertEqual(
                kept["windows"],
                [{"label": answered.ALL_WINDOW, "start": None, "end": None, "startUtc": None, "endUtc": None, "changedInside": ["app", "zo"]}],
            )
            self.assertIn("| notify-call | window |", printed.getvalue())


if __name__ == "__main__":
    unittest.main()
