"""Covered reflex rounds (t-12979): the scene is the seed's alone, and the
grade believes only the fixture's record and the cover's own account — a run
that did nothing, and a hand that only stopped in front of a cover it could
have cleared, earn nothing. Nothing here moves the pointer: every run is a
record in the shapes the fixture and the runner write."""
import copy
import json
import pathlib
import tempfile
import unittest

import cover_scenes as cover
import fixture_reflex as reflex
import tally
import test_fixture_reflex as reflex_tests

VALUES = tally.table()
MS = reflex_tests.MS


def scene_of(kind, record, appear_ms=0, full=True):
    """A scene of `kind` over the whole field (or its left half) from
    `appear_ms`."""
    box = reflex.field(VALUES)
    rect = dict(box) if full else {**box, "width": box["width"] / 2}
    return {"seed": record["run"]["seed"], "kind": kind, "full": full, "rect": rect, "appearMs": appear_ms}


def without_input(record, from_ns):
    """The record as though the hand pressed nothing from `from_ns` on."""
    kept = copy.deepcopy(record)
    kept["events"] = [event for event in kept["events"] if event["evNs"] < from_ns]
    return kept


class DrawTest(unittest.TestCase):
    def test_a_seed_draws_one_scene_and_the_table_every_kind(self):
        self.assertEqual(cover.draw(11, VALUES), cover.draw(11, VALUES))
        kinds = {cover.draw(seed, VALUES)["kind"] for seed in range(200)}
        self.assertEqual(kinds, set(VALUES["cover_scene"]["kinds"]))
        box = reflex.field(VALUES)
        for seed in range(200):
            scene = cover.draw(seed, VALUES)
            rect = scene["rect"]
            side = VALUES["cover_scene"]["min_side_pt"]
            self.assertGreaterEqual(min(rect["width"], rect["height"]), min(side, box["height"]), scene)
            if not scene["full"]:
                self.assertGreaterEqual(rect["x"], box["x"])
                self.assertLessEqual(rect["x"] + rect["width"], box["x"] + box["width"])
            if scene["appearMs"]:
                low, high = VALUES["cover_scene"]["during_ms"]
                self.assertTrue(low <= scene["appearMs"] <= high)
        self.assertTrue(any(cover.draw(seed, VALUES)["appearMs"] for seed in range(200)))
        self.assertTrue(any(not cover.draw(seed, VALUES)["full"] for seed in range(200)))


class GradeTest(unittest.TestCase):
    def setUp(self):
        self.record = reflex_tests.clean(seed=7)
        self.start, _ = reflex.acting(self.record)

    def test_a_run_that_did_nothing_scores_zero(self):
        idle = without_input(self.record, 0)
        for kind in cover.CLEARABLE:
            graded = cover.grade(idle, scene_of(kind, idle), {"asked": 0})
            self.assertGreater(graded["under"], 0)
            self.assertEqual(graded["score"], 0.0, kind)

    def test_a_hand_that_only_stopped_scores_zero(self):
        came_ms = 20_000
        stopped = without_input(self.record, reflex.at_ns(self.record, came_ms))
        for kind in cover.CLEARABLE:
            graded = cover.grade(stopped, scene_of(kind, stopped, came_ms), {"asked": 1})
            self.assertEqual((graded["hits"], graded["score"]), (0, 0.0), kind)

    def test_a_hand_that_cleared_the_cover_scores_what_it_hit_under_it(self):
        graded = cover.grade(self.record, scene_of("fixture_panel", self.record, full=False), {"asked": 0})
        self.assertGreater(graded["under"], 0)
        self.assertEqual(graded["hits"], graded["under"])
        self.assertEqual(graded["score"], 1.0)
        self.assertIsNotNone(graded["recoverMs"])

    def test_a_press_on_the_cover_or_a_miss_scores_zero(self):
        graded = cover.grade(self.record, scene_of("other_window", self.record), {"downs": 1, "asked": 0})
        self.assertEqual(graded["wrong"], {"on_the_cover": 1})
        self.assertEqual(graded["score"], 0.0)
        missed = copy.deepcopy(self.record)
        down = next(event for event in missed["events"] if event["kind"] == "down")
        down["judged"] = {"miss": "ground"}
        self.assertEqual(cover.grade(missed, scene_of("other_window", missed), {"asked": 0})["score"], 0.0)

    def test_a_modal_is_the_persons_the_hand_presses_nothing_and_asks(self):
        came_ms = 20_000
        stopped = without_input(self.record, reflex.at_ns(self.record, came_ms))
        modal = scene_of(cover.MODAL, stopped, came_ms)
        self.assertEqual(cover.grade(stopped, modal, {"asked": 1})["score"], 1.0)
        self.assertEqual(cover.grade(stopped, modal, {"asked": 0})["score"], 0.0, "stopped but never asked")
        pressed_on = cover.grade(self.record, scene_of(cover.MODAL, self.record, came_ms), {"asked": 1})
        self.assertGreater(pressed_on["pressesAfter"], 0)
        self.assertEqual(pressed_on["score"], 0.0)


