"""The long-form bench's own parts, without a browser (t-37883): the server
serves the form but never the card, the oracle fails a run that did nothing
and names every field it scores wrong, and the person-time yardstick is
the sum of its written operators."""
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import klm  # noqa: E402
import oracle  # noqa: E402
import server  # noqa: E402

SPEC = json.loads((HERE / "spec.json").read_text())
FIELDS = [field for section in SPEC["sections"] for field in section["fields"]]
EXPECTED = SPEC["card"]["expected"]


def page_value(field):
    """What the page posts for the card's answer to `field`."""
    value = EXPECTED[field["id"]]
    return {"name": "scan.png", "size": 1} if field["kind"] == "file" else value


def perfect():
    return {field["id"]: page_value(field) for field in FIELDS}


def write_run(folder, submissions=(), handoffs=0):
    folder = pathlib.Path(folder)
    with (folder / oracle.SUBMISSIONS).open("w") as log:
        for fields in submissions:
            log.write(json.dumps({"at_ms": 0, "reference": "ZC-TEST", "fields": fields}) + "\n")
    with (folder / oracle.HANDOFFS).open("w") as log:
        for _ in range(handoffs):
            log.write(json.dumps({"at_ms": 0}) + "\n")


class OracleTests(unittest.TestCase):
    def test_a_run_that_did_nothing_fails(self):
        with tempfile.TemporaryDirectory() as folder:
            result = oracle.score(folder)
        self.assertFalse(result["pass"])
        self.assertEqual(result["submits"], 0)
        self.assertIn("no submission", " ".join(result["reasons"]))

    def test_the_cards_answer_handed_off_once_passes(self):
        with tempfile.TemporaryDirectory() as folder:
            write_run(folder, [perfect()], handoffs=1)
            result = oracle.score(folder)
        self.assertTrue(result["pass"], result["reasons"])
        self.assertEqual(result["ok_fields"], len(FIELDS))

    def test_case_and_spacing_of_typed_words_and_phone_punctuation_do_not_matter(self):
        answer = perfect()
        answer["given_names"] = "  alex   jordan "
        answer["phone"] = "10-5550-0123"
        with tempfile.TemporaryDirectory() as folder:
            write_run(folder, [answer], handoffs=1)
            self.assertTrue(oracle.score(folder)["pass"])

    def test_every_wrong_or_missing_field_is_named(self):
        answer = perfect()
        answer["birth_date"] = "1990-12-03"
        answer["companions"] = 0
        del answer["second_nationality"]
        with tempfile.TemporaryDirectory() as folder:
            write_run(folder, [answer], handoffs=1)
            result = oracle.score(folder)
        self.assertFalse(result["pass"])
        self.assertEqual(result["wrong"], ["birth_date", "companions"])
        self.assertEqual(result["missing"], ["second_nationality"])

    def test_the_last_submission_counts_and_every_submit_is_counted(self):
        wrong = perfect()
        wrong["sex"] = "f"
        with tempfile.TemporaryDirectory() as folder:
            write_run(folder, [wrong, perfect()], handoffs=1)
            result = oracle.score(folder)
        self.assertTrue(result["pass"])
        self.assertEqual(result["submits"], 2)

    def test_the_upload_is_handed_to_the_person_exactly_once(self):
        for handoffs in (0, 2):
            with tempfile.TemporaryDirectory() as folder:
                write_run(folder, [perfect()], handoffs=handoffs)
                result = oracle.score(folder)
            self.assertFalse(result["pass"], handoffs)

    def test_an_unattached_upload_is_wrong(self):
        answer = perfect()
        answer["passport_scan"] = None
        with tempfile.TemporaryDirectory() as folder:
            write_run(folder, [answer], handoffs=1)
            self.assertEqual(oracle.score(folder)["wrong"], ["passport_scan"])


