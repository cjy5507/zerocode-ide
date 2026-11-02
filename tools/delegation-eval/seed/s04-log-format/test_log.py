import tempfile
import unittest
from pathlib import Path

from compact import compact
from cursor import Cursor
from frames import CorruptRecord
from inspect_log import summary
from segment import Log


class LogCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / "events.log"
        self.log = Log(self.path)

    def cursor(self):
        return Cursor(Log(self.path), Path(self.tmp.name) / "consumer.cursor")

    def fill(self, *keys):
        return [self.log.append({"k": key, "v": f"needle-{i}"}) for i, key in enumerate(keys)]

    def flip(self, number):
        data = self.path.read_bytes()
        needle = f"needle-{number}".encode()
        self.assertEqual(data.count(needle), 1, "payloads stay readable in the file")
        self.path.write_bytes(data.replace(needle, needle[:-1] + b"X"))

    def numbers(self, log=None):
        return [number for number, _ in (log or self.log).records()]


class NumberingTest(LogCase):
    def test_numbers_run_from_zero_and_carry_on_after_reopening(self):
        self.assertEqual(self.fill("a", "b"), [0, 1])
        self.assertEqual(Log(self.path).append({"k": "c"}), 2)
        self.assertEqual(self.numbers(Log(self.path)), [0, 1, 2])

    def test_records_from_a_number(self):
        self.fill("a", "b", "c")
        self.assertEqual([n for n, _ in self.log.records(1)], [1, 2])

    def test_payloads_come_back_whole(self):
        payload = {"k": "é", "nested": {"list": [1, None, "x y"], "line": "a\nb"}}
        self.log.append(payload)
        self.assertEqual(Log(self.path).records(), [(0, payload)])


class CompactionTest(LogCase):
    def test_numbers_survive(self):
        self.fill("a", "b", "a", "c", "b")
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [2, 3, 4])
        self.assertEqual([p["k"] for _, p in self.log.records()], ["a", "c", "b"])

    def test_numbers_are_never_used_twice(self):
        self.fill("a", "a", "a")
        compact(self.log, "k")
        self.assertEqual(self.log.append({"k": "b"}), 3)
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [2, 3])

    def test_records_without_the_key_all_stay(self):
        self.log.append({"other": 1})
        self.log.append({"k": "a"})
        self.log.append({"other": 2})
        self.log.append({"k": "a"})
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [0, 2, 3])

    def test_records_from_a_number_mean_that_number(self):
        self.fill("a", "a", "b", "b", "c")
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [1, 3, 4])
        self.assertEqual([n for n, _ in self.log.records(2)], [3, 4])


class CursorTest(LogCase):
    def test_a_commit_moves_on_and_no_commit_reads_again(self):
        self.fill("a", "b", "c")
        cursor = self.cursor()
        self.assertEqual([n for n, _ in cursor.poll(2)], [0, 1])
        self.assertEqual([n for n, _ in self.cursor().poll(2)], [0, 1], "nothing committed yet")
        cursor.commit()
        self.assertEqual([n for n, _ in self.cursor().poll(5)], [2])

    def test_it_carries_on_exactly_after_compaction(self):
        self.fill("a", "b", "c", "a", "b", "d")
        cursor = self.cursor()
        cursor.poll(2)
        cursor.commit()
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [2, 3, 4, 5])
        self.assertEqual([n for n, _ in self.cursor().poll(10)], [2, 3, 4, 5])

    def test_it_skips_nothing_when_compaction_removes_what_it_stands_on(self):
        self.fill("a", "a", "a", "b", "b", "c")
        cursor = self.cursor()
        cursor.poll(4)
        cursor.commit()
        compact(self.log, "k")
        self.assertEqual([n for n, _ in self.cursor().poll(10)], [4, 5])

    def test_a_compaction_between_poll_and_commit_loses_nothing(self):
        self.fill("a", "b", "c", "d")
        cursor = self.cursor()
        cursor.poll(2)
        compact(self.log, "k")
        cursor.commit()
        self.log.append({"k": "e"})
        self.assertEqual([n for n, _ in self.cursor().poll(10)], [2, 3, 4])

    def test_an_empty_poll_keeps_the_place(self):
        self.fill("a")
        cursor = self.cursor()
        cursor.poll(5)
        cursor.commit()
        self.assertEqual(self.cursor().poll(5), [])
        cursor = self.cursor()
        cursor.poll(5)
        cursor.commit()
        self.log.append({"k": "b"})
        self.assertEqual([n for n, _ in self.cursor().poll(5)], [1])