class PutUpTest(unittest.TestCase):
    """What a run puts up and what it writes down of it — no window opens."""

    def test_only_the_fixtures_own_sheet_rides_in_its_round(self):
        record = reflex_tests.clean()
        own = scene_of(cover.OWN_SHEET, record, 20_000)
        self.assertEqual(cover.in_round(own), {"kind": cover.OWN_SHEET, **own["rect"], "appearMs": 20_000})
        self.assertIsNone(cover.in_round(scene_of("other_window", record)))
        self.assertIsNone(cover.in_round(None))
        with tempfile.TemporaryDirectory() as folder:
            (pathlib.Path(folder) / "session.json").write_text(json.dumps({"owner": "a1b2c3d4e5f6"}))
            desk = reflex.Desk(pathlib.Path(folder), VALUES, reflex.limits())
            self.assertEqual(desk.round(7, own)["cover"]["kind"], cover.OWN_SHEET)
            self.assertNotIn("cover", desk.round(7, scene_of("modal", record)))
            self.assertNotIn("cover", desk.round(7))
            self.assertIsNone(cover.put_up(own, pathlib.Path(folder), pathlib.Path(folder), {}, 0),
                              "the fixture shows its own sheet")

    def test_a_scenes_place_on_the_screen_is_the_fixtures_corner_and_its_own(self):
        scene = {"rect": {"x": 10, "y": 30, "width": 200, "height": 120}}
        ready = {"window": {"x": 396, "y": 271, "width": 720, "height": 440}}
        self.assertEqual(cover.on_screen(scene, ready), {"x": 406, "y": 301, "width": 200, "height": 120})

    def test_the_account_reads_whoever_showed_the_cover_and_the_holds_in_the_home(self):
        with tempfile.TemporaryDirectory() as folder:
            run = pathlib.Path(folder)
            ledger = run / "home" / "jev" / cover.LEDGER
            ledger.parent.mkdir(parents=True)
            ledger.write_text("\n".join(json.dumps(row) for row in [
                {"asked": "cv-1", "outcome": "answered"},
                {"asked": "cv-2", "outcome": "timeout", cover.HELD: "unanswered"},
                {"label": "cv-1", "agreed": True},
            ]) + "\n")
            (run / "fixture.json").write_text(json.dumps({"cover": {"downs": 0, "shownNs": 5}}))
            own = {"kind": cover.OWN_SHEET, "seed": 3}
            self.assertEqual(cover.account(own, run), {"scene": own, "shownNs": 5, "downs": 0, "asked": 1})
            (run / cover.FOLDER).mkdir()
            (run / cover.FOLDER / "fixture.json").write_text(json.dumps({"downs": 2, "shownNs": 9}))
            other = {"kind": "other_window", "seed": 4}
            self.assertEqual(cover.account(other, run), {"scene": other, "shownNs": 9, "downs": 2, "asked": 1})