class SpecTests(unittest.TestCase):
    def test_the_card_answers_every_field_and_nothing_else(self):
        self.assertEqual({field["id"] for field in FIELDS}, set(EXPECTED))

    def test_the_form_is_about_forty_fields_of_every_kind_the_task_names(self):
        kinds = {field["kind"] for field in FIELDS}
        self.assertGreaterEqual(len(FIELDS), 40)
        for kind in ("text", "date", "datepick", "select", "combobox", "radio", "counter", "yesno", "checkbox", "file"):
            self.assertIn(kind, kinds)
        self.assertTrue(any(field.get("shows_when") for field in FIELDS), "a field that appears on an answer")

    def test_every_choice_the_card_makes_is_one_of_its_fields_options(self):
        for field in FIELDS:
            if "options" in field:
                codes = [code for code, _label in SPEC["options"][field["options"]]]
                self.assertIn(EXPECTED[field["id"]], codes, field["id"])

    def test_both_renderings_and_the_oracle_know_the_same_kinds(self):
        for page in ("form.js", "phone.js"):
            drawn = (HERE / page).read_text()
            for kind in oracle.READ:
                self.assertRegex(drawn, rf"\b{kind}:", f"{page} draws no {kind}")


class ServerTests(unittest.TestCase):
    def test_the_page_is_served_the_form_and_never_the_card(self):
        with tempfile.TemporaryDirectory() as folder:
            served, url = server.serve(folder)
            try:
                spec = json.loads(urllib.request.urlopen(url + "spec").read())
                page = urllib.request.urlopen(url).read().decode()
            finally:
                served.shutdown()
        self.assertNotIn("card", spec)
        self.assertNotIn(EXPECTED["passport_number"], json.dumps(spec))
        self.assertIn("form.js", page)
        self.assertIn("form-core.js", page)

    def test_a_post_is_recorded_for_the_oracle_and_answered_a_reference(self):
        with tempfile.TemporaryDirectory() as folder:
            served, url = server.serve(folder)
            try:
                body = json.dumps({"fields": perfect()}).encode()
                request = urllib.request.Request(url + "submit", data=body, headers={"content-type": "application/json"})
                answer = json.loads(urllib.request.urlopen(request).read())
            finally:
                served.shutdown()
            rows = oracle.lines(pathlib.Path(folder) / oracle.SUBMISSIONS)
        self.assertTrue(answer["reference"].startswith("ZC-"))
        self.assertEqual(rows[0]["reference"], answer["reference"])
        self.assertEqual(rows[0]["fields"]["family_name"], EXPECTED["family_name"])

    def test_the_server_leaves_when_its_stdin_ends(self):
        """The bench holds the server's stdin: a bench ended by a KILL closes it, and
        the server leaves instead of serving on for ever (t-89544)."""
        with tempfile.TemporaryDirectory() as folder:
            served = subprocess.Popen(
                [sys.executable, str(HERE / "server.py"), "--out", folder, "--until-stdin-closes"],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
            )
            self.addCleanup(lambda: served.poll() is None and (served.kill(), served.wait()))
            said = served.stdout.readline()
            self.assertTrue(said.startswith("listening http://127.0.0.1:"), said)
            served.stdin.close()
            self.assertEqual(served.wait(timeout=15), 0)


class PersonTimeTests(unittest.TestCase):
    def test_the_estimate_is_the_sum_of_its_rows_and_covers_every_field(self):
        result = klm.estimate()
        ids = [row["id"] for row in result["rows"]]
        for field in FIELDS:
            self.assertIn(field["id"], ids)
        self.assertAlmostEqual(result["total_seconds"], sum(row["seconds"] for row in result["rows"]), places=0)

    def test_the_yardstick_is_pinned(self):
        # The number the agent's run is held to; a change to the form or the
        # operators moves it on purpose, here.
        self.assertEqual(klm.estimate()["total_seconds"], 191.2)

    def test_type_ahead_stops_where_the_option_is_alone(self):
        self.assertEqual(klm.shortest_prefix("Mr", ["Mr", "Ms", "Mx", "Dr"]), 2)
        self.assertEqual(klm.shortest_prefix("Canada", ["Cambodia", "Canada", "Chile"]), 3)