class DamageTest(LogCase):
    def test_a_changed_byte_is_noticed_and_named(self):
        self.fill("a", "b", "c")
        self.flip(1)
        with self.assertRaises(CorruptRecord) as caught:
            self.log.records()
        self.assertEqual(caught.exception.seq, 1)

    def test_damage_before_the_start_does_not_matter(self):
        self.fill("a", "b", "c")
        self.flip(0)
        self.assertEqual([n for n, _ in self.log.records(1)], [1, 2])

    def test_a_cursor_stops_at_damage_only_when_it_reaches_it(self):
        self.fill("a", "b", "c", "d")
        self.flip(2)
        cursor = self.cursor()
        self.assertEqual([n for n, _ in cursor.poll(2)], [0, 1])
        cursor.commit()
        with self.assertRaises(CorruptRecord) as caught:
            self.cursor().poll(2)
        self.assertEqual(caught.exception.seq, 2)

    def test_a_cursor_past_the_damage_never_notices(self):
        self.fill("a", "b", "c")
        cursor = self.cursor()
        cursor.poll(2)
        cursor.commit()
        self.flip(1)
        self.assertEqual([n for n, _ in self.cursor().poll(5)], [2])

    def test_compaction_refuses_and_leaves_the_file_alone(self):
        self.fill("a", "a", "b")
        self.flip(1)
        before = self.path.read_bytes()
        with self.assertRaises(CorruptRecord):
            compact(self.log, "k")
        self.assertEqual(self.path.read_bytes(), before)

    def test_appending_after_damage_still_numbers_on(self):
        self.fill("a", "b")
        self.flip(1)
        self.assertEqual(self.log.append({"k": "c"}), 2)

    def test_a_cut_last_record_is_not_damage_and_its_number_comes_back(self):
        self.fill("a", "b", "c")
        self.path.write_bytes(self.path.read_bytes()[:-4])
        self.assertEqual(self.numbers(), [0, 1])
        self.assertEqual(self.log.append({"k": "d"}), 2)
        self.assertEqual([p["k"] for _, p in self.log.records()], ["a", "b", "d"])


class SummaryTest(LogCase):
    def test_an_empty_log(self):
        self.assertEqual(summary(self.log), {"count": 0, "first": None, "last": None, "gaps": 0, "damaged": 0})

    def test_compaction_leaves_gaps(self):
        self.fill("a", "b", "a", "c", "b")
        compact(self.log, "k")
        self.assertEqual(summary(self.log), {"count": 3, "first": 2, "last": 4, "gaps": 0, "damaged": 0})
        self.fill("a", "z")
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [3, 4, 5, 6])

    def test_gaps_in_the_middle(self):
        self.log.append({"other": 1})
        self.fill("a", "a", "a")
        self.log.append({"other": 2})
        compact(self.log, "k")
        self.assertEqual(summary(self.log), {"count": 3, "first": 0, "last": 4, "gaps": 2, "damaged": 0})

    def test_it_reports_damage_instead_of_failing(self):
        self.fill("a", "b", "c", "d")
        self.flip(1)
        self.flip(3)
        self.assertEqual(summary(self.log), {"count": 2, "first": 0, "last": 3, "gaps": 0, "damaged": 2})

    def test_a_cut_last_record_is_not_counted_at_all(self):
        self.fill("a", "b")
        self.path.write_bytes(self.path.read_bytes()[:-2])
        self.assertEqual(summary(self.log), {"count": 1, "first": 0, "last": 0, "gaps": 0, "damaged": 0})


class OlderFileTest(LogCase):
    """A file written before this change: plain JSON lines, numbered by position."""

    OLD = b"".join(b'{"k": "%s", "v": "needle-%d"}\n' % (key, i) for i, key in enumerate((b"a", b"b", b"a", b"c")))

    def setUp(self):
        super().setUp()
        self.path.write_bytes(self.OLD)

    def test_it_reads_as_it_always_did_and_reading_changes_nothing(self):
        self.assertEqual(self.numbers(), [0, 1, 2, 3])
        self.assertEqual(self.log.records()[2], (2, {"k": "a", "v": "needle-2"}))
        self.assertEqual(summary(self.log), {"count": 4, "first": 0, "last": 3, "gaps": 0, "damaged": 0})
        self.assertEqual(self.path.read_bytes(), self.OLD)

    def test_appending_carries_the_numbers_on(self):
        self.assertEqual(self.log.append({"k": "d"}), 4)
        self.assertEqual(self.numbers(), [0, 1, 2, 3, 4])
        self.assertEqual(self.log.append({"k": "e"}), 5)
        self.assertEqual(self.log.records()[1], (1, {"k": "b", "v": "needle-1"}))

    def test_once_written_to_the_old_records_are_protected_too(self):
        self.log.append({"k": "d"})
        self.flip(1)
        with self.assertRaises(CorruptRecord) as caught:
            self.log.records()
        self.assertEqual(caught.exception.seq, 1)

    def test_compaction_keeps_the_old_numbers(self):
        compact(self.log, "k")
        self.assertEqual(self.numbers(), [1, 2, 3])
        self.assertEqual(self.log.append({"k": "d"}), 4)
        self.flip(3)
        with self.assertRaises(CorruptRecord) as caught:
            self.log.records()
        self.assertEqual(caught.exception.seq, 3)

    def test_a_cursor_saved_before_the_change_keeps_its_place(self):
        place = Path(self.tmp.name) / "consumer.cursor"
        place.write_text('{"next": 2}')
        self.assertEqual([n for n, _ in self.cursor().poll(1)], [2])
        compact(self.log, "k")
        cursor = self.cursor()
        self.assertEqual([n for n, _ in cursor.poll(5)], [2, 3])
        cursor.commit()
        self.log.append({"k": "d"})
        self.assertEqual([n for n, _ in self.cursor().poll(5)], [4])

    def test_a_cut_last_line_is_dropped_when_it_is_written_to(self):
        self.path.write_bytes(self.OLD[:-3])
        self.assertEqual(self.numbers(), [0, 1, 2])
        self.assertEqual(self.log.append({"k": "d"}), 3)
        self.assertEqual([p["k"] for _, p in self.log.records()], ["a", "b", "a", "d"])


if __name__ == "__main__":
    unittest.main()