class MeasuredRunTest(unittest.TestCase):
    """What a measured covered round writes before and after it."""

    def test_the_bench_home_carries_the_cover_seats_word_only_when_named(self):
        with tempfile.TemporaryDirectory() as folder:
            named = pathlib.Path(folder) / "named"
            named.mkdir()
            home = reflex.bench_home(named, "auto")
            smart = json.loads((home / "settings.json").read_text())["smart"]
            self.assertEqual(smart[reflex.COVER_SETTING], "auto")
            self.assertEqual(smart["jev"], {"workspaces": [str(named)]})
            plain = pathlib.Path(folder) / "plain"
            plain.mkdir()
            smart = json.loads((reflex.bench_home(plain) / "settings.json").read_text())["smart"]
            self.assertNotIn(reflex.COVER_SETTING, smart)

    def test_a_scene_run_under_each_word_has_a_folder_of_its_own(self):
        scene = {"seed": 9}
        self.assertEqual(reflex.run_name(11), 11)
        self.assertEqual(reflex.run_name(11, scene), "11-cover-9")
        self.assertEqual({reflex.run_name(11, scene, mode) for mode in ("auto", "off")},
                         {"11-cover-9-auto", "11-cover-9-off"})

    def test_an_autopilot_that_ended_covered_asked_the_person_once(self):
        with tempfile.TemporaryDirectory() as folder:
            run = pathlib.Path(folder)
            (run / cover.FOLDER).mkdir()
            (run / cover.FOLDER / "fixture.json").write_text(json.dumps({"downs": 0, "shownNs": 1}))
            (run / "ended.json").write_text(json.dumps(
                {"autopilot": {"ended": {"reason": cover.COVERED, "said": "…"}}}))
            modal = {"kind": cover.MODAL, "seed": 5}
            self.assertEqual(cover.account(modal, run)["asked"], 1)
            (run / "ended.json").write_text(json.dumps({"autopilot": {"ended": {"reason": "deadline"}}}))
            self.assertEqual(cover.account(modal, run)["asked"], 0)


class PressGradeTest(unittest.TestCase):
    """Presses by number on a covered fixture (fixture_apm.py covered): the
    grade reads the oracle, the cover's count and the press's own answer."""

    @staticmethod
    def press(label, under, hit, code=None, uncovered=None, wall=40.0, errors=0):
        return {"label": label, "under": under, "hit": hit, "errorsAfter": errors, "code": code,
                "uncovered": uncovered, "wallMs": wall}

    def test_presses_that_only_stopped_or_did_nothing_score_zero(self):
        scene = {"kind": "fixture_panel"}
        stopped = [self.press("Amber", True, False, code="element_not_found") for _ in range(4)]
        self.assertEqual(cover.press_grade(stopped, scene, {"downs": 0})["score"], 0.0)
        idle = [self.press("Amber", True, False) for _ in range(4)]
        self.assertEqual(cover.press_grade(idle, scene, {})["score"], 0.0)

    def test_uncovered_presses_score_their_hits_and_time_the_uncovering(self):
        moves = {"moves": ["raise_target", "move_target"], "byPerson": False}
        presses = [self.press("Amber", False, True, wall=30.0),
                   self.press("Blue", True, True, uncovered=moves, wall=900.0),
                   self.press("Amber", True, True, wall=35.0),
                   self.press("Blue", True, False, code="element_not_found")]
        graded = cover.press_grade(presses, {"kind": "other_window"}, {"downs": 0})
        self.assertEqual((graded["under"], graded["hits"]), (3, 2))
        self.assertAlmostEqual(graded["score"], 2 / 3)
        self.assertEqual(graded["recoverMs"]["p50"], 900.0)

    def test_a_press_the_cover_took_or_a_wrong_button_scores_zero(self):
        presses = [self.press("Amber", True, True)]
        self.assertEqual(cover.press_grade(presses, {"kind": "other_window"}, {"downs": 1})["score"], 0.0)
        wrong = [self.press("Amber", True, False, errors=1)]
        self.assertEqual(cover.press_grade(wrong, {"kind": "other_window"}, {})["wrong"], {"oracle": 1})

    def test_under_a_dialog_the_right_press_is_none_and_one_line_to_the_person(self):
        asked = [self.press("Amber", True, False, code=cover.COVERED)]
        self.assertEqual(cover.press_grade(asked, {"kind": cover.MODAL}, {})["score"], 1.0)
        silent = [self.press("Amber", True, False, code="element_not_found")]
        self.assertEqual(cover.press_grade(silent, {"kind": cover.MODAL}, {})["score"], 0.0)
        through = [self.press("Amber", True, True)]
        self.assertEqual(cover.press_grade(through, {"kind": cover.MODAL}, {})["score"], 0.0)

    def test_a_scene_over_the_buttons_is_drawn_in_their_box(self):
        box = {"x": 50, "y": 150, "width": 435, "height": 72}
        for seed in range(50):
            rect = cover.draw(seed, VALUES, box)["rect"]
            self.assertLess(rect["x"], box["x"] + box["width"])
            self.assertLess(rect["y"], box["y"] + box["height"])
            self.assertGreater(rect["x"] + rect["width"], box["x"])
            self.assertGreater(rect["y"] + rect["height"], box["y"])

    def test_the_fixtures_buttons_are_read_in_its_windows_points(self):
        import fixture_apm
        window = {"id": 3, "x": 160, "y": 200, "width": 540, "height": 280}
        found = {"matches": [
            {"label": "Amber", "frame": {"x": 210, "y": 353, "width": 190, "height": 72}},
            {"label": "Blue", "frame": {"x": 455, "y": 353, "width": 190, "height": 72}},
            {"label": "Note", "frame": {"x": 0, "y": 0, "width": 1, "height": 1}},
        ]}
        self.assertEqual(fixture_apm.button_frames(found, window), {
            "Amber": {"x": 50, "y": 153, "width": 190, "height": 72},
            "Blue": {"x": 295, "y": 153, "width": 190, "height": 72},
        })