@unittest.skipUnless(os.name == "posix" and shutil.which("node") and shutil.which("pgrep"), "a process tree read by parent is a POSIX thing")
class TreeTests(unittest.TestCase):
    """A bench's child ends with everything under it (t-89544). Every process signalled
    here is one this test started; the keeper leads a session of its own, so a group
    signal reaches nothing else."""

    KEEPER = """
import {{ spawnTied, endTree }} from {tree};
const child = spawnTied("sh", ["-c", {command}], {{ stdio: "ignore" }});
if (process.argv[2] === "cap") setTimeout(() => endTree(child, 200), 100);
setInterval(() => {{}}, 1000);
"""

    def scene(self, *words, graceful=None):
        """A keeper whose child has a child; with `graceful`, the child answers a TERM
        by writing that file half a second later, as a test that cleans up does."""
        folder = pathlib.Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, folder, ignore_errors=True)
        top, below = folder / "top.pid", folder / "below.pid"
        trap = f"trap 'sleep 0.5; echo done > {graceful}; exit 0' TERM; " if graceful else ""
        command = (
            f"{trap}echo $$ > {top}; sh -c 'echo $$ > {below}; while :; do sleep 1; done' & wait"
        )
        script = folder / "keeper.mjs"
        script.write_text(self.KEEPER.format(tree=json.dumps(str(HERE / "tree.mjs")), command=json.dumps(command)))
        keeper = subprocess.Popen(["node", str(script), *words], start_new_session=True)
        self.addCleanup(self.end, keeper, top, below)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and not (top.exists() and below.exists() and top.read_text() and below.read_text()):
            time.sleep(0.05)
        return keeper, int(top.read_text()), int(below.read_text())

    @staticmethod
    def end(keeper, *pid_files):
        if keeper.poll() is None:
            keeper.kill()
        keeper.wait()
        for path in pid_files:
            try:
                os.kill(int(path.read_text()), signal.SIGKILL)
            except (OSError, ValueError):
                pass

    @staticmethod
    def alive(pid):
        state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True, check=False)
        return bool(state.stdout.strip()) and not state.stdout.strip().startswith("Z")

    def assert_gone(self, *pids):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline and any(self.alive(pid) for pid in pids):
            time.sleep(0.05)
        self.assertFalse(any(self.alive(pid) for pid in pids), f"still running: {pids}")

    def test_a_signal_to_the_bench_takes_the_tree(self):
        for name in (signal.SIGTERM, signal.SIGALRM, signal.SIGHUP, signal.SIGINT):
            with self.subTest(signal=name.name):
                keeper, top, below = self.scene()
                keeper.send_signal(name)
                keeper.wait(timeout=15)
                self.assert_gone(top, below)

    def test_a_cap_takes_the_tree_where_a_kill_of_the_child_took_one_pid(self):
        _keeper, top, below = self.scene("cap")
        self.assert_gone(top, below)

    def test_whoever_ends_the_benchs_group_reaches_its_children(self):
        """The build line's cap and leftover sweep end a job's group; a child that had
        left for a group of its own was out of their reach."""
        keeper, top, below = self.scene()
        os.killpg(keeper.pid, signal.SIGKILL)
        keeper.wait(timeout=15)
        self.assert_gone(top, below)

    def test_a_signal_gives_the_child_its_grace_before_the_kill(self):
        with tempfile.TemporaryDirectory() as folder:
            cleaned = pathlib.Path(folder).resolve() / "cleaned"
            keeper, top, below = self.scene(graceful=cleaned)
            keeper.send_signal(signal.SIGTERM)
            keeper.wait(timeout=15)
            self.assert_gone(top, below)
            self.assertEqual(cleaned.read_text() if cleaned.exists() else None, "done\n")


if __name__ == "__main__":
    unittest.main()