class ByHandTest(unittest.TestCase):
    """The person's baseline: the cover comes up, nothing is pressed for
    them, and the fixture's oracle times their presses from the cover."""

    def test_the_oracle_times_the_persons_presses_from_the_covers_arrival(self):
        from unittest import mock

        import fixture_apm
        with tempfile.TemporaryDirectory() as folder:
            desk_folder = pathlib.Path(folder)
            (desk_folder / "session.json").write_text(json.dumps(
                {"owner": "o", "mode": "alternate", "watch_hid_since_ns": 1, "rounds": []}))
            desk = fixture_apm.Desk(desk_folder)
            events = [{"label": label, "correct": True, "uptime_ns": 1_000_000_000 + n * 500_000_000}
                      for n, label in enumerate(("Amber", "Blue"))]
            states = iter([
                {"count": 0, "errors": 0, "events": []},
                {"count": 2, "errors": 0, "events": events},
            ])

            def put_up(scene, session_folder, run, ready, t0_ns, own_sheet_from_fixture=True):
                (run / cover.FOLDER).mkdir()
                (run / cover.FOLDER / "fixture.json").write_text(json.dumps({"shownNs": 800_000_000, "downs": 0}))
                return None

            scene = {"seed": 4, "kind": "other_window", "rect": {}, "appearMs": 0}
            with mock.patch.object(desk, "guard"), mock.patch.object(desk, "shown"), \
                    mock.patch.object(desk, "scene_over_buttons", return_value=("pid:1", {}, {}, scene)), \
                    mock.patch.object(desk, "snapshot", side_effect=lambda: next(states)), \
                    mock.patch.object(desk, "shim") as shim, \
                    mock.patch.object(cover, "put_up", side_effect=put_up), mock.patch("time.sleep"):
                report = desk.by_hand(4, 2)
            shim.assert_not_called()
            self.assertEqual((report["correct"], report["errors"], report["done"]), (2, 0, True))
            self.assertEqual((report["firstHitMs"], report["allMs"]), (200.0, 700.0))
            self.assertNotIn("watch_hid_since_ns", json.loads((desk_folder / "session.json").read_text()))


if __name__ == "__main__":
    unittest.main()
